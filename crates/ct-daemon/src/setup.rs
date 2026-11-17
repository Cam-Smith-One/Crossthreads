//! What's set up on this machine, for the web app's "Get started" checklist.
//!
//! Each check only reads a file the corresponding installer writes, so it is
//! cheap and changes nothing. It answers "is it there?", not "does it work?".

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What is installed, and what is turned on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStatus {
    /// `crossthreads hooks install` has run, per tool.
    pub hooks_claude: bool,
    pub hooks_codex: bool,
    /// The MCP server is registered, per tool.
    pub mcp_claude: bool,
    pub mcp_codex: bool,
    /// `crossthreads skill install` has run, per tool.
    pub skill_claude: bool,
    pub skill_codex: bool,
    /// A model is available for Ask and the analyses.
    pub model: bool,
    /// Other devices approved for cross-device search.
    pub devices: usize,
    /// Of those, how many may message this device's rooms.
    pub devices_can_message: usize,
    /// Phone access is on, and how many phones are paired.
    pub phone_enabled: bool,
    pub phones: usize,
    /// Which tools have their data on this machine at all.
    pub claude_present: bool,
    pub codex_present: bool,
    /// When a local client last connected without the local key (epoch
    /// seconds): an older `ct-mcp` or CLI is still in use somewhere.
    #[serde(default)]
    pub keyless_client_at: Option<i64>,
    /// Agents get the thread's context when a session starts.
    #[serde(default)]
    pub agent_context: bool,
    /// This computer is reachable on Tailscale, so other devices can pair.
    #[serde(default)]
    pub tailnet: bool,
}

fn home(env: &str, default: &str) -> PathBuf {
    std::env::var_os(env)
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(default))
}

fn contains(path: &Path, needle: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| s.contains(needle))
}

/// Does a tool's hook file run `crossthreads … hook <tool>`?
fn has_hook(path: &Path, tool: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| {
        s.lines()
            .any(|l| l.contains("crossthreads") && l.contains(&format!(" hook {tool}")))
    })
}

/// Check the files the installers write.
pub fn local_checks() -> SetupStatus {
    let claude = home("CLAUDE_CONFIG_DIR", ".claude");
    let codex = home("CODEX_HOME", ".codex");
    // `claude mcp add -s user` writes ~/.claude.json, next to (not inside)
    // ~/.claude, unless CLAUDE_CONFIG_DIR moves it.
    let claude_json = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir).join(".claude.json"),
        None => dirs::home_dir().unwrap_or_default().join(".claude.json"),
    };
    SetupStatus {
        // The installer writes `"<path to crossthreads>" hook <tool> <event>`.
        hooks_claude: has_hook(&claude.join("settings.json"), "claude"),
        hooks_codex: has_hook(&codex.join("hooks.json"), "codex"),
        mcp_claude: contains(&claude_json, "ct-mcp")
            || contains(&claude.join("settings.json"), "ct-mcp"),
        mcp_codex: contains(&codex.join("config.toml"), "ct-mcp")
            || contains(&codex.join("config.toml"), "mcp_servers.crossthreads"),
        skill_claude: claude.join("skills/crossthreads/SKILL.md").is_file(),
        skill_codex: codex.join("prompts/crossthreads.md").is_file(),
        claude_present: claude.is_dir(),
        codex_present: codex.is_dir(),
        model: ct_llm::available(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_installers_write() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join("claude");
        let codex = dir.path().join("codex");
        std::fs::create_dir_all(claude.join("skills/crossthreads")).unwrap();
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"command":"\"/Applications/Crossthreads.app/Contents/MacOS/crossthreads\" hook claude Stop"}]}]}}"#,
        )
        .unwrap();
        std::fs::write(
            claude.join(".claude.json"),
            r#"{"mcpServers":{"crossthreads":{"command":"/x/ct-mcp"}}}"#,
        )
        .unwrap();
        std::fs::write(claude.join("skills/crossthreads/SKILL.md"), "x").unwrap();
        std::fs::write(
            codex.join("config.toml"),
            "[mcp_servers.crossthreads]\ncommand = \"/x/ct-mcp\"\n",
        )
        .unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", &claude);
        std::env::set_var("CODEX_HOME", &codex);
        let s = local_checks();
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::env::remove_var("CODEX_HOME");
        assert!(s.hooks_claude && !s.hooks_codex);
        assert!(s.mcp_claude && s.mcp_codex);
        assert!(s.skill_claude && !s.skill_codex);
        assert!(s.claude_present && s.codex_present);
    }
}
