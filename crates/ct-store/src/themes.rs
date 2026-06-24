//! Theme extraction: cluster the indexed conversations by their embedding
//! centroids and label each cluster by its most distinctive title terms.
//!
//! Pure and offline (no model calls). Shared by the CLI (`crossthreads themes`),
//! the daemon's `Themes` request (for the web UI and MCP), so the algorithm
//! lives in one place. Deterministic: fixed PRNG seed.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ConversationVec;

/// One cluster of conversations that share a theme.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    /// Keyword label — the cluster's most distinctive title terms (`a · b · c`).
    pub label: String,
    /// Number of conversations in the cluster.
    pub size: usize,
    /// Tool mix, dominant first: `[("claude-code", 7), ("cursor", 2)]`.
    pub tools: Vec<(String, usize)>,
    /// A few representative conversations (for display and LLM naming).
    pub samples: Vec<ThemeSample>,
    /// Every member conversation id (for drill-in).
    pub conversation_ids: Vec<String>,
}

/// A representative conversation in a [`Theme`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeSample {
    pub id: String,
    pub title: Option<String>,
    pub tool: String,
}

/// Cluster `convos` into at most `k` themes, each carrying up to `max_samples`
/// sample conversations. Largest theme first. Empty input → empty output.
pub fn cluster(convos: &[ConversationVec], k: usize, max_samples: usize) -> Vec<Theme> {
    if convos.is_empty() || k == 0 {
        return Vec::new();
    }
    let k = k.min(convos.len());
    let vectors: Vec<&[f32]> = convos.iter().map(|c| c.vec.as_slice()).collect();
    let assignments = kmeans(&vectors, k, 25);

    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); k];
    for (i, &c) in assignments.iter().enumerate() {
        groups[c].push(i);
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.len()));

    let doc_freq = global_doc_freq(convos);
    let total = convos.len() as f64;

    groups
        .into_iter()
        .filter(|g| !g.is_empty())
        .map(|g| Theme {
            label: label_cluster(&g, convos, &doc_freq, total),
            size: g.len(),
            tools: tool_mix(&g, convos),
            samples: g
                .iter()
                .take(max_samples)
                .map(|&i| ThemeSample {
                    id: convos[i].id.clone(),
                    title: convos[i].title.clone(),
                    tool: convos[i].tool.clone(),
                })
                .collect(),
            conversation_ids: g.iter().map(|&i| convos[i].id.clone()).collect(),
        })
        .collect()
}

/// Lloyd's k-means with k-means++ seeding, squared-Euclidean distance (the
/// vectors are unit-norm, so this tracks cosine). Deterministic.
fn kmeans(vectors: &[&[f32]], k: usize, iters: usize) -> Vec<usize> {
    let n = vectors.len();
    let dim = vectors[0].len();
    let mut rng = Lcg::new(0x9E3779B97F4A7C15);

    let mut centers: Vec<Vec<f32>> = Vec::with_capacity(k);
    centers.push(vectors[rng.below(n)].to_vec());
    let mut dist2 = vec![f32::INFINITY; n];
    while centers.len() < k {
        let last = centers.last().unwrap();
        let mut total = 0.0f64;
        for (i, v) in vectors.iter().enumerate() {
            let d = sq_dist(v, last);
            if d < dist2[i] {
                dist2[i] = d;
            }
            total += dist2[i] as f64;
        }
        let mut target = rng.unit() * total;
        let mut chosen = n - 1;
        for (i, &d) in dist2.iter().enumerate() {
            target -= d as f64;
            if target <= 0.0 {
                chosen = i;
                break;
            }
        }
        centers.push(vectors[chosen].to_vec());
    }

    let mut assign = vec![0usize; n];
    for _ in 0..iters {
        let mut moved = false;
        for (i, v) in vectors.iter().enumerate() {
            let mut best = 0;
            let mut best_d = f32::INFINITY;
            for (c, center) in centers.iter().enumerate() {
                let d = sq_dist(v, center);
                if d < best_d {
                    best_d = d;
                    best = c;
                }
            }
            if assign[i] != best {
                assign[i] = best;
                moved = true;
            }
        }
        if !moved {
            break;
        }
        let mut sums = vec![vec![0.0f32; dim]; k];
        let mut counts = vec![0usize; k];
        for (i, v) in vectors.iter().enumerate() {
            let c = assign[i];
            for (s, x) in sums[c].iter_mut().zip(v.iter()) {
                *s += *x;
            }
            counts[c] += 1;
        }
        for (c, center) in centers.iter_mut().enumerate() {
            if counts[c] > 0 {
                let inv = 1.0 / counts[c] as f32;
                for (slot, s) in center.iter_mut().zip(&sums[c]) {
                    *slot = s * inv;
                }
            }
        }
    }
    assign
}

fn sq_dist(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

fn global_doc_freq(convos: &[ConversationVec]) -> HashMap<String, usize> {
    let mut df: HashMap<String, usize> = HashMap::new();
    for c in convos {
        for term in terms(c.title.as_deref().unwrap_or("")) {
            *df.entry(term).or_insert(0) += 1;
        }
    }
    df
}

fn label_cluster(
    cluster: &[usize],
    convos: &[ConversationVec],
    doc_freq: &HashMap<String, usize>,
    total: f64,
) -> String {
    let mut tf: HashMap<String, usize> = HashMap::new();
    for &i in cluster {
        for term in terms(convos[i].title.as_deref().unwrap_or("")) {
            *tf.entry(term).or_insert(0) += 1;
        }
    }
    let mut scored: Vec<(String, f64)> = tf
        .into_iter()
        .map(|(term, count)| {
            let df = *doc_freq.get(&term).unwrap_or(&1) as f64;
            let idf = (total / (1.0 + df)).ln().max(0.0);
            (term, count as f64 * idf)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let label: Vec<String> = scored
        .into_iter()
        .filter(|(_, s)| *s > 0.0)
        .take(3)
        .map(|(t, _)| t)
        .collect();
    if label.is_empty() {
        "(mixed)".to_string()
    } else {
        label.join(" · ")
    }
}

fn tool_mix(cluster: &[usize], convos: &[ConversationVec]) -> Vec<(String, usize)> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for &i in cluster {
        *counts.entry(convos[i].tool.as_str()).or_insert(0) += 1;
    }
    let mut pairs: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(t, n)| (t.to_string(), n))
        .collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    pairs
}

/// Lowercased alphanumeric title terms, dropping stopwords and short tokens.
fn terms(title: &str) -> impl Iterator<Item = String> + '_ {
    title
        .split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 3 && !is_stopword(w))
}

fn is_stopword(w: &str) -> bool {
    matches!(
        w,
        "the"
            | "and"
            | "for"
            | "with"
            | "you"
            | "your"
            | "how"
            | "this"
            | "that"
            | "from"
            | "are"
            | "was"
            | "can"
            | "use"
            | "using"
            | "add"
            | "fix"
            | "new"
            | "get"
            | "set"
            | "run"
            | "make"
            | "via"
            | "out"
            | "all"
            | "not"
            | "now"
            | "into"
            | "what"
            | "when"
            | "where"
            | "why"
            | "should"
            | "would"
            | "could"
            | "did"
            | "does"
            | "have"
            | "has"
    )
}

/// Tiny deterministic xorshift PRNG so themes are reproducible run-to-run.
struct Lcg(u64);
impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}
