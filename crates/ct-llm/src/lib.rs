//! Optional LLM backend for Crossthreads.
//!
//! Local-first: the index and core features never need this. It only powers
//! opt-in niceties (e.g. naming theme clusters) and reuses the user's *existing*
//! Codex / OpenAI login rather than asking for a new key — see [`auth`].
//!
//! [`complete`] walks the resolved auth methods in preference order until one
//! succeeds, so a working API key is used before a best-effort ChatGPT token,
//! and the `codex` CLI is the final fallback.

pub mod auth;

pub use auth::{resolve, AuthMethod};

use anyhow::{bail, Context, Result};

/// Default model for direct API calls; override with `CROSSTHREADS_OPENAI_MODEL`.
const DEFAULT_MODEL: &str = "gpt-4o-mini";

/// True when at least one auth method is available (cheap; for feature-gating).
pub fn available() -> bool {
    !auth::resolve().is_empty()
}

/// Generate a short completion for `user` (steered by an optional `system`),
/// trying each resolved auth method until one works. Errors only when no method
/// is available, or every method failed.
pub fn complete(system: &str, user: &str) -> Result<String> {
    let methods = auth::resolve();
    if methods.is_empty() {
        bail!(
            "no Codex/OpenAI auth found — set OPENAI_API_KEY, run `codex login`, \
             or install the codex CLI"
        );
    }
    let mut last_err = None;
    for method in &methods {
        let attempt = match method {
            AuthMethod::ApiKey(k) | AuthMethod::ChatGptToken(k) => api_complete(k, system, user),
            AuthMethod::CodexCli => cli_complete(system, user),
        };
        match attempt {
            Ok(text) => return Ok(text),
            Err(e) => last_err = Some(e.context(format!("via {}", method.describe()))),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("all auth methods failed")))
}

fn model() -> String {
    std::env::var("CROSSTHREADS_OPENAI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string())
}

fn base_url() -> String {
    std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string())
}

/// One chat completion over the OpenAI-compatible HTTP API with a Bearer token.
fn api_complete(bearer: &str, system: &str, user: &str) -> Result<String> {
    let url = format!("{}/chat/completions", base_url().trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model(),
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
        "max_tokens": 64,
        "temperature": 0.2,
    });
    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {bearer}"))
        .send_json(body)
        .map_err(|e| anyhow::anyhow!("request failed: {e}"))?;
    let v: serde_json::Value = resp.into_json().context("decoding response")?;
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .context("no message content in response")?
        .trim()
        .to_string();
    if text.is_empty() {
        bail!("empty completion");
    }
    Ok(text)
}

/// Fallback: run the installed `codex` CLI non-interactively and capture stdout.
/// Reuses whatever login Codex already has — no token handling here.
fn cli_complete(system: &str, user: &str) -> Result<String> {
    let prompt = if system.is_empty() {
        user.to_string()
    } else {
        format!("{system}\n\n{user}")
    };
    let out = std::process::Command::new("codex")
        .arg("exec")
        .arg(&prompt)
        .output()
        .context("spawning codex")?;
    if !out.status.success() {
        bail!(
            "codex exec failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        bail!("codex produced no output");
    }
    Ok(text)
}
