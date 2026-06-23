//! `crossthreads themes` — cluster the indexed conversations by their embeddings
//! and surface the themes you spend time on. A prototype (ADR-pending): the
//! clustering and labels are computed locally and offline — no model calls —
//! so it works anywhere the index does. Labels are the most *distinctive* terms
//! from each cluster's titles (a later pass can name clusters with an LLM).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_store::{ConversationVec, Store};

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut db: Option<PathBuf> = None;
    let mut k: usize = 8;
    let mut samples: usize = 3;
    let mut name = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--name" => name = true,
            "--db" => {
                db = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?,
                ))
            }
            "--k" => {
                k = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--k needs a value"))?
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--k must be a number"))?
            }
            "--samples" => {
                samples = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--samples needs a value"))?
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--samples must be a number"))?
            }
            other => bail!("unknown option for `themes`: {other}"),
        }
    }
    if k == 0 {
        bail!("--k must be at least 1");
    }

    let db_path = crate::resolve_db(db)?;
    let store = Store::open(&db_path)?;
    let convos = store.conversation_vectors()?;
    if convos.is_empty() {
        println!(
            "No embeddings yet — run `crossthreads index` first (semantic themes need vectors)."
        );
        return Ok(ExitCode::SUCCESS);
    }
    let k = k.min(convos.len());

    let vectors: Vec<&[f32]> = convos.iter().map(|c| c.vec.as_slice()).collect();
    let assignments = kmeans(&vectors, k, 25);

    // Group conversation indices by cluster.
    let mut clusters: Vec<Vec<usize>> = vec![Vec::new(); k];
    for (i, &c) in assignments.iter().enumerate() {
        clusters[c].push(i);
    }
    // Largest themes first.
    clusters.sort_by_key(|c| std::cmp::Reverse(c.len()));

    let doc_freq = global_doc_freq(&convos);
    let total = convos.len() as f64;

    // `--name` uses the user's local Claude/Codex login to name each cluster;
    // it degrades to the offline keyword label if no auth or the call fails.
    if name && !ct_llm::available() {
        eprintln!(
            "note: --name needs model auth — set ANTHROPIC_API_KEY or OPENAI_API_KEY, sign into \
             Claude Code or Codex, or install a CLI (`crossthreads llm-auth`). Using keyword \
             labels.\n"
        );
    }

    println!(
        "Themes across {} conversations ({} clusters):\n",
        convos.len(),
        clusters.iter().filter(|c| !c.is_empty()).count()
    );
    for cluster in clusters.iter().filter(|c| !c.is_empty()) {
        let keywords = label_cluster(cluster, &convos, &doc_freq, total);
        let label = if name {
            llm_name(cluster, &convos, &keywords).unwrap_or(keywords)
        } else {
            keywords
        };
        let tools = tool_mix(cluster, &convos);
        println!("▸ {label}  ({} conversations · {tools})", cluster.len());
        for &i in cluster.iter().take(samples) {
            let title = convos[i].title.as_deref().unwrap_or("(untitled)");
            println!("    · {}", title.trim());
        }
        println!();
    }
    Ok(ExitCode::SUCCESS)
}

/// Lloyd's k-means with k-means++ seeding, squared-Euclidean distance (the
/// vectors are unit-norm, so this tracks cosine). Deterministic: fixed seed.
fn kmeans(vectors: &[&[f32]], k: usize, iters: usize) -> Vec<usize> {
    let n = vectors.len();
    let dim = vectors[0].len();
    let mut rng = Lcg::new(0x9E3779B97F4A7C15);

    // k-means++ seeding.
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
        // Weighted pick proportional to distance² (falls back to uniform).
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
        // Recompute centers as the mean of assigned vectors.
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

/// Document frequency of each title term across the whole corpus.
fn global_doc_freq(convos: &[ConversationVec]) -> HashMap<String, usize> {
    let mut df: HashMap<String, usize> = HashMap::new();
    for c in convos {
        for term in terms(c.title.as_deref().unwrap_or("")) {
            *df.entry(term).or_insert(0) += 1;
        }
    }
    df
}

/// Label a cluster by its most distinctive title terms (TF in the cluster
/// weighted by inverse corpus document frequency).
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

/// Ask the LLM (via the user's local Codex/OpenAI login) for a concise theme
/// name, given the cluster's distinctive keywords and a few sample titles.
/// `None` on any error, so the caller falls back to the keyword label.
fn llm_name(cluster: &[usize], convos: &[ConversationVec], keywords: &str) -> Option<String> {
    let titles: Vec<&str> = cluster
        .iter()
        .take(8)
        .map(|&i| convos[i].title.as_deref().unwrap_or("").trim())
        .filter(|t| !t.is_empty())
        .collect();
    let user = format!(
        "These AI coding-session titles form one cluster (keywords: {keywords}).\n\
         Give a 2-4 word theme name. Reply with only the name.\n\n- {}",
        titles.join("\n- ")
    );
    let system =
        "You label clusters of software-engineering chat sessions with a short, specific theme.";
    let raw = ct_llm::complete(system, &user).ok()?;
    // Keep it to one tidy line.
    let name = raw
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .trim();
    if name.is_empty() || name.len() > 60 {
        None
    } else {
        Some(name.to_string())
    }
}

/// A short "tool: count" summary for a cluster, dominant tool first.
fn tool_mix(cluster: &[usize], convos: &[ConversationVec]) -> String {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for &i in cluster {
        *counts.entry(convos[i].tool.as_str()).or_insert(0) += 1;
    }
    let mut pairs: Vec<(&str, usize)> = counts.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    pairs
        .iter()
        .take(3)
        .map(|(t, n)| format!("{t} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Tokenize a title into lowercased alphanumeric terms, dropping stopwords and
/// very short tokens.
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

/// Tiny deterministic xorshift-style PRNG so themes are reproducible run-to-run.
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
