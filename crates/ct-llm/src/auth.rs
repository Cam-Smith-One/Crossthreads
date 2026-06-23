//! Resolve OpenAI / Codex credentials from the local machine, reusing the same
//! Codex home the connector reads (`$CODEX_HOME`, else `~/.codex`).
//!
//! Tiered and best-effort, in descending preference:
//!   1. a real OpenAI **API key** — `OPENAI_API_KEY` in the environment, then
//!      the `OPENAI_API_KEY` field of `~/.codex/auth.json` (sanctioned);
//!   2. a **ChatGPT-login** OAuth access token from `auth.json` (gray area —
//!      the platform API may reject it);
//!   3. the **`codex` CLI** itself, as a zero-config fallback that reuses
//!      whatever login it already has.
//!
//! Nothing here is written to disk and no token is logged.

use std::path::PathBuf;

use serde::Deserialize;

/// A usable way to reach an OpenAI-compatible model, most-preferred first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    /// A real OpenAI API key (env or `auth.json`) — direct API call.
    ApiKey(String),
    /// A ChatGPT-login OAuth access token from `auth.json` — direct API call,
    /// best-effort (`api.openai.com` may reject a ChatGPT-scoped token).
    ChatGptToken(String),
    /// The `codex` CLI is installed; shell out to it. Reuses its login.
    CodexCli,
}

impl AuthMethod {
    /// A short, secret-free label for diagnostics (`crossthreads codex-auth`).
    pub fn describe(&self) -> &'static str {
        match self {
            AuthMethod::ApiKey(_) => "OpenAI API key",
            AuthMethod::ChatGptToken(_) => "ChatGPT-login token",
            AuthMethod::CodexCli => "codex CLI",
        }
    }
}

/// The Codex home: `$CODEX_HOME`, else `~/.codex`. Mirrors the codex connector.
pub fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")))
}

/// Resolve every available auth method, most-preferred first. Reads the
/// environment, `auth.json`, and probes the `codex` CLI.
pub fn resolve() -> Vec<AuthMethod> {
    let mut out = Vec::new();
    if let Some(k) = non_empty(std::env::var("OPENAI_API_KEY").ok()) {
        out.push(AuthMethod::ApiKey(k));
    }
    if let Some(text) = read_auth_file() {
        for m in parse_auth_json(&text) {
            if !out.contains(&m) {
                out.push(m);
            }
        }
    }
    if codex_cli_available() {
        out.push(AuthMethod::CodexCli);
    }
    out
}

/// Pure: the API-key and ChatGPT-token methods present in an `auth.json` body.
/// Separated from disk/env so it's unit-testable.
pub fn parse_auth_json(text: &str) -> Vec<AuthMethod> {
    #[derive(Deserialize)]
    struct AuthFile {
        #[serde(rename = "OPENAI_API_KEY")]
        openai_api_key: Option<String>,
        tokens: Option<Tokens>,
    }
    #[derive(Deserialize)]
    struct Tokens {
        access_token: Option<String>,
    }

    let mut out = Vec::new();
    let Ok(file) = serde_json::from_str::<AuthFile>(text) else {
        return out;
    };
    if let Some(k) = non_empty(file.openai_api_key) {
        out.push(AuthMethod::ApiKey(k));
    }
    if let Some(tok) = file.tokens.and_then(|t| non_empty(t.access_token)) {
        out.push(AuthMethod::ChatGptToken(tok));
    }
    out
}

fn read_auth_file() -> Option<String> {
    let path = codex_home()?.join("auth.json");
    std::fs::read_to_string(path).ok()
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

/// Whether a runnable `codex` is on PATH.
pub fn codex_cli_available() -> bool {
    std::process::Command::new("codex")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_key_then_token_in_order() {
        let json = r#"{ "OPENAI_API_KEY": "sk-abc", "tokens": { "access_token": "oauth-xyz" } }"#;
        assert_eq!(
            parse_auth_json(json),
            vec![
                AuthMethod::ApiKey("sk-abc".into()),
                AuthMethod::ChatGptToken("oauth-xyz".into()),
            ]
        );
    }

    #[test]
    fn token_only_when_no_api_key() {
        let json = r#"{ "tokens": { "access_token": "oauth-xyz" } }"#;
        assert_eq!(
            parse_auth_json(json),
            vec![AuthMethod::ChatGptToken("oauth-xyz".into())]
        );
    }

    #[test]
    fn blank_and_missing_fields_are_skipped() {
        assert!(parse_auth_json(r#"{ "OPENAI_API_KEY": "  " }"#).is_empty());
        assert!(parse_auth_json("{}").is_empty());
        assert!(parse_auth_json("not json").is_empty());
    }
}
