//! `crossthreads connect` — connect Claude Code and Codex in one step: the
//! hooks (live state, room wake-ups, session-start context), the skill, and
//! the MCP server. Safe to run again: it fixes what's missing or out of date,
//! including an MCP entry that points at an old `ct-mcp`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result};
use serde_json::Value;

const USAGE: &str = "\
crossthreads connect — connect Claude Code and Codex to Crossthreads

USAGE:
    crossthreads connect

Installs the hooks and the skill, and registers the ct-mcp server with each
tool that's on this computer (pointing at the ct-mcp next to this program).
Run it again after moving or updating Crossthreads. Restart the tools after.
";

pub fn run(args: &[String]) -> Result<ExitCode> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    let mut failed = false;

    println!("Hooks:");
    if let Err(e) = crate::hooks::run(&["install".to_string()]) {
        println!("  couldn't install: {e:#}");
        failed = true;
    }
    println!("\nSkill:");
    if let Err(e) = crate::skill::run(&["install".to_string(), "--force".to_string()]) {
        println!("  couldn't install: {e:#}");
        failed = true;
    }

    println!("\nMCP server:");
    let ct_mcp = ct_mcp_path();
    let home = dirs::home_dir().context("no home folder")?;
    let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let codex_dir = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));

    if claude_dir.is_dir() {
        match connect_claude(&home, &ct_mcp) {
            Ok(msg) => println!("  Claude Code: {msg}"),
            Err(e) => {
                println!("  Claude Code: {e:#}");
                failed = true;
            }
        }
    }
    if codex_dir.is_dir() {
        match connect_codex(&codex_dir.join("config.toml"), &ct_mcp) {
            Ok(msg) => println!("  Codex: {msg}"),
            Err(e) => {
                println!("  Codex: {e:#}");
                failed = true;
            }
        }
    }
    if !claude_dir.is_dir() && !codex_dir.is_dir() {
        println!("  neither Claude Code nor Codex is set up on this computer yet");
    }
    println!("\nRestart Claude Code and Codex to pick this up.");
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// The `ct-mcp` beside this program (the app bundle or install folder), else
/// the bare name.
fn ct_mcp_path() -> String {
    let name = if cfg!(windows) {
        "ct-mcp.exe"
    } else {
        "ct-mcp"
    };
    std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string())
}

/// The `claude` command: on PATH, or where its installers put it (an app
/// opened from Finder has a minimal PATH).
fn claude_bin(home: &Path) -> Option<PathBuf> {
    let on_path = Command::new("claude")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if on_path {
        return Some(PathBuf::from("claude"));
    }
    [
        home.join(".claude/local/claude"),
        home.join(".local/bin/claude"),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// What Claude Code has registered as the `crossthreads` MCP server, if any.
fn claude_registered(home: &Path) -> Option<String> {
    let path = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(|d| PathBuf::from(d).join(".claude.json"))
        .unwrap_or_else(|| home.join(".claude.json"));
    let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    v.get("mcpServers")?
        .get("crossthreads")?
        .get("command")?
        .as_str()
        .map(str::to_string)
}

fn connect_claude(home: &Path, ct_mcp: &str) -> Result<String> {
    let current = claude_registered(home);
    if current.as_deref() == Some(ct_mcp) {
        return Ok("already connected".into());
    }
    let Some(claude) = claude_bin(home) else {
        anyhow::bail!(
            "couldn't find the `claude` command. Run this yourself:\n    \
             claude mcp add crossthreads -s user -- \"{ct_mcp}\""
        );
    };
    if current.is_some() {
        // An entry pointing elsewhere (an older ct-mcp): replace it.
        let _ = Command::new(&claude)
            .args(["mcp", "remove", "crossthreads", "-s", "user"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let out = Command::new(&claude)
        .args(["mcp", "add", "crossthreads", "-s", "user", "--", ct_mcp])
        .stdin(Stdio::null())
        .output()
        .context("running claude mcp add")?;
    if !out.status.success() {
        anyhow::bail!(
            "`claude mcp add` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(if current.is_some() {
        format!("updated to {ct_mcp}")
    } else {
        format!("connected ({ct_mcp})")
    })
}

/// Add or update `[mcp_servers.crossthreads]` in Codex's config.toml,
/// leaving everything else as it is.
fn connect_codex(config: &Path, ct_mcp: &str) -> Result<String> {
    let text = std::fs::read_to_string(config).unwrap_or_default();
    let (next, what) = codex_config_with(&text, ct_mcp);
    if next == text {
        return Ok("already connected".into());
    }
    if !text.is_empty() {
        let backup = config.with_extension("toml.bak-crossthreads");
        if !backup.exists() {
            std::fs::copy(config, &backup).context("backing up config.toml")?;
        }
    }
    std::fs::write(config, next).context("writing config.toml")?;
    Ok(what.to_string())
}

/// `text` with the crossthreads MCP server pointing at `ct_mcp`.
fn codex_config_with(text: &str, ct_mcp: &str) -> (String, &'static str) {
    // A TOML literal string: no escaping, so Windows paths stay as they are.
    let command = format!("command = '{}'", ct_mcp.replace('\'', ""));
    let mut out = Vec::new();
    let mut in_section = false;
    let mut found = false;
    let mut replaced = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t == "[mcp_servers.crossthreads]";
            found |= in_section;
        } else if in_section && t.starts_with("command") && t.contains('=') {
            if t != command {
                replaced = true;
            }
            out.push(command.clone());
            continue;
        }
        out.push(line.to_string());
    }
    if found {
        let mut s = out.join("\n");
        if text.ends_with('\n') {
            s.push('\n');
        }
        return (
            s,
            if replaced {
                "updated"
            } else {
                "already connected"
            },
        );
    }
    let mut s = text.to_string();
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    if !s.is_empty() {
        s.push('\n');
    }
    s.push_str("[mcp_servers.crossthreads]\n");
    s.push_str(&command);
    s.push('\n');
    (s, "connected")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_updates_and_leaves_codex_config_alone() {
        let (s, what) = codex_config_with("model = \"o4\"\n", "/A/ct-mcp");
        assert_eq!(what, "connected");
        assert_eq!(
            s,
            "model = \"o4\"\n\n[mcp_servers.crossthreads]\ncommand = '/A/ct-mcp'\n"
        );
        let (same, what) = codex_config_with(&s, "/A/ct-mcp");
        assert_eq!(same, s);
        assert_eq!(what, "already connected");
        let (moved, what) = codex_config_with(&s, "/B/ct-mcp");
        assert_eq!(what, "updated");
        assert!(moved.contains("command = '/B/ct-mcp'"));
        assert!(moved.starts_with("model = \"o4\"\n"));
        let other = "[mcp_servers.other]\ncommand = 'x'\n";
        let (s, _) = codex_config_with(other, "/A/ct-mcp");
        assert!(s.starts_with(other));
        assert!(s.contains("command = 'x'"));
    }
}
