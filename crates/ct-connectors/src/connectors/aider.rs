//! Aider connector.
//!
//! Aider writes a human-readable transcript to `.aider.chat.history.md` in the
//! directory it's run from (the project root). The file accumulates multiple
//! sessions over time, each delimited by a header line:
//!
//! ```text
//! # aider chat started at 2026-05-14 09:12:00
//! ```
//!
//! Within a session:
//! - user messages are lines prefixed with `#### `,
//! - assistant prose is unprefixed text (may contain fenced code),
//! - lines prefixed with `> ` are aider's own notes (commands, token counts,
//!   commit messages) — skipped, except a best-effort `Model:` sniff.
//!
//! There is no central registry of these files, so discovery scans a bounded
//! set of roots (override with `CROSSTHREADS_AIDER_ROOTS`, colon-separated).

use std::path::{Path, PathBuf};

use ct_core::connector::{Connector, ConnectorError, DiscoveredSession, Result};
use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Kind, Message, Role, Source, Tool};

use crate::text::extract_code_snippets;

pub const FINGERPRINT: &str = "aider/v1";
const HISTORY_FILE: &str = ".aider.chat.history.md";
const SESSION_MARKER: &str = "# aider chat started at ";
const MAX_DEPTH: usize = 6;

#[derive(Debug, Default)]
pub struct AiderConnector;

impl AiderConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Connector for AiderConnector {
    fn tool(&self) -> Tool {
        Tool::Aider
    }

    fn fingerprint(&self) -> &'static str {
        FINGERPRINT
    }

    fn roots(&self) -> Vec<PathBuf> {
        default_roots().into_iter().filter(|p| p.exists()).collect()
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>> {
        let mut history_files = Vec::new();
        for root in self.roots() {
            find_history_files(&root, 0, &mut history_files);
        }
        history_files.sort();
        history_files.dedup();

        let mut sessions = Vec::new();
        for file in history_files {
            match discover_in_file(&file) {
                Ok(mut s) => sessions.append(&mut s),
                Err(e) => eprintln!("warn: aider discover failed for {}: {e}", file.display()),
            }
        }
        Ok(sessions)
    }

    fn parse(&self, session: &DiscoveredSession) -> Result<Conversation> {
        let locator = session
            .locator
            .as_deref()
            .ok_or_else(|| ConnectorError::Parse {
                path: session.path.clone(),
                reason: "aider sessions require a locator".into(),
            })?;
        parse_in_file(&session.path, locator)
    }
}

/// One [`DiscoveredSession`] per session block in a history file.
pub fn discover_in_file(path: &Path) -> Result<Vec<DiscoveredSession>> {
    let text = std::fs::read_to_string(path).map_err(|source| ConnectorError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let count = split_sessions(&text).len();
    Ok((0..count)
        .map(|i| DiscoveredSession {
            path: path.to_path_buf(),
            locator: Some(format!("session/{i}")),
        })
        .collect())
}

/// Parse one session (`session/<index>`) out of a history file.
pub fn parse_in_file(path: &Path, locator: &str) -> Result<Conversation> {
    let idx: usize = locator
        .strip_prefix("session/")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| ConnectorError::Parse {
            path: path.to_path_buf(),
            reason: format!("bad aider locator: {locator}"),
        })?;

    let text = std::fs::read_to_string(path).map_err(|source| ConnectorError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    let sessions = split_sessions(&text);
    let session = sessions.get(idx).ok_or_else(|| ConnectorError::Parse {
        path: path.to_path_buf(),
        reason: format!("session {idx} not found"),
    })?;

    let project = path.parent().map(|p| p.to_string_lossy().into_owned());

    build_conversation(session, path, project).ok_or_else(|| ConnectorError::Parse {
        path: path.to_path_buf(),
        reason: "session had no usable messages".into(),
    })
}

/// A parsed session header + its body lines.
pub struct SessionBlock<'a> {
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub body: Vec<&'a str>,
}

/// Split a history file into sessions on the `# aider chat started at` marker.
/// Content before the first marker is ignored; a file with no marker is treated
/// as a single, timestamp-less session.
pub fn split_sessions(text: &str) -> Vec<SessionBlock<'_>> {
    let mut sessions: Vec<SessionBlock> = Vec::new();
    let mut saw_marker = false;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(SESSION_MARKER) {
            saw_marker = true;
            sessions.push(SessionBlock {
                started_at: parse_timestamp(rest.trim()),
                body: Vec::new(),
            });
        } else if let Some(cur) = sessions.last_mut() {
            cur.body.push(line);
        } else if !saw_marker {
            // No marker yet: start an implicit session.
            sessions.push(SessionBlock {
                started_at: None,
                body: vec![line],
            });
            saw_marker = true; // subsequent lines append to it until a real marker
        }
    }

    sessions
}

/// Build a normalized conversation from one session block.
pub fn build_conversation(
    session: &SessionBlock,
    path: &Path,
    project: Option<String>,
) -> Option<Conversation> {
    let mut messages = Vec::new();
    let mut model: Option<String> = None;

    let mut role: Option<Role> = None;
    let mut buf: Vec<String> = Vec::new();

    // Flush the buffer into a message of the pending role.
    let flush = |role: Option<Role>, buf: &mut Vec<String>, messages: &mut Vec<Message>| {
        if let Some(role) = role {
            let content = buf.join("\n");
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                messages.push(make_message(messages.len(), role, trimmed));
            }
        }
        buf.clear();
    };

    for line in &session.body {
        // aider writes user turns as `#### ` (or a bare `####`); require that exact
        // form so assistant Markdown headings (`##### Foo`) aren't misread as user.
        if *line == "####" || line.starts_with("#### ") {
            if role != Some(Role::User) {
                flush(role, &mut buf, &mut messages);
                role = Some(Role::User);
            }
            buf.push(line.strip_prefix("#### ").unwrap_or("").to_string());
        } else if let Some(meta) = line.strip_prefix('>') {
            // aider's own notes. Sniff a model name; otherwise skip.
            if model.is_none() {
                if let Some(m) = meta.split("Model:").nth(1) {
                    model = Some(m.split_whitespace().next().unwrap_or("").to_string())
                        .filter(|s| !s.is_empty());
                }
            }
        } else {
            if role != Some(Role::Assistant) {
                flush(role, &mut buf, &mut messages);
                role = Some(Role::Assistant);
            }
            buf.push((*line).to_string());
        }
    }
    flush(role, &mut buf, &mut messages);

    if messages.is_empty() {
        return None;
    }

    let content_hash = conversation_hash(&Tool::Aider, &messages);
    Some(Conversation {
        id: conversation_id(&content_hash),
        tool: Tool::Aider,
        kind: Kind::Thread,
        title: None,
        project,
        model,
        started_at: session.started_at,
        ended_at: session.started_at,
        git_context: GitContext::default(),
        source: Source {
            path: path.to_string_lossy().into_owned(),
            offset: None,
            fingerprint: FINGERPRINT.into(),
        },
        content_hash,
        messages,
    })
}

fn make_message(idx: usize, role: Role, content: &str) -> Message {
    Message {
        id: format!("ms_{idx}"),
        role,
        content: content.to_string(),
        timestamp: None,
        code_snippets: extract_code_snippets(content),
        tool_calls: vec![],
        metadata: serde_json::Value::Null,
    }
}

fn parse_timestamp(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

fn default_roots() -> Vec<PathBuf> {
    if let Ok(v) = std::env::var("CROSSTHREADS_AIDER_ROOTS") {
        return v
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
    }
    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    if let Some(home) = dirs::home_dir() {
        for name in [
            "code", "Code", "projects", "Projects", "dev", "src", "repos", "work",
        ] {
            roots.push(home.join(name));
        }
    }
    roots
}

/// Directory names never worth descending into.
fn is_ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | ".venv"
            | "venv"
            | "dist"
            | "build"
            | ".cache"
            | "__pycache__"
    )
}

/// Depth-limited search for `.aider.chat.history.md`, skipping noise dirs and
/// not following symlinks (avoids cycles and runaway scans).
fn find_history_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let candidate = dir.join(HISTORY_FILE);
    if candidate.is_file() {
        out.push(candidate);
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() && !ft.is_symlink() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Skip hidden dirs (project history file itself is hidden but is a file).
            if name.starts_with('.') || is_ignored_dir(&name) {
                continue;
            }
            find_history_files(&path, depth + 1, out);
        }
    }
}
