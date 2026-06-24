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
pub mod store;

pub use auth::{available_providers, resolve, Cred, Provider};

use anyhow::{bail, Context, Result};

/// A provider's resolved auth state, for the Settings UI / `llm-auth` (secret-free).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderStatus {
    /// Stable id: `anthropic` | `openai`.
    pub id: String,
    /// Human label, e.g. "Anthropic (Claude)".
    pub label: String,
    /// Resolved credential methods, most-preferred first (e.g. "API key").
    pub methods: Vec<String>,
    /// Whether a BYO key is stored in the keychain for this provider.
    pub has_stored_key: bool,
}

/// Per-provider auth status plus the pinned active provider (if any). Secret-free.
pub fn status() -> (Vec<ProviderStatus>, Option<String>) {
    let providers = auth::ALL
        .into_iter()
        .map(|p| ProviderStatus {
            id: p.id().to_string(),
            label: p.label().to_string(),
            methods: auth::resolve(p)
                .iter()
                .map(|c| c.describe().to_string())
                .collect(),
            has_stored_key: store::get_key(p).is_some(),
        })
        .collect();
    let active = store::active_provider().map(|p| p.id().to_string());
    (providers, active)
}

/// Cheap defaults aimed at short labeling calls; override per provider.
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";
const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
const DEFAULT_GOOGLE_MODEL: &str = "gemini-2.0-flash";

/// True when at least one provider has a usable credential (for feature-gating).
pub fn available() -> bool {
    !auth::available_providers().is_empty()
}

/// Generate a short completion (~64 tokens) — for labels and names.
pub fn complete(system: &str, user: &str) -> Result<String> {
    complete_with(system, user, 64)
}

/// Generate a longer completion (~1024 tokens) — for synthesis (open loops,
/// knowledge cards, digests, …).
pub fn complete_long(system: &str, user: &str) -> Result<String> {
    complete_with(system, user, 1024)
}

/// Generate a completion of up to `max_tokens`, trying the selected provider's
/// credentials until one works; errors only when nothing is available or every
/// attempt failed.
pub fn complete_with(system: &str, user: &str, max_tokens: u32) -> Result<String> {
    let providers = provider_order();
    if providers.is_empty() {
        bail!(
            "no LLM auth found — add a key in Settings → Models, set ANTHROPIC_API_KEY / \
             OPENAI_API_KEY / GEMINI_API_KEY, or sign into Claude Code / Codex / Gemini"
        );
    }
    let mut last_err = None;
    for provider in providers {
        for cred in auth::resolve(provider) {
            let attempt = match provider {
                Provider::Anthropic => anthropic_complete(&cred, system, user, max_tokens),
                Provider::OpenAi => openai_complete(&cred, system, user, max_tokens),
                Provider::Google => google_complete(&cred, system, user, max_tokens),
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

/// Provider attempt order: an explicit env override, else the keychain-pinned
/// provider first, else every available provider in default order.
fn provider_order() -> Vec<Provider> {
    if let Ok(name) = std::env::var("CROSSTHREADS_LLM_PROVIDER") {
        if let Some(p) = Provider::parse(&name) {
            return vec![p];
        }
    }
    let mut order = auth::available_providers();
    if let Some(pinned) = store::active_provider() {
        order.retain(|p| *p != pinned);
        order.insert(0, pinned);
    }
    order
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

// --- Anthropic ------------------------------------------------------------

fn anthropic_complete(cred: &Cred, system: &str, user: &str, max_tokens: u32) -> Result<String> {
    if let Cred::Cli = cred {
        return claude_cli(system, user);
    }
    let base = env_or("ANTHROPIC_BASE_URL", "https://api.anthropic.com");
    let url = format!("{}/v1/messages", base.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": env_or("CROSSTHREADS_ANTHROPIC_MODEL", DEFAULT_ANTHROPIC_MODEL),
        "max_tokens": max_tokens,
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

fn openai_complete(cred: &Cred, system: &str, user: &str, max_tokens: u32) -> Result<String> {
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
        "max_tokens": max_tokens,
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

// --- Google / Gemini ------------------------------------------------------

fn google_complete(cred: &Cred, system: &str, user: &str, max_tokens: u32) -> Result<String> {
    if let Cred::Cli = cred {
        return gemini_cli(system, user);
    }
    let key = match cred {
        Cred::ApiKey(k) | Cred::OAuthToken(k) => k,
        Cred::Cli => unreachable!(),
    };
    let base = env_or(
        "GOOGLE_BASE_URL",
        "https://generativelanguage.googleapis.com/v1beta",
    );
    let model = env_or("CROSSTHREADS_GOOGLE_MODEL", DEFAULT_GOOGLE_MODEL);
    let url = format!(
        "{}/models/{model}:generateContent?key={key}",
        base.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "systemInstruction": { "parts": [{ "text": system }] },
        "contents": [{ "role": "user", "parts": [{ "text": user }] }],
        "generationConfig": { "maxOutputTokens": max_tokens, "temperature": 0.2 },
    });
    let resp = ureq::post(&url)
        .send_json(body)
        .map_err(|e| anyhow::anyhow!("request failed: {e}"))?;
    let v: serde_json::Value = resp.into_json().context("decoding response")?;
    let text = v["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .context("no text in response")?
        .trim()
        .to_string();
    if text.is_empty() {
        bail!("empty completion");
    }
    Ok(text)
}

fn gemini_cli(system: &str, user: &str) -> Result<String> {
    let prompt = if system.is_empty() {
        user.to_string()
    } else {
        format!("{system}\n\n{user}")
    };
    let mut cmd = std::process::Command::new("gemini");
    cmd.arg("-p").arg(&prompt);
    run_cli(cmd, "gemini")
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
