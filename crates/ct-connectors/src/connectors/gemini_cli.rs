//! Gemini CLI connector.
//!
//! Google's `gemini` CLI records each session under a per-project temp dir:
//!
//! ```text
//! ~/.gemini/tmp/<projectId>/chats/session-<ts>-<id>.jsonl   (current)
//! ~/.gemini/tmp/<projectId>/chats/session-<ts>-<id>.json    (older builds)
//! ```
//!
//! `<projectId>` is a sha256 hash (older) or a registry slug (current) of the
//! project root — we glob it rather than recompute. The newer `.jsonl` form is
//! a metadata line followed by message lines plus delta records
//! (`{"$set":…}`, `{"$rewindTo":<id>}`); the older `.json` form is a single
//! `{sessionId, startTime, messages:[…]}` object. In both, a message is
//! `{"id","timestamp","type":"user"|"gemini"|…,"content": <PartListUnion>}` —
//! note the role lives in `type` with the value `gemini` (not `assistant`), and
//! the text lives in `content` (a string, a `Part`, or a `Part[]`).
//!
//! `GEMINI_DIR_OVERRIDE` (a path) replaces `~/.gemini` for tests.

use std::path::{Path, PathBuf};

use ct_core::connector::{Connector, ConnectorError, DiscoveredSession, Result};
use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Kind, Message, Role, Source, Tool};

use crate::text::extract_code_snippets;

pub const FINGERPRINT: &str = "gemini-cli/v1";

#[derive(Debug, Default)]
pub struct GeminiCliConnector;

impl GeminiCliConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Connector for GeminiCliConnector {
    fn tool(&self) -> Tool {
        Tool::GeminiCli
    }

    fn fingerprint(&self) -> &'static str {
        FINGERPRINT
    }

    fn roots(&self) -> Vec<PathBuf> {
        gemini_dir()
            .map(|d| d.join("tmp"))
            .filter(|p| p.exists())
            .into_iter()
            .collect()
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>> {
        let mut out = Vec::new();
        for tmp in self.roots() {
            let Ok(projects) = std::fs::read_dir(&tmp) else {
                continue;
            };
            for project in projects.flatten() {
                let chats = project.path().join("chats");
                let Ok(files) = std::fs::read_dir(&chats) else {
                    continue;
                };
                for f in files.flatten() {
                    let path = f.path();
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if name.starts_with("session-") && (ext == "json" || ext == "jsonl") {
                        out.push(DiscoveredSession::file(path));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out.dedup();
        Ok(out)
    }

    fn parse(&self, session: &DiscoveredSession) -> Result<Conversation> {
        let text = std::fs::read_to_string(&session.path).map_err(|source| ConnectorError::Io {
            path: session.path.clone(),
            source,
        })?;
        parse_session(&session.path, &text)
    }
}

fn gemini_dir() -> Option<PathBuf> {
    if let Some(over) = std::env::var_os("GEMINI_DIR_OVERRIDE") {
        return Some(PathBuf::from(over));
    }
    dirs::home_dir().map(|h| h.join(".gemini"))
}

/// Parse one session file (`.json` object or `.jsonl` log) into a conversation.
pub fn parse_session(path: &Path, text: &str) -> Result<Conversation> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let (started_at, records) = if ext == "jsonl" {
        collect_jsonl(text)
    } else {
        collect_json(path, text)?
    };

    let mut messages = Vec::new();
    for (idx, rec) in records.iter().enumerate() {
        let role = match rec.get("type").and_then(|v| v.as_str()) {
            Some("user") => Role::User,
            Some("gemini") | Some("assistant") | Some("model") => Role::Assistant,
            // info / error / warning / tool UI rows aren't conversation text.
            _ => continue,
        };
        let content = flatten_parts(rec.get("content"));
        if content.trim().is_empty() {
            continue;
        }
        messages.push(Message {
            id: format!("ms_{idx}"),
            role,
            code_snippets: extract_code_snippets(&content),
            content,
            timestamp: None,
            tool_calls: vec![],
            metadata: serde_json::Value::Null,
        });
    }

    if messages.is_empty() {
        return Err(ConnectorError::Parse {
            path: path.to_path_buf(),
            reason: "no user/model messages in session".into(),
        });
    }

    let content_hash = conversation_hash(&Tool::GeminiCli, &messages);
    Ok(Conversation {
        id: conversation_id(&content_hash),
        tool: Tool::GeminiCli,
        kind: Kind::Thread,
        title: None,
        project: None,
        model: None,
        started_at,
        ended_at: started_at,
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

type Records = (
    Option<chrono::DateTime<chrono::Utc>>,
    Vec<serde_json::Value>,
);

/// Older single-object form: `{startTime, messages:[…]}`.
fn collect_json(path: &Path, text: &str) -> Result<Records> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ConnectorError::Parse {
            path: path.to_path_buf(),
            reason: format!("session json: {e}"),
        })?;
    let started = parse_ts(value.get("startTime"));
    let records = value
        .get("messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok((started, records))
}

/// Newer JSONL log: first line is metadata; following lines are messages plus
/// `$set` (metadata mutation) and `$rewindTo` (truncate history) deltas.
fn collect_jsonl(text: &str) -> Records {
    let mut started = None;
    let mut messages: Vec<serde_json::Value> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if i == 0 {
            started = parse_ts(value.get("startTime"));
            if let Some(arr) = value.get("messages").and_then(|v| v.as_array()) {
                messages.extend(arr.iter().cloned());
            }
            continue;
        }
        if let Some(set) = value.get("$set") {
            if started.is_none() {
                started = parse_ts(set.get("startTime"));
            }
            continue;
        }
        if let Some(rewind) = value.get("$rewindTo").and_then(|v| v.as_str()) {
            if let Some(pos) = messages
                .iter()
                .position(|m| m.get("id").and_then(|v| v.as_str()) == Some(rewind))
            {
                messages.truncate(pos + 1);
            }
            continue;
        }
        // A plain message record.
        if value.get("type").is_some() || value.get("content").is_some() {
            messages.push(value);
        }
    }
    (started, messages)
}

fn parse_ts(v: Option<&serde_json::Value>) -> Option<chrono::DateTime<chrono::Utc>> {
    let s = v?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Flatten a `PartListUnion` (`string | Part | Part[]`) into plain text.
pub fn flatten_parts(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter_map(part_text)
            .collect::<Vec<_>>()
            .join("\n\n"),
        Some(obj @ serde_json::Value::Object(_)) => part_text(obj).unwrap_or_default(),
        _ => String::new(),
    }
}

fn part_text(part: &serde_json::Value) -> Option<String> {
    if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
        return Some(t.to_string());
    }
    if let Some(s) = part.as_str() {
        return Some(s.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_json_session() {
        let json = r#"{
            "sessionId":"s1","startTime":"2026-01-02T03:04:05.000Z",
            "messages":[
                {"id":"a","type":"user","content":"why is the build red?"},
                {"id":"b","type":"gemini","content":[{"text":"a flaky lint rule"}]},
                {"id":"c","type":"info","content":"ignored row"}
            ]
        }"#;
        let c = parse_session(Path::new("/p/chats/session-x.json"), json).unwrap();
        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.messages[1].role, Role::Assistant);
        assert!(c.started_at.is_some());
    }

    #[test]
    fn jsonl_applies_rewind() {
        let jsonl = concat!(
            r#"{"sessionId":"s","startTime":"2026-01-02T03:04:05.000Z","messages":[]}"#,
            "\n",
            r#"{"id":"m1","type":"user","content":"first"}"#,
            "\n",
            r#"{"id":"m2","type":"gemini","content":"second"}"#,
            "\n",
            r#"{"id":"m3","type":"user","content":"third"}"#,
            "\n",
            r#"{"$rewindTo":"m2"}"#,
            "\n",
        );
        let c = parse_session(Path::new("/p/chats/session-x.jsonl"), jsonl).unwrap();
        // Rewind to m2 drops m3.
        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.messages[0].content, "first");
        assert_eq!(c.messages[1].content, "second");
    }
}
