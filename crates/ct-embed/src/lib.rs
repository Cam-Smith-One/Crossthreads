//! `ct-embed` — text embedding behind a small trait so the index/search layer
//! is agnostic to the model (ADR-006: pure-Rust ONNX).
//!
//! Two implementations:
//! - [`HashEmbedder`] — deterministic, offline, dependency-free. The default,
//!   so the workspace builds and tests with no network or model. It captures
//!   lexical overlap (shared words → similar vectors) but **not** true
//!   semantics; it exists to exercise the pipeline, not to ship quality search.
//! - `OnnxEmbedder` (feature `onnx`) — real sentence embeddings via
//!   `fastembed` (all-MiniLM-L6-v2, 384-dim). This is the shipping path.
//!
//! [`default_embedder`] picks the best available at runtime.

use anyhow::Result;

mod hash;
pub use hash::HashEmbedder;

#[cfg(feature = "onnx")]
mod onnx;
#[cfg(feature = "onnx")]
pub use onnx::OnnxEmbedder;

/// Produces fixed-dimension vectors for text.
pub trait Embedder: Send + Sync {
    /// Output dimensionality. Stable for a given embedder instance.
    fn dim(&self) -> usize;

    /// Short identifier stamped alongside stored vectors (e.g. model name), so
    /// the index can detect when vectors were produced by a different embedder.
    fn id(&self) -> &str;

    /// Embed a batch of texts. Returns one vector per input, in order.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Convenience for a single text.
    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut v = self.embed(std::slice::from_ref(&text.to_string()))?;
        Ok(v.pop().unwrap_or_default())
    }
}

/// Best embedder available: the real ONNX model when compiled in and loadable,
/// otherwise the deterministic hash embedder.
pub fn default_embedder() -> Box<dyn Embedder> {
    #[cfg(feature = "onnx")]
    {
        match OnnxEmbedder::new() {
            Ok(e) => return Box::new(e),
            Err(e) => {
                eprintln!("warn: ONNX embedder unavailable ({e:#}); falling back to hash embedder")
            }
        }
    }
    Box::new(HashEmbedder::default())
}

/// Cosine similarity between two equal-length vectors. Returns 0.0 for a zero
/// vector. Shared by callers that rank by similarity.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}
