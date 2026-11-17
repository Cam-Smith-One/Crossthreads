//! Grok Build connector (xAI's `grok` terminal coding agent).
//!
//! Grok Build keeps every session in its own folder, grouped by the folder it
//! ran in (URL-encoded):
//!
//! ```text
//! ~/.grok/sessions/<url-encoded cwd>/<session id>/
//!     summary.json        title, model, created_at, updated_at, …
//!     updates.jsonl       the conversation as ACP session updates
//!     chat_history.jsonl  the raw messages sent to the model
//! ```
//!
//! `GROK_HOME` replaces `~/.grok`, as it does for `grok` itself.
//!
//! The formats aren't formally documented, so parsing is forgiving: it reads
//! `chat_history.jsonl` when it has user/assistant messages (`{role, content}`,
//! content a string or a list of `{type:"text", text}` parts), else assembles
//! messages from the `updates.jsonl` stream (`user_message_chunk`,
//! `agent_message_chunk`, `tool_call`), with or without an `update`/`params`
//! wrapper around each update.

use std::path::{Path, PathBuf};

use ct_core::connector::{Connector, ConnectorError, DiscoveredSession, Result};
use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Kind, Message, Role, Source, Tool, ToolCall};
use serde_json::Value;

use crate::text::extract_code_snippets;

pub const FINGERPRINT: &str = "grok-build/v1";
/// The files a session folder may hold its conversation in, preferred first.
const LOGS: &[&str] = &["chat_history.jsonl", "updates.jsonl"];

#[derive(Debug, Default)]
pub struct GrokBuildConnector;

impl GrokBuildConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Connector for GrokBuildConnector {
    fn tool(&self) -> Tool {
        Tool::GrokBuild
    }

    fn fingerprint(&self) -> &'static str {
        FINGERPRINT
    }

    fn roots(&self) -> Vec<PathBuf> {
        grok_home()
            .map(|d| d.join("sessions"))
            .filter(|p| p.is_dir())
            .into_iter()
            .collect()
    }

    /// One session per folder: its preferred log file stands for it.
    fn discover(&self) -> Result<Vec<DiscoveredSession>> {
        let mut out = Vec::new();
        for root in self.roots() {
            let Ok(places) = std::fs::read_dir(&root) else {
                continue;
            };
            for place in places.flatten() {
                let Ok(sessions) = std::fs::read_dir(place.path()) else {
                    continue;
                };
                for s in sessions.flatten() {
                    let dir = s.path();
                    if let Some(log) = LOGS.iter().map(|f| dir.join(f)).find(|p| p.is_file()) {
                        out.push(DiscoveredSession::file(log));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    fn parse(&self, session: &DiscoveredSession) -> Result<Conversation> {
        parse_session_dir(&session.path)
    }
}

fn grok_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("GROK_HOME") {
        return Some(PathBuf::from(home));
    }
    dirs::home_dir().map(|h| h.join(".grok"))
}

/// Parse the session whose log is at `log` (in its session folder).
pub fn parse_session_dir(log: &Path) -> Result<Conversation> {
    let dir = log.parent().unwrap_or(Path::new("."));
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
    let summary: Value = read("summary.json")
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);

    let mut messages = read("chat_history.jsonl")
        .map(|t| from_chat_history(&t))
        .unwrap_or_default();
    if !messages.iter().any(|m| m.role == Role::User) {
        messages = read("updates.jsonl")
            .map(|t| from_updates(&t))
            .unwrap_or_default();
    }
    if messages.is_empty() {
        return Err(ConnectorError::Parse {
            path: log.to_path_buf(),
            reason: "no user/assistant messages in session".into(),
        });
    }
    for (i, m) in messages.iter_mut().enumerate() {
        m.id = format!("gb_{i}");
        m.code_snippets = extract_code_snippets(&m.content);
    }

    let project = dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(percent_decode)
        .filter(|p| p.starts_with('/') || p.contains(":\\") || p.contains(":/"));
    let title = summary
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let started_at =
        ts(summary.get("created_at")).or_else(|| messages.first().and_then(|m| m.timestamp));
    let ended_at = ts(summary.get("updated_at"))
        .or_else(|| messages.iter().rev().find_map(|m| m.timestamp))
        .or(started_at);

    let content_hash = conversation_hash(&Tool::GrokBuild, &messages);
    Ok(Conversation {
        id: conversation_id(&content_hash),
        tool: Tool::GrokBuild,
        kind: Kind::Thread,
        title,
        project,
        model: summary
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        started_at,
        ended_at,
        git_context: GitContext::default(),
        source: Source {
            path: log.to_string_lossy().into_owned(),
            offset: None,
            fingerprint: FINGERPRINT.into(),
        },
        content_hash,
        messages,
    })
}

fn message(
    role: Role,
    content: String,
    timestamp: Option<chrono::DateTime<chrono::Utc>>,
) -> Message {
    Message {
        id: String::new(),
        role,
        content,
        timestamp,
        code_snippets: vec![],
        tool_calls: vec![],
        metadata: Value::Null,
    }
}

/// `chat_history.jsonl`: one `{role, content}` message per line.
fn from_chat_history(text: &str) -> Vec<Message> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let v = v.get("message").unwrap_or(&v);
        let role = match v.get("role").and_then(Value::as_str) {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => continue, // system prompts and tool results aren't the conversation
        };
        let content = flatten(v.get("content"));
        let mut m = message(role, content, ts(v.get("timestamp")));
        if let Some(calls) = v.get("tool_calls").and_then(Value::as_array) {
            for c in calls {
                let f = c.get("function").unwrap_or(c);
                if let Some(name) = f.get("name").and_then(Value::as_str) {
                    m.tool_calls.push(ToolCall {
                        name: name.to_string(),
                        args: f.get("arguments").cloned().unwrap_or(Value::Null),
                        result_ref: None,
                    });
                }
            }
        }
        if !m.content.trim().is_empty() || !m.tool_calls.is_empty() {
            out.push(m);
        }
    }
    out
}

/// `updates.jsonl`: ACP session updates; consecutive chunks of one kind are
/// one message.
fn from_updates(text: &str) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let u = v
            .pointer("/params/update")
            .or_else(|| v.get("update"))
            .unwrap_or(&v);
        let kind = u.get("sessionUpdate").and_then(Value::as_str).unwrap_or("");
        let when = ts(v.get("timestamp")).or_else(|| ts(u.get("timestamp")));
        let role = match kind {
            "user_message_chunk" | "user_message" => Role::User,
            "agent_message_chunk" | "agent_message" => Role::Assistant,
            "tool_call" => {
                let name = u
                    .get("title")
                    .or_else(|| u.get("kind"))
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                let call = ToolCall {
                    name,
                    args: u.get("rawInput").cloned().unwrap_or(Value::Null),
                    result_ref: None,
                };
                match out.last_mut() {
                    Some(m) if m.role == Role::Assistant => m.tool_calls.push(call),
                    _ => {
                        let mut m = message(Role::Assistant, String::new(), when);
                        m.tool_calls.push(call);
                        out.push(m);
                    }
                }
                continue;
            }
            _ => continue, // thoughts, plans, tool updates, mode changes
        };
        let chunk = flatten(u.get("content"));
        match out.last_mut() {
            Some(m) if m.role == role && m.tool_calls.is_empty() => m.content.push_str(&chunk),
            _ => out.push(message(role, chunk, when)),
        }
    }
    out.retain(|m| !m.content.trim().is_empty() || !m.tool_calls.is_empty());
    out
}

/// Text from a string, a `{type:"text", text}` part, or a list of parts.
fn flatten(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                p.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| p.as_str())
                    .map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        Some(Value::Object(o)) => o
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

fn ts(v: Option<&Value>) -> Option<chrono::DateTime<chrono::Utc>> {
    match v? {
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&chrono::Utc)),
        Value::Number(n) => {
            let n = n.as_i64()?;
            // Seconds or milliseconds since the epoch.
            let secs = if n > 100_000_000_000 { n / 1000 } else { n };
            chrono::DateTime::from_timestamp(secs, 0)
        }
        _ => None,
    }
}

/// `%2Fhome%2Fme%2Fapi` → `/home/me/api`. Invalid escapes are kept as is.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|c| (*c as char).to_digit(16)),
                bytes.get(i + 2).and_then(|c| (*c as char).to_digit(16)),
            ) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(dir: &Path, files: &[(&str, &str)]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
        LOGS.iter()
            .map(|f| dir.join(f))
            .find(|p| p.is_file())
            .unwrap()
    }

    #[test]
    fn reads_chat_history_with_summary_and_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sessions/%2Fhome%2Fme%2Fapi/0b7c-1");
        let log = session(
            &dir,
            &[
                (
                    "summary.json",
                    r#"{"title":"Retry the token refresh","model":"grok-code","created_at":"2026-11-10T09:00:00Z","updated_at":"2026-11-10T09:20:00Z"}"#,
                ),
                (
                    "chat_history.jsonl",
                    "{\"role\":\"system\",\"content\":\"You are Grok\"}\n\
                     {\"role\":\"user\",\"content\":\"Add retry with backoff\"}\n\
                     {\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Done:\\n```rust\\nretry()\\n```\"}],\"tool_calls\":[{\"function\":{\"name\":\"edit_file\",\"arguments\":\"{}\"}}]}\n\
                     {\"role\":\"tool\",\"content\":\"ok\"}\n",
                ),
            ],
        );
        let c = parse_session_dir(&log).unwrap();
        assert_eq!(c.tool, Tool::GrokBuild);
        assert_eq!(c.title.as_deref(), Some("Retry the token refresh"));
        assert_eq!(c.project.as_deref(), Some("/home/me/api"));
        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.messages[0].content, "Add retry with backoff");
        assert_eq!(c.messages[1].tool_calls[0].name, "edit_file");
        assert_eq!(c.messages[1].code_snippets.len(), 1);
        assert!(c.started_at.is_some() && c.ended_at > c.started_at);
    }

    #[test]
    fn falls_back_to_the_acp_update_stream() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sessions/%2Fwork%2Fweb/s2");
        let log = session(
            &dir,
            &[(
                "updates.jsonl",
                "{\"update\":{\"sessionUpdate\":\"user_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"Fix the \"}}}\n\
                 {\"update\":{\"sessionUpdate\":\"user_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"login test\"}}}\n\
                 {\"params\":{\"update\":{\"sessionUpdate\":\"agent_thought_chunk\",\"content\":{\"type\":\"text\",\"text\":\"hmm\"}}}}\n\
                 {\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"Mocked the clock.\"}}\n\
                 {\"sessionUpdate\":\"tool_call\",\"title\":\"run_tests\"}\n",
            )],
        );
        let c = parse_session_dir(&log).unwrap();
        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.messages[0].content, "Fix the login test");
        assert_eq!(c.messages[1].content, "Mocked the clock.");
        assert_eq!(c.messages[1].tool_calls[0].name, "run_tests");
        assert_eq!(c.project.as_deref(), Some("/work/web"));
    }

    #[test]
    fn discovers_one_session_per_folder_and_skips_empty_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("sessions/%2Fa");
        session(
            &base.join("one"),
            &[("updates.jsonl", ""), ("chat_history.jsonl", "")],
        );
        std::fs::create_dir_all(base.join("empty")).unwrap();
        std::env::set_var("GROK_HOME", tmp.path());
        let found = GrokBuildConnector::new().discover().unwrap();
        std::env::remove_var("GROK_HOME");
        assert_eq!(found.len(), 1);
        assert!(found[0].path.ends_with("one/chat_history.jsonl"));
        assert!(
            parse_session_dir(&found[0].path).is_err(),
            "no messages, no conversation"
        );
    }

    #[test]
    fn decodes_folder_names() {
        assert_eq!(
            percent_decode("%2FUsers%2Fme%2Fmy%20app"),
            "/Users/me/my app"
        );
        assert_eq!(percent_decode("C%3A%5Cwork"), "C:\\work");
        assert_eq!(percent_decode("bad%zz"), "bad%zz");
    }
}
