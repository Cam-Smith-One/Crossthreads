//! Deterministic, offline hash embedder.
//!
//! A hashing bag-of-words: each lowercase word token is hashed into a bucket of
//! a fixed-width vector, accumulated, then L2-normalized. Shared vocabulary
//! yields nonzero cosine similarity — enough to exercise vector storage and RRF
//! fusion in tests without a model — but it does **not** capture meaning
//! (synonyms don't match). Not for production retrieval quality.

use crate::Embedder;

const DEFAULT_DIM: usize = 384;

pub struct HashEmbedder {
    dim: usize,
}

impl Default for HashEmbedder {
    fn default() -> Self {
        Self { dim: DEFAULT_DIM }
    }
}

impl HashEmbedder {
    pub fn with_dim(dim: usize) -> Self {
        Self { dim: dim.max(1) }
    }

    fn embed_text(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dim];
        for token in text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
        {
            let h = fnv1a(&token.to_lowercase());
            let idx = (h % self.dim as u64) as usize;
            // Sign from a different hash bit to reduce collisions cancelling.
            let sign = if (h >> 33) & 1 == 0 { 1.0 } else { -1.0 };
            v[idx] += sign;
        }
        l2_normalize(&mut v);
        v
    }
}

impl Embedder for HashEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn id(&self) -> &str {
        "hash-bow-v1"
    }

    fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.embed_text(t)).collect())
    }
}

fn fnv1a(s: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cosine;

    #[test]
    fn shared_words_are_more_similar_than_disjoint() {
        let e = HashEmbedder::default();
        let v = e.embed(&[
            "add retry logic with backoff".into(),
            "retry with backoff please".into(),
            "completely unrelated banana sentence".into(),
        ])
        .unwrap();
        let near = cosine(&v[0], &v[1]);
        let far = cosine(&v[0], &v[2]);
        assert!(near > far, "near={near} far={far}");
    }

    #[test]
    fn deterministic() {
        let e = HashEmbedder::default();
        let a = e.embed_one("hello world").unwrap();
        let b = e.embed_one("hello world").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), e.dim());
    }
}
