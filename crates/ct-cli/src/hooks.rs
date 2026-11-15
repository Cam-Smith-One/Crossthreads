//! `crossthreads hooks install|uninstall|status` and `crossthreads hook …`.
//!
//! Hooks are how Crossthreads learns what a session is doing *now* (SPEC §6.2).
//! `hooks install` adds a hook to Claude Code and Codex that runs
//! `crossthreads hook <tool>` on session events; that command forwards the
//! event to the local daemon and exits.
//!
//! Two rules keep this safe to leave installed:
//!
//! - **The hook never breaks the agent.** It prints nothing (some tools feed a
//!   hook's output back to the model), waits a fraction of a second at most,
//!   and exits 0 whatever happens, including when no daemon is running.
//! - **The installer only touches its own entries.** Other hooks are kept, key
//!   order is preserved, and the file is backed up before the first edit.
//!
//! - Claude Code: `<~/.claude>/settings.json` (`CLAUDE_CONFIG_DIR` overrides).
//! - Codex: `<~/.codex>/hooks.json` (`CODEX_HOME` overrides).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ct_daemon::live::HookEvent;
use ct_daemon::{Client, Request};
use serde_json::{json, Map, Value};

const USAGE: &str = "\
crossthreads hooks — report live session state to Crossthreads

USAGE:
    crossthreads hooks install [--claude] [--codex]
    crossthreads hooks uninstall [--claude] [--codex]
    crossthreads hooks status

Adds a hook to Claude Code and Codex so the board can show which sessions are
running and which are waiting on you. By default it covers both tools that are
present. Your other hooks are kept, and each file is backed up once
(<file>.crossthreads.bak) before it is first changed.

Restart the tools afterwards; Codex asks you to trust new hooks once.
";

/// How long the hook waits for the daemon before giving up.
const HOOK_TIMEOUT: Duration = Duration::from_millis(400);

struct Target {
    /// Tool slug passed to `crossthreads hook`.
    tool: &'static str,
    label: &'static str,
    /// The tool's config directory; hooks are only installed if it exists.
    home: PathBuf,
    /// Hooks file inside `home`.
    file: &'static str,
    /// Events to hook. Only events the tool is known to support: an unknown
    /// event name could invalidate the whole file.
    events: &'static [&'static str],
}

fn home_dir(env: &str, default: &str) -> PathBuf {
    std::env::var_os(env)
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(default))
}

fn targets() -> Vec<Target> {
    vec![
        Target {
            tool: "claude-code",
            label: "Claude Code",
            home: home_dir("CLAUDE_CONFIG_DIR", ".claude"),
            file: "settings.json",
            events: &[
                "SessionStart",
                "UserPromptSubmit",
                "PreToolUse",
                "PostToolUse",
                "Notification",
                "Stop",
                "SessionEnd",
            ],
        },
        Target {
            tool: "codex",
            label: "Codex",
            home: home_dir("CODEX_HOME", ".codex"),
            file: "hooks.json",
            // The two events Codex hooks are known to deliver. Enough to tell
            // "working" from "turn ended"; not enough to see a permission prompt.
            events: &["PreToolUse", "Stop"],
        },
    ]
}

/// Is this hook entry one of ours?
fn is_ours(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains("crossthreads") && c.contains(" hook "))
}

/// Remove our entries from a settings object, dropping groups and event lists
/// that become empty. Everything else is left exactly as it was.
fn strip(cfg: &mut Map<String, Value>) {
    let Some(hooks) = cfg.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                list.retain(|h| !is_ours(h));
            }
        }
        groups.retain(|g| {
            g.get("hooks")
                .and_then(Value::as_array)
                .map_or(true, |l| !l.is_empty())
        });
    }
    hooks.retain(|_, groups| groups.as_array().map_or(true, |g| !g.is_empty()));
    if hooks.is_empty() {
        cfg.remove("hooks");
    }
}

/// Add our hook for each of the target's events.
fn add(cfg: &mut Map<String, Value>, target: &Target, exe: &str) -> Result<()> {
    let hooks = cfg
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("`hooks` is not an object")?;
    let command = format!("\"{exe}\" hook {}", target.tool);
    for event in target.events {
        hooks
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .with_context(|| format!("`hooks.{event}` is not a list"))?
            .push(json!({
                "hooks": [{ "type": "command", "command": command, "timeout": 5 }]
            }));
    }
    Ok(())
}

fn read(path: &Path) -> Result<Map<String, Value>> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => bail!("{} is not a JSON object; left untouched", path.display()),
        Err(e) => bail!("{} is not valid JSON ({e}); left untouched", path.display()),
    }
}

fn write(path: &Path, cfg: &Map<String, Value>) -> Result<()> {
    // Keep one copy of the file as it was before we ever touched it.
    let backup = PathBuf::from(format!("{}.crossthreads.bak", path.display()));
    if path.exists() && !backup.exists() {
        std::fs::copy(path, &backup).with_context(|| format!("backing up {}", path.display()))?;
    }
    let mut text = serde_json::to_string_pretty(cfg)?;
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

fn installed(path: &Path) -> bool {
    read(path).is_ok_and(|cfg| {
        cfg.get("hooks")
            .and_then(Value::as_object)
            .is_some_and(|hooks| {
                hooks
                    .values()
                    .filter_map(Value::as_array)
                    .flatten()
                    .filter_map(|g| g.get("hooks").and_then(Value::as_array))
                    .flatten()
                    .any(is_ours)
            })
    })
}

pub fn run(args: &[String]) -> Result<ExitCode> {
    let (cmd, rest) = match args.split_first() {
        Some((c, rest)) => (c.as_str(), rest),
        None => ("help", &[][..]),
    };
    let mut only: Vec<&str> = Vec::new();
    for a in rest {
        match a.as_str() {
            "--claude" => only.push("claude-code"),
            "--codex" => only.push("codex"),
            other => bail!("unknown option for `hooks`: {other}"),
        }
    }
    let chosen: Vec<Target> = targets()
        .into_iter()
        .filter(|t| only.is_empty() || only.contains(&t.tool))
        .collect();

    match cmd {
        "install" => {
            let exe = std::env::current_exe().context("locating the crossthreads binary")?;
            let exe = exe.to_string_lossy();
            let mut any = false;
            for t in &chosen {
                if !t.home.is_dir() {
                    println!("- {}: not found ({}), skipped", t.label, t.home.display());
                    continue;
                }
                let path = t.home.join(t.file);
                let mut cfg = read(&path)?;
                strip(&mut cfg);
                add(&mut cfg, t, &exe)?;
                write(&path, &cfg)?;
                println!("✓ {}: hooks installed in {}", t.label, path.display());
                any = true;
            }
            if any {
                println!(
                    "\nRestart the tools so new sessions pick the hooks up. Codex asks you to \
                     trust them once.\nKeep the daemon running (`crossthreads-up`); without it \
                     the hooks do nothing."
                );
            }
        }
        "uninstall" => {
            for t in &chosen {
                let path = t.home.join(t.file);
                if !installed(&path) {
                    println!("- {}: no Crossthreads hooks", t.label);
                    continue;
                }
                let mut cfg = read(&path)?;
                strip(&mut cfg);
                write(&path, &cfg)?;
                println!("✓ {}: hooks removed from {}", t.label, path.display());
            }
        }
        "status" => {
            for t in &chosen {
                let state = if !t.home.is_dir() {
                    "tool not found"
                } else if installed(&t.home.join(t.file)) {
                    "hooks installed"
                } else {
                    "hooks not installed"
                };
                println!("{}: {state}", t.label);
            }
            let up = Client::from_env()
                .call_timeout(&Request::Ping, HOOK_TIMEOUT)
                .is_ok();
            println!(
                "Daemon: {}",
                if up {
                    "running"
                } else {
                    "not reachable (hooks will do nothing)"
                }
            );
        }
        "help" | "--help" | "-h" => print!("{USAGE}"),
        other => {
            eprintln!("unknown hooks command: {other}\n");
            print!("{USAGE}");
            return Ok(ExitCode::FAILURE);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Turn a tool's hook payload into the event we forward. `event` on the command
/// line wins over the payload's own `hook_event_name`.
fn event_from(tool: &str, event: Option<&str>, payload: &Value) -> Option<HookEvent> {
    let text = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    Some(HookEvent {
        tool: tool.to_string(),
        event: event
            .map(str::to_string)
            .or_else(|| text("hook_event_name"))?,
        transcript_path: text("transcript_path"),
        session_id: text("session_id"),
        cwd: text("cwd"),
        message: text("message"),
    })
}

/// `crossthreads hook <tool> [event]` — called by the tools, not by people.
/// Reads the hook payload on stdin and forwards it to the daemon. Never prints,
/// never fails.
pub fn run_hook(args: &[String]) -> ExitCode {
    let Some(tool) = args.first() else {
        return ExitCode::SUCCESS;
    };
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    if let Some(event) = event_from(tool, args.get(1).map(String::as_str), &payload) {
        let _ = Client::from_env().call_timeout(&Request::HookEvent { event }, HOOK_TIMEOUT);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Target {
        Target {
            tool: "claude-code",
            label: "Claude Code",
            home: PathBuf::new(),
            file: "settings.json",
            events: &["Stop", "PreToolUse"],
        }
    }

    #[test]
    fn install_keeps_other_hooks_settings_and_key_order() {
        let mut cfg: Map<String, Value> = serde_json::from_str(
            r#"{
              "zeta": 1,
              "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }],
                "PreCompact": [{ "hooks": [{ "type": "command", "command": "echo hi" }] }]
              },
              "alpha": { "nested": true }
            }"#,
        )
        .unwrap();
        add(&mut cfg, &target(), "/opt/ct/crossthreads").unwrap();

        // The user's keys keep their order; nothing of theirs is lost.
        assert_eq!(cfg.keys().collect::<Vec<_>>(), ["zeta", "hooks", "alpha"]);
        let stop = cfg["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "say done");
        assert_eq!(
            stop[1]["hooks"][0]["command"],
            "\"/opt/ct/crossthreads\" hook claude-code"
        );
        assert_eq!(cfg["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(cfg["hooks"]["PreCompact"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn reinstall_does_not_duplicate_and_uninstall_restores() {
        let original: Map<String, Value> = serde_json::from_str(
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]},"model":"x"}"#,
        )
        .unwrap();
        let mut cfg = original.clone();
        for exe in ["/old/crossthreads", "/new/crossthreads"] {
            strip(&mut cfg);
            add(&mut cfg, &target(), exe).unwrap();
        }
        assert_eq!(cfg["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert!(cfg["hooks"]["Stop"][1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("/new/"));

        strip(&mut cfg);
        assert_eq!(cfg, original);

        // With nothing else there, the `hooks` key itself goes away.
        let mut only_ours = Map::new();
        add(&mut only_ours, &target(), "/x/crossthreads").unwrap();
        strip(&mut only_ours);
        assert!(only_ours.is_empty());
    }

    #[test]
    fn leaves_a_meat_proxy_hook_alone() {
        let mut cfg: Map<String, Value> = serde_json::from_str(
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"\"/bin/meat-proxy\" hook stop"}]}]}}"#,
        )
        .unwrap();
        let before = cfg.clone();
        strip(&mut cfg);
        assert_eq!(cfg, before);
    }

    #[test]
    fn refuses_to_rewrite_a_file_it_cannot_parse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(read(&path).is_err());
        std::fs::write(&path, "[1, 2]").unwrap();
        assert!(read(&path).is_err());
        assert!(read(&dir.path().join("missing.json")).unwrap().is_empty());
    }

    #[test]
    fn write_backs_up_once_and_reports_installed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{\"model\":\"x\"}\n").unwrap();
        assert!(!installed(&path));

        let mut cfg = read(&path).unwrap();
        add(&mut cfg, &target(), "/x/crossthreads").unwrap();
        write(&path, &cfg).unwrap();
        assert!(installed(&path));
        let backup = dir.path().join("settings.json.crossthreads.bak");
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "{\"model\":\"x\"}\n"
        );

        // A second write must not overwrite the original backup.
        strip(&mut cfg);
        write(&path, &cfg).unwrap();
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "{\"model\":\"x\"}\n"
        );
        assert!(!installed(&path));
    }

    #[test]
    fn reads_the_event_from_the_payload_or_the_command_line() {
        let payload = json!({
            "hook_event_name": "Notification",
            "transcript_path": "/t/s.jsonl",
            "session_id": "abc",
            "cwd": "/work/api",
            "message": "Claude needs your permission to use Bash"
        });
        let ev = event_from("claude-code", None, &payload).unwrap();
        assert_eq!(ev.event, "Notification");
        assert_eq!(ev.transcript_path.as_deref(), Some("/t/s.jsonl"));
        assert!(ev.message.unwrap().contains("permission"));
        assert_eq!(
            event_from("codex", Some("Stop"), &payload).unwrap().event,
            "Stop"
        );
        // Nothing to go on: no event, nothing forwarded.
        assert!(event_from("codex", None, &Value::Null).is_none());
    }
}
