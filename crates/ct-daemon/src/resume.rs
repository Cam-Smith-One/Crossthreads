//! "Pick up where you left off." Given an indexed conversation's tool and source
//! transcript, produce the best native way to resume it: a runnable **resume
//! command** for tools that expose one (Claude Code today), and a
//! **reveal-the-source** fallback for everything else. Pure and deterministic —
//! the daemon op, the `crossthreads resume` CLI, and the UI all render this.
//!
//! This is deliberately conservative: a tool only gets a resume *command* when we
//! know the exact, correct invocation. Everything else reveals the transcript
//! (always correct) rather than emit a command that might not work.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// What to do to get back into a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// A shell command that resumes the session in its native tool. `display` is
    /// the copy-paste form (with `cd` when the project dir matters);
    /// `program`/`args`/`cwd` let a GUI spawn it directly without a shell.
    Resume {
        program: String,
        args: Vec<String>,
        cwd: Option<String>,
        display: String,
    },
    /// No native resume for this tool — reveal the source transcript instead.
    Reveal { path: String },
}

/// A resume plan for one conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resume {
    pub tool: String,
    pub action: Action,
    /// Always present: the source transcript path, for "open original".
    pub source_path: String,
}

/// The transcript's file stem (e.g. a Claude Code session UUID).
fn file_stem(path: &str) -> Option<String> {
    let stem = Path::new(path).file_stem()?.to_str()?.trim();
    (!stem.is_empty()).then(|| stem.to_string())
}

/// Extract an embedded canonical UUID (`8-4-4-4-12` hex) from a string, if
/// present — Codex names its rollout files `rollout-<timestamp>-<uuid>.jsonl`,
/// so the resumable id is the embedded UUID, not the whole stem.
fn embedded_uuid(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let is_hex = |b: u8| b.is_ascii_hexdigit();
    // UUID is 36 chars with dashes at fixed offsets 8,13,18,23.
    for start in 0..bytes.len().saturating_sub(35) {
        let w = &bytes[start..start + 36];
        let shape_ok = w.iter().enumerate().all(|(i, &b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => is_hex(b),
        });
        if shape_ok {
            return Some(s[start..start + 36].to_lowercase());
        }
    }
    None
}

/// Single-quote a string for a POSIX shell display (wrap only when needed).
fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Build the resume plan for a conversation from its `tool`, `source_path`, and
/// (optional) `project` working directory.
pub fn resume_for(tool: &str, source_path: &str, project: Option<&str>) -> Resume {
    // (program, session id) for tools with a known resume CLI.
    let command = match tool {
        // Claude Code: `claude --resume <session-id>`; the id is the file stem.
        "claude-code" => file_stem(source_path).map(|id| ("claude", id)),
        // Codex: `codex resume <uuid>`; the id is the UUID embedded in the
        // `rollout-<ts>-<uuid>.jsonl` filename (not the whole stem).
        "codex" => embedded_uuid(source_path).map(|id| ("codex", id)),
        _ => None,
    };

    let action = match command {
        Some((program, id)) => {
            // Run from the project dir so the resumed session lands in the right
            // workspace; the display command is copy-paste complete.
            let subcmd = if program == "claude" {
                format!("{program} --resume {id}")
            } else {
                format!("{program} resume {id}")
            };
            let display = match project {
                Some(p) if !p.is_empty() => format!("cd {} && {subcmd}", shell_quote(p)),
                _ => subcmd,
            };
            let args = if program == "claude" {
                vec!["--resume".to_string(), id]
            } else {
                vec!["resume".to_string(), id]
            };
            Action::Resume {
                program: program.to_string(),
                args,
                cwd: project.filter(|p| !p.is_empty()).map(str::to_string),
                display,
            }
        }
        // Everything else (Cursor, Copilot, IDE stores, …): no resume CLI we can
        // emit with confidence — reveal the transcript instead.
        None => Action::Reveal {
            path: source_path.to_string(),
        },
    };

    Resume {
        tool: tool.to_string(),
        action,
        source_path: source_path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_resumes_with_session_id_and_cwd() {
        let r = resume_for(
            "claude-code",
            "/home/me/.claude/projects/-home-me-app/abc-123-def.jsonl",
            Some("/home/me/app"),
        );
        assert_eq!(r.tool, "claude-code");
        match r.action {
            Action::Resume {
                program,
                args,
                cwd,
                display,
            } => {
                assert_eq!(program, "claude");
                assert_eq!(args, vec!["--resume", "abc-123-def"]);
                assert_eq!(cwd.as_deref(), Some("/home/me/app"));
                assert_eq!(display, "cd /home/me/app && claude --resume abc-123-def");
            }
            other => panic!("expected Resume, got {other:?}"),
        }
    }

    #[test]
    fn claude_code_without_project_omits_cd() {
        let r = resume_for("claude-code", "/x/abc.jsonl", None);
        match r.action {
            Action::Resume { display, cwd, .. } => {
                assert_eq!(display, "claude --resume abc");
                assert_eq!(cwd, None);
            }
            other => panic!("expected Resume, got {other:?}"),
        }
    }

    #[test]
    fn project_with_spaces_is_quoted() {
        let r = resume_for("claude-code", "/x/s.jsonl", Some("/home/me/My Project"));
        match r.action {
            Action::Resume { display, .. } => {
                assert_eq!(display, "cd '/home/me/My Project' && claude --resume s");
            }
            other => panic!("expected Resume, got {other:?}"),
        }
    }

    #[test]
    fn codex_resumes_with_embedded_uuid() {
        // Codex rollout files embed the session UUID in the filename.
        let r = resume_for(
            "codex",
            "/home/me/.codex/sessions/2026/06/25/rollout-2026-06-25T00-55-24-1e2d3c4b-5a6f-7089-9abc-def012345678.jsonl",
            Some("/home/me/app"),
        );
        match r.action {
            Action::Resume {
                program,
                args,
                display,
                ..
            } => {
                assert_eq!(program, "codex");
                assert_eq!(args, vec!["resume", "1e2d3c4b-5a6f-7089-9abc-def012345678"]);
                assert_eq!(
                    display,
                    "cd /home/me/app && codex resume 1e2d3c4b-5a6f-7089-9abc-def012345678"
                );
            }
            other => panic!("expected Resume, got {other:?}"),
        }
    }

    #[test]
    fn codex_without_uuid_reveals() {
        let r = resume_for("codex", "/home/me/.codex/sessions/weird-name.jsonl", None);
        assert!(matches!(r.action, Action::Reveal { .. }));
    }

    #[test]
    fn unknown_tool_reveals_source() {
        let r = resume_for("cursor", "/x/state.vscdb", Some("/proj"));
        assert_eq!(
            r.action,
            Action::Reveal {
                path: "/x/state.vscdb".to_string()
            }
        );
        assert_eq!(r.source_path, "/x/state.vscdb");
    }

    #[test]
    fn claude_code_bad_path_falls_back_to_reveal() {
        // No file stem to derive a session id from → reveal, never a broken command.
        let r = resume_for("claude-code", "/", None);
        assert!(matches!(r.action, Action::Reveal { .. }));
    }
}
