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
    Google,
}

/// Every provider, in default preference order.
pub const ALL: [Provider; 3] = [Provider::Anthropic, Provider::OpenAi, Provider::Google];

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic (Claude)",
            Provider::OpenAi => "OpenAI (Codex)",
            Provider::Google => "Google (Gemini)",
        }
    }

    /// Stable id used in the protocol / keychain: `anthropic` | `openai` | `google`.
    pub fn id(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
            Provider::Google => "google",
        }
    }

    /// Parse a `CROSSTHREADS_LLM_PROVIDER` value or a provider id.
    pub fn parse(s: &str) -> Option<Provider> {
        match s.trim().to_lowercase().as_str() {
            "anthropic" | "claude" => Some(Provider::Anthropic),
            "openai" | "codex" => Some(Provider::OpenAi),
            "google" | "gemini" => Some(Provider::Google),
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
        Provider::Google => resolve_google(),
    }
}

/// Providers with at least one usable credential, in auto-preference order.
pub fn available_providers() -> Vec<Provider> {
    ALL.into_iter()
        .filter(|p| !resolve(*p).is_empty())
        .collect()
}

// --- Google / Gemini ------------------------------------------------------

fn resolve_google() -> Vec<Cred> {
    let mut out = Vec::new();
    if let Some(k) = crate::store::get_key(Provider::Google) {
        out.push(Cred::ApiKey(k));
    }
    if let Some(k) = env_nonempty("GEMINI_API_KEY").or_else(|| env_nonempty("GOOGLE_API_KEY")) {
        push_unique(&mut out, Cred::ApiKey(k));
    }
    if cli_available("gemini") {
        out.push(Cred::Cli);
    }
    out
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
    // A key the user pasted in Settings (keychain) wins.
    if let Some(k) = crate::store::get_key(Provider::OpenAi) {
        out.push(Cred::ApiKey(k));
    }
    if let Some(k) = env_nonempty("OPENAI_API_KEY") {
        push_unique(&mut out, Cred::ApiKey(k));
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
    // A scavenged ChatGPT-login token does NOT authenticate against the public
    // OpenAI API (it 401s) — a ChatGPT/Codex subscription isn't an API key. So we
    // do *not* offer it by default; the `codex` CLI above is the real reuse path.
    // Power users pointing OPENAI_BASE_URL at a gateway that accepts the token can
    // opt back in.
    if std::env::var_os("CROSSTHREADS_OPENAI_USE_LOGIN_TOKEN").is_some() {
        if let Some(t) = file_token {
            out.push(Cred::OAuthToken(t));
        }
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
    // A key the user pasted in Settings (keychain) wins.
    if let Some(k) = crate::store::get_key(Provider::Anthropic) {
        out.push(Cred::ApiKey(k));
    }
    // Explicit, user-provided credentials rank highest.
    if let Some(k) = env_nonempty("ANTHROPIC_API_KEY") {
        push_unique(&mut out, Cred::ApiKey(k));
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

/// Whether a runnable `<name>` CLI can be found (on PATH or a common install
/// location). See [`cli_path`].
pub fn cli_available(name: &str) -> bool {
    cli_path(name).is_some()
        // Windows fallback (cli_path looks for the bare name): try to run it.
        || std::process::Command::new(name)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

/// Resolve a CLI binary to an absolute path, searching `PATH` first and then the
/// usual install locations a **GUI-launched** process often *doesn't* inherit
/// (Homebrew, `~/.local/bin`, npm/nvm/bun/volta global bins, `~/.codex/bin`).
/// This is why a desktop-launched daemon couldn't find `codex` and fell back to
/// a credential that doesn't work — see [`resolve_openai`].
pub fn cli_path(name: &str) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(name);
            if is_executable(&p) {
                return Some(p);
            }
        }
    }
    let mut dirs_to_try: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ];
    if let Some(home) = dirs::home_dir() {
        for sub in [
            ".local/bin",
            ".npm-global/bin",
            ".bun/bin",
            ".deno/bin",
            ".volta/bin",
            ".cargo/bin",
            ".codex/bin",
        ] {
            dirs_to_try.push(home.join(sub));
        }
        // nvm/fnm keep node (and its global bins like `codex`) under a per-version
        // dir — scan them so an nvm-installed CLI is still found.
        for base in [".nvm/versions/node", ".local/share/fnm/node-versions"] {
            if let Ok(rd) = std::fs::read_dir(home.join(base)) {
                for entry in rd.flatten() {
                    dirs_to_try.push(entry.path().join("bin"));
                    dirs_to_try.push(entry.path().join("installation").join("bin"));
                }
            }
        }
    }
    dirs_to_try
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

/// Whether `p` is a regular file with an executable bit (any) set.
fn is_executable(p: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
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

    #[cfg(unix)]
    #[test]
    fn cli_path_resolves_real_binary_not_bogus() {
        // `sh` exists in /bin on every unix; a nonsense name resolves to None.
        assert!(cli_path("sh").is_some());
        assert!(cli_path("ct-nonexistent-binary-zzz").is_none());
    }

    #[test]
    fn provider_parse() {
        assert_eq!(Provider::parse("claude"), Some(Provider::Anthropic));
        assert_eq!(Provider::parse("OpenAI"), Some(Provider::OpenAi));
        assert_eq!(Provider::parse("codex"), Some(Provider::OpenAi));
        assert_eq!(Provider::parse("gemini"), Some(Provider::Google));
        assert_eq!(Provider::parse("google"), Some(Provider::Google));
        assert_eq!(Provider::parse("mistral"), None);
    }
}
