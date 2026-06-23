//! Resolve LLM credentials from the user's *existing* local agent logins — no
//! new key required — for both providers Crossthreads already indexes:
//! Anthropic (Claude Code) and OpenAI (Codex).
//!
//! Each provider is resolved into an ordered list of usable [`Cred`]s, most
//! preferred first. The ordering puts explicit, sanctioned credentials (a real
//! API key, or a token the user set themselves) above the agent CLI, and the
//! agent CLI above a *scavenged* OAuth token (reused from the agent's own login,
//! which the platform API may reject). Nothing here is written to disk or logged.

use std::path::PathBuf;

use serde::Deserialize;

/// A supported model provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAi,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic (Claude)",
            Provider::OpenAi => "OpenAI (Codex)",
        }
    }

    /// Parse a `CROSSTHREADS_LLM_PROVIDER` value.
    pub fn parse(s: &str) -> Option<Provider> {
        match s.trim().to_lowercase().as_str() {
            "anthropic" | "claude" => Some(Provider::Anthropic),
            "openai" | "codex" => Some(Provider::OpenAi),
            _ => None,
        }
    }
}

/// A usable credential for a provider. Carries enough to actually make the call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cred {
    /// A real API key — direct API call (sanctioned).
    ApiKey(String),
    /// An OAuth bearer token (set explicitly, or reused from the agent login) —
    /// direct API, best-effort: the platform API may reject an app-scoped token.
    OAuthToken(String),
    /// The provider's CLI is installed; shell out to it. Reuses its login.
    Cli,
}

impl Cred {
    pub fn describe(&self) -> &'static str {
        match self {
            Cred::ApiKey(_) => "API key",
            Cred::OAuthToken(_) => "reused login token",
            Cred::Cli => "agent CLI",
        }
    }
}

/// Resolve every usable credential for one provider, most preferred first.
pub fn resolve(provider: Provider) -> Vec<Cred> {
    match provider {
        Provider::Anthropic => resolve_anthropic(),
        Provider::OpenAi => resolve_openai(),
    }
}

/// Providers with at least one usable credential, in auto-preference order.
pub fn available_providers() -> Vec<Provider> {
    [Provider::Anthropic, Provider::OpenAi]
        .into_iter()
        .filter(|p| !resolve(*p).is_empty())
        .collect()
}

// --- OpenAI / Codex -------------------------------------------------------

/// The Codex home: `$CODEX_HOME`, else `~/.codex`. Mirrors the codex connector.
pub fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")))
}

fn resolve_openai() -> Vec<Cred> {
    let mut out = Vec::new();
    if let Some(k) = env_nonempty("OPENAI_API_KEY") {
        out.push(Cred::ApiKey(k));
    }
    let (file_key, file_token) = codex_home()
        .and_then(|h| std::fs::read_to_string(h.join("auth.json")).ok())
        .map(|t| parse_openai_auth(&t))
        .unwrap_or((None, None));
    if let Some(k) = file_key {
        push_unique(&mut out, Cred::ApiKey(k));
    }
    if cli_available("codex") {
        out.push(Cred::Cli);
    }
    // Scavenged ChatGPT-login token ranks *below* the CLI (reorder per review).
    if let Some(t) = file_token {
        out.push(Cred::OAuthToken(t));
    }
    out
}

/// Pure: `(OPENAI_API_KEY, tokens.access_token)` from an `auth.json` body.
pub fn parse_openai_auth(text: &str) -> (Option<String>, Option<String>) {
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
    match serde_json::from_str::<AuthFile>(text) {
        Ok(f) => (
            nonempty(f.openai_api_key),
            f.tokens.and_then(|t| nonempty(t.access_token)),
        ),
        Err(_) => (None, None),
    }
}

// --- Anthropic / Claude ---------------------------------------------------

fn resolve_anthropic() -> Vec<Cred> {
    let mut out = Vec::new();
    // Explicit, user-provided credentials rank highest.
    if let Some(k) = env_nonempty("ANTHROPIC_API_KEY") {
        out.push(Cred::ApiKey(k));
    }
    if let Some(t) = env_nonempty("ANTHROPIC_AUTH_TOKEN") {
        out.push(Cred::OAuthToken(t));
    }
    if cli_available("claude") {
        out.push(Cred::Cli);
    }
    // Scavenged Claude Code OAuth token ranks below the CLI (best-effort).
    if let Some(t) = claude_code_oauth() {
        push_unique(&mut out, Cred::OAuthToken(t));
    }
    out
}

/// The Claude Code config home: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_home() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
}

/// Best-effort: Claude Code's stored OAuth access token from
/// `~/.claude/.credentials.json`. `None` if absent or shaped differently (the
/// macOS keychain store isn't read here — the `claude` CLI fallback covers it).
fn claude_code_oauth() -> Option<String> {
    let path = claude_home()?.join(".credentials.json");
    let text = std::fs::read_to_string(path).ok()?;
    parse_claude_credentials(&text)
}

/// Pure: the OAuth access token from a Claude Code `.credentials.json` body.
pub fn parse_claude_credentials(text: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Creds {
        #[serde(rename = "claudeAiOauth")]
        claude_ai_oauth: Option<OAuth>,
    }
    #[derive(Deserialize)]
    struct OAuth {
        #[serde(rename = "accessToken")]
        access_token: Option<String>,
    }
    let creds: Creds = serde_json::from_str(text).ok()?;
    nonempty(creds.claude_ai_oauth.and_then(|o| o.access_token))
}

// --- shared helpers -------------------------------------------------------

fn env_nonempty(key: &str) -> Option<String> {
    nonempty(std::env::var(key).ok())
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

fn push_unique(out: &mut Vec<Cred>, c: Cred) {
    if !out.contains(&c) {
        out.push(c);
    }
}

/// Whether a runnable `<name>` CLI is on PATH.
pub fn cli_available(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_auth_api_key_and_token() {
        let json = r#"{ "OPENAI_API_KEY": "sk-abc", "tokens": { "access_token": "oauth-xyz" } }"#;
        assert_eq!(
            parse_openai_auth(json),
            (Some("sk-abc".into()), Some("oauth-xyz".into()))
        );
        assert_eq!(parse_openai_auth("{}"), (None, None));
        assert_eq!(parse_openai_auth("nope"), (None, None));
        assert_eq!(
            parse_openai_auth(r#"{ "OPENAI_API_KEY": "  " }"#),
            (None, None)
        );
    }

    #[test]
    fn claude_credentials_access_token() {
        let json =
            r#"{ "claudeAiOauth": { "accessToken": "sk-ant-oat-123", "refreshToken": "r" } }"#;
        assert_eq!(
            parse_claude_credentials(json),
            Some("sk-ant-oat-123".into())
        );
        assert_eq!(parse_claude_credentials("{}"), None);
        assert_eq!(
            parse_claude_credentials(r#"{ "claudeAiOauth": { "accessToken": "" } }"#),
            None
        );
    }

    #[test]
    fn provider_parse() {
        assert_eq!(Provider::parse("claude"), Some(Provider::Anthropic));
        assert_eq!(Provider::parse("OpenAI"), Some(Provider::OpenAi));
        assert_eq!(Provider::parse("codex"), Some(Provider::OpenAi));
        assert_eq!(Provider::parse("gemini"), None);
    }
}
