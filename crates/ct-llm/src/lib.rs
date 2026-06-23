//! Optional LLM backend for Crossthreads.
//!
//! Local-first: the index and core features never need this. It powers opt-in
//! niceties (e.g. naming theme clusters) and reuses the user's *existing* agent
//! login — Claude Code or Codex — rather than asking for a new key. See [`auth`].
//!
//! [`complete`] picks a provider (an explicit `CROSSTHREADS_LLM_PROVIDER`, else
//! whichever is available, Anthropic preferred) and walks that provider's
//! credentials in preference order until one succeeds.

pub mod auth;

pub use auth::{available_providers, resolve, Cred, Provider};

use anyhow::{bail, Context, Result};

/// Cheap defaults aimed at short labeling calls; override per provider.
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";
const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";

/// True when at least one provider has a usable credential (for feature-gating).
pub fn available() -> bool {
    !auth::available_providers().is_empty()
}

/// Generate a short completion for `user` (steered by an optional `system`).
/// Tries the selected provider's credentials until one works; errors only when
/// nothing is available or every attempt failed.
pub fn complete(system: &str, user: &str) -> Result<String> {
    let providers = provider_order();
    if providers.is_empty() {
        bail!(
            "no LLM auth found — set ANTHROPIC_API_KEY or OPENAI_API_KEY, sign into Claude Code \
             or Codex, or install one of their CLIs"
        );
    }
    let mut last_err = None;
    for provider in providers {
        for cred in auth::resolve(provider) {
            let attempt = match provider {
                Provider::Anthropic => anthropic_complete(&cred, system, user),
                Provider::OpenAi => openai_complete(&cred, system, user),
            };
            match attempt {
                Ok(text) => return Ok(text),
                Err(e) => {
                    last_err =
                        Some(e.context(format!("{} via {}", provider.label(), cred.describe())))
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("all providers failed")))
}

/// Provider attempt order: an explicit override, else every available provider.
fn provider_order() -> Vec<Provider> {
    if let Ok(name) = std::env::var("CROSSTHREADS_LLM_PROVIDER") {
        if let Some(p) = Provider::parse(&name) {
            return vec![p];
        }
    }
    auth::available_providers()
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

// --- Anthropic ------------------------------------------------------------

fn anthropic_complete(cred: &Cred, system: &str, user: &str) -> Result<String> {
    if let Cred::Cli = cred {
        return claude_cli(system, user);
    }
    let base = env_or("ANTHROPIC_BASE_URL", "https://api.anthropic.com");
    let url = format!("{}/v1/messages", base.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": env_or("CROSSTHREADS_ANTHROPIC_MODEL", DEFAULT_ANTHROPIC_MODEL),
        "max_tokens": 64,
        "system": system,
        "messages": [{ "role": "user", "content": user }],
    });
    let mut req = ureq::post(&url).set("anthropic-version", "2023-06-01");
    req = match cred {
        Cred::ApiKey(k) => req.set("x-api-key", k),
        // OAuth tokens use Bearer + the oauth beta header.
        Cred::OAuthToken(t) => req
            .set("Authorization", &format!("Bearer {t}"))
            .set("anthropic-beta", "oauth-2025-04-20"),
        Cred::Cli => unreachable!(),
    };
    let resp = req
        .send_json(body)
        .map_err(|e| anyhow::anyhow!("request failed: {e}"))?;
    let v: serde_json::Value = resp.into_json().context("decoding response")?;
    let text = v["content"][0]["text"]
        .as_str()
        .context("no text content in response")?
        .trim()
        .to_string();
    if text.is_empty() {
        bail!("empty completion");
    }
    Ok(text)
}

fn claude_cli(system: &str, user: &str) -> Result<String> {
    let mut cmd = std::process::Command::new("claude");
    cmd.arg("-p").arg(user);
    if !system.is_empty() {
        cmd.arg("--append-system-prompt").arg(system);
    }
    run_cli(cmd, "claude")
}

// --- OpenAI / Codex -------------------------------------------------------

fn openai_complete(cred: &Cred, system: &str, user: &str) -> Result<String> {
    if let Cred::Cli = cred {
        return codex_cli(system, user);
    }
    let bearer = match cred {
        Cred::ApiKey(k) | Cred::OAuthToken(k) => k,
        Cred::Cli => unreachable!(),
    };
    let base = env_or("OPENAI_BASE_URL", "https://api.openai.com/v1");
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": env_or("CROSSTHREADS_OPENAI_MODEL", DEFAULT_OPENAI_MODEL),
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

fn codex_cli(system: &str, user: &str) -> Result<String> {
    let prompt = if system.is_empty() {
        user.to_string()
    } else {
        format!("{system}\n\n{user}")
    };
    let mut cmd = std::process::Command::new("codex");
    cmd.arg("exec").arg(&prompt);
    run_cli(cmd, "codex")
}

fn run_cli(mut cmd: std::process::Command, name: &str) -> Result<String> {
    let out = cmd.output().with_context(|| format!("spawning {name}"))?;
    if !out.status.success() {
        bail!(
            "{name} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        bail!("{name} produced no output");
    }
    Ok(text)
}
