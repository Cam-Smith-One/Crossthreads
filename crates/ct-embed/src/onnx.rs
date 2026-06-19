//! Real sentence embeddings via `fastembed` (all-MiniLM-L6-v2, 384-dim).
//!
//! On a normal machine `fastembed` downloads the model on first use. In locked-
//! down environments, pre-seed an hf-hub-style cache and set the env vars
//! `FASTEMBED_CACHE_DIR` and `HF_HUB_OFFLINE=1` (see `docs/DEVELOPMENT.md`). The
//! ONNX runtime itself is bundled by `ort`.

use std::sync::Mutex;

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::Embedder;

const MODEL_ID: &str = "all-MiniLM-L6-v2";
const DIM: usize = 384;

pub struct OnnxEmbedder {
    // fastembed's `embed` takes `&mut self`; wrap so our trait can stay `&self`.
    inner: Mutex<TextEmbedding>,
}

impl OnnxEmbedder {
    /// Load the model, honoring `FASTEMBED_CACHE_DIR` / `HF_HUB_OFFLINE`.
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(false),
        )
        .context("loading all-MiniLM-L6-v2")?;
        Ok(Self {
            inner: Mutex::new(model),
        })
    }
}

impl Embedder for OnnxEmbedder {
    fn dim(&self) -> usize {
        DIM
    }

    fn id(&self) -> &str {
        MODEL_ID
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut model = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("embedder mutex poisoned"))?;
        model.embed(texts, None).context("embedding texts")
    }
}
