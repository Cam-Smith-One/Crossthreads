//! `crossthreads hooks install|uninstall|status` and `crossthreads hook …`.
//!
//! Hooks are how Crossthreads learns what a session is doing *now* (SPEC §6.2).
//! `hooks install` adds a hook to Claude Code and Codex that runs
//! `crossthreads hook <tool>` on session events; that command forwards the
//! event to the local daemon and exits.
//!
//! Two rules keep this safe to leave installed:
//!
//! - **The hook never breaks the agent.** It exits 0 whatever happens,
//!   including when no daemon is running, and prints nothing (some tools feed
//!   a hook's output back to the model) except in the two cases below.
//!
//! For an agent that has joined a room (SPEC §6.4) the same hook does two more
//! things, and these are the only times it prints:
//!
//! - **Auto-wake.** When the agent ends its turn mid-exchange, the Stop hook
//!   waits for the next room message and hands it back, so nobody has to type
//!   "keep going".
//! - **Claim guard.** Before an edit, the PreToolUse hook denies it if another
//!   agent has claimed the file.
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
    for event in target.events {
        // The event is named on the command line, so the hook never depends on
        // how a tool spells it in the payload. Stop may hold an agent that is
        // waiting in a room, so it gets a long timeout; everything else is quick.
        let command = format!("\"{exe}\" hook {} {event}", target.tool);
        let timeout = if *event == "Stop" { 300 } else { 10 };
        hooks
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .with_context(|| format!("`hooks.{event}` is not a list"))?
            .push(json!({
                "hooks": [{ "type": "command", "command": command, "timeout": timeout }]
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

/// The name and room this session last used in a Crossthreads room, read from
/// the tail of its own transcript. That is how a hook knows whether the agent
/// has joined a room at all, without the agent having to register anything.
fn room_identity(transcript: Option<&str>) -> Option<(String, Option<String>)> {
    use std::io::{Seek, SeekFrom};
    // MCP calls look like "me": "claude" (possibly escaped inside a JSON
    // string); shell calls look like --me claude.
    static ME: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static ROOM: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let me_re = ME.get_or_init(|| {
        regex::Regex::new(r#"\\*"me\\*"\s*:\s*\\*"([^"\\]+)|--me[ =]\\*["']?([\w.-]+)"#).unwrap()
    });
    let room_re = ROOM.get_or_init(|| {
        regex::Regex::new(r#"\\*"room\\*"\s*:\s*\\*"([^"\\]+)|--room[ =]\\*["']?([^"'\\\s]+)"#)
            .unwrap()
    });

    let mut file = std::fs::File::open(transcript?).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(2_000_000)))
        .ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let tail = String::from_utf8_lossy(&bytes);

    let mut found: Option<(String, Option<String>)> = None;
    let pick = |c: regex::Captures<'_>| c.get(1).or(c.get(2)).map(|m| m.as_str().to_string());
    for line in tail.lines() {
        let ours = line.contains("crossthreads_room")
            || line.contains("crossthreads_claim")
            || line.contains("crossthreads_release")
            || line.contains("crossthreads room ");
        if !ours {
            continue;
        }
        if let Some(me) = me_re.captures(line).and_then(pick) {
            found = Some((me, room_re.captures(line).and_then(pick)));
        }
    }
    found
}

/// The absolute paths a tool call is about to edit: Claude Code's edit tools
/// name one file; Codex's `apply_patch` names files inside the patch text.
fn edited_paths(payload: &Value) -> Vec<String> {
    static PATCH: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let patch_re = PATCH.get_or_init(|| {
        regex::Regex::new(r"\*\*\* (?:Update|Add|Delete) File: ([^\n\\]+)").unwrap()
    });
    let Some(input) = payload.get("tool_input") else {
        return Vec::new();
    };
    let mut paths: Vec<String> = ["file_path", "notebook_path", "path"]
        .iter()
        .filter_map(|k| input.get(k).and_then(Value::as_str).map(str::to_string))
        .collect();
    // In the serialised input a newline in the patch is the two characters
    // `\n`, which the pattern stops at.
    let raw = input.to_string();
    paths.extend(
        patch_re
            .captures_iter(&raw)
            .map(|c| c[1].trim().to_string()),
    );

    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    paths
        .into_iter()
        .filter(|p| !p.is_empty())
        .map(|p| cwd.join(p).to_string_lossy().into_owned())
        .collect()
}

/// What a hook should print, if anything.
fn hook_output(tool: &str, event: &str, payload: &Value, client: &Client) -> Option<Value> {
    let transcript = payload.get("transcript_path").and_then(Value::as_str);
    let forward = |name: &str| {
        if let Some(mut ev) = event_from(tool, Some(name), payload) {
            ev.event = name.to_string();
            let _ = client.call_timeout(&Request::HookEvent { event: ev }, HOOK_TIMEOUT);
        }
    };

    if event.eq_ignore_ascii_case("PreToolUse") {
        forward(event);
        let paths = edited_paths(payload);
        if paths.is_empty() {
            return None;
        }
        let Ok(ct_daemon::Response::RoomGuard { claims }) =
            client.call_timeout(&Request::RoomGuard { paths }, Duration::from_secs(2))
        else {
            return None;
        };
        // Only now, with a claim in the way, is it worth reading the
        // transcript to learn who we are: an agent may edit what it claimed.
        let me = room_identity(transcript).map(|(me, _)| me.to_lowercase());
        let clash = claims.iter().find(|c| Some(&c.holder) != me.as_ref())?;
        let until = chrono::TimeZone::timestamp_opt(&chrono::Local, clash.expires, 0)
            .single()
            .map(|t| t.format("%H:%M").to_string())
            .unwrap_or_default();
        return Some(json!({ "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": format!(
                "{} is claimed by {} in a Crossthreads room until {until}. Ask {} to release it with crossthreads_room_send, or work on something else.",
                clash.path, clash.holder, clash.holder
            ),
        }}));
    }

    if event.eq_ignore_ascii_case("Stop") {
        let Some((me, room)) = room_identity(transcript) else {
            forward(event);
            return None; // this session never joined a room
        };
        let wait: u64 = std::env::var("CROSSTHREADS_HOOK_WAIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(240);
        let room = room
            .or_else(|| {
                payload
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        forward("RoomListen");
        let answer = client.call_timeout(
            &Request::RoomStop {
                me: me.clone(),
                room: room.clone(),
                wait,
                transcript_path: transcript.map(str::to_string),
            },
            Duration::from_secs(wait + 15),
        );
        if let Ok(ct_daemon::Response::RoomStop {
            room: Some(label),
            messages: Some(messages),
        }) = answer
        {
            forward("RoomWake");
            return Some(json!({ "decision": "block", "reason": format!(
                "Crossthreads: new messages for you ({me}) in room '{label}':\n{messages}\n\
                 Handle them, then reply with crossthreads_room_send(me=\"{me}\", room=\"{room}\", ...)."
            )}));
        }
        forward(event);
        return None;
    }

    forward(event);
    None
}

/// `crossthreads hook <tool> [event]` — called by the tools, not by people.
/// Reads the hook payload on stdin and forwards it to the daemon. Prints only
/// when it has a room message to hand back or an edit to deny; never fails.
pub fn run_hook(args: &[String]) -> ExitCode {
    let Some(tool) = args.first() else {
        return ExitCode::SUCCESS;
    };
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let event = args.get(1).cloned().or_else(|| {
        payload
            .get("hook_event_name")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    if let Some(event) = event {
        if let Some(out) = hook_output(tool, &event, &payload, &Client::from_env()) {
            println!("{out}");
        }
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
            "\"/opt/ct/crossthreads\" hook claude-code Stop"
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
    fn finds_the_room_identity_in_mcp_and_shell_calls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let write = |lines: &[&str]| std::fs::write(&path, lines.join("\n")).unwrap();
        let id = || room_identity(path.to_str());

        // Never used a room tool: no identity, however the words appear.
        write(&[r#"{"type":"user","message":"me and my room"}"#]);
        assert_eq!(id(), None);

        // An MCP tool call, as Claude Code records it.
        write(&[
            r#"{"type":"tool_use","name":"mcp__crossthreads__crossthreads_room_send","input":{"me":"Claude","room":"/code/demo","text":"hi"}}"#,
        ]);
        assert_eq!(id(), Some(("Claude".into(), Some("/code/demo".into()))));

        // The same call nested inside an escaped JSON string (Codex rollouts).
        write(&[
            r#"{"payload":{"arguments":"{\"me\":\"codex\",\"room\":\"/code/demo\"}","name":"crossthreads_room_wait"}}"#,
        ]);
        assert_eq!(id(), Some(("codex".into(), Some("/code/demo".into()))));

        // The shell fallback; the latest call wins, and a call without a room is fine.
        write(&[
            r#"{"input":{"command":"crossthreads room send --me old --room \"/tmp/x\" --to mia hi"}}"#,
            r#"{"input":{"command":"crossthreads room wait --me newer"}}"#,
        ]);
        assert_eq!(id(), Some(("newer".into(), None)));
        assert_eq!(room_identity(Some("/no/such/file")), None);
        assert_eq!(room_identity(None), None);
    }

    #[test]
    fn collects_the_files_an_edit_or_a_patch_touches() {
        let edit = json!({"cwd": "/code/demo", "tool_name": "Edit",
            "tool_input": {"file_path": "src/app.py"}});
        assert_eq!(edited_paths(&edit), ["/code/demo/src/app.py"]);
        let absolute = json!({"cwd": "/code/demo", "tool_input": {"file_path": "/etc/hosts"}});
        assert_eq!(edited_paths(&absolute), ["/etc/hosts"]);
        let patch = json!({"cwd": "/code/demo", "tool_name": "apply_patch", "tool_input": {
            "command": ["apply_patch", "*** Begin Patch\n*** Update File: src/x.py\n@@\n-a\n+b\n*** Add File: docs/n.md\n+hi\n*** End Patch"]}});
        assert_eq!(
            edited_paths(&patch),
            ["/code/demo/src/x.py", "/code/demo/docs/n.md"]
        );
        // A tool that edits nothing (Bash, Read) yields nothing to guard.
        assert!(edited_paths(&json!({"tool_input": {"command": "ls"}})).is_empty());
        assert!(edited_paths(&json!({})).is_empty());
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
