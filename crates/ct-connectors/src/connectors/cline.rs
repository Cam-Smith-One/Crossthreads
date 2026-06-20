//! Cline connector (also covers Roo Code's Cline lineage).
//!
//! Cline is a VS Code extension (`saoudrizwan.claude-dev`) that stores each task
//! under the editor's globalStorage:
//!
//! ```text
//! <config>/<Editor>/User/globalStorage/saoudrizwan.claude-dev/tasks/<taskId>/
//!     api_conversation_history.json   # the raw Anthropic-format messages
//!     ui_messages.json                # UI event log (not parsed here)
//! ```
//!
//! `<taskId>` is the task's creation time in epoch milliseconds, which we use as
//! the conversation timestamp. `api_conversation_history.json` is a JSON array
//! of Anthropic messages: `{"role":"user"|"assistant","content": <string | block[]>}`
//! where each block is `{"type":"text","text":…}`, `{"type":"tool_use","name":…,
//! "input":…}`, or `{"type":"tool_result","content": <string | block[]>}`.
//!
//! Cline installs into VS Code, Cursor, Windsurf, etc., so we look under every
//! VS Code-family editor (see [`crate::connectors::vscode`]).

use std::path::{Path, PathBuf};

use ct_core::connector::{Connector, ConnectorError, DiscoveredSession, Result};
use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Kind, Message, Role, Source, Tool};

use crate::connectors::vscode::family_user_dirs;
use crate::text::extract_code_snippets;

pub const FINGERPRINT: &str = "cline/v1";
const EXT_ID: &str = "saoudrizwan.claude-dev";
const HISTORY_FILE: &str = "api_conversation_history.json";

#[derive(Debug, Default)]
pub struct ClineConnector;

impl ClineConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Connector for ClineConnector {
    fn tool(&self) -> Tool {
        Tool::Cline
    }

    fn fingerprint(&self) -> &'static str {
        FINGERPRINT
    }

    fn roots(&self) -> Vec<PathBuf> {
        task_dirs().into_iter().filter(|p| p.exists()).collect()
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>> {
        let mut out = Vec::new();
        for tasks in self.roots() {
            let Ok(entries) = std::fs::read_dir(&tasks) else {
                continue;
            };
            for entry in entries.flatten() {
                let history = entry.path().join(HISTORY_FILE);
                if history.is_file() {
                    out.push(DiscoveredSession::file(history));
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
        parse_history(&session.path, &text)
    }
}

/// `…/globalStorage/saoudrizwan.claude-dev/tasks` under each editor.
fn task_dirs() -> Vec<PathBuf> {
    family_user_dirs()
        .into_iter()
        .map(|u| u.join("globalStorage").join(EXT_ID).join("tasks"))
        .collect()
}

/// The taskId (epoch-ms dir name) → start time.
fn started_at_from_path(path: &Path) -> Option<chrono::DateTime<chrono::Utc>> {
    let dir = path.parent()?.file_name()?.to_str()?;
    let ms: i64 = dir.parse().ok()?;
    chrono::DateTime::from_timestamp_millis(ms)
}

fn parse_history(path: &Path, text: &str) -> Result<Conversation> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ConnectorError::Parse {
            path: path.to_path_buf(),
            reason: format!("history json: {e}"),
        })?;
    let entries = value.as_array().ok_or_else(|| ConnectorError::Parse {
        path: path.to_path_buf(),
        reason: "history is not a JSON array".into(),
    })?;

    let mut messages = Vec::new();
    for (idx, entry) in entries.iter().enumerate() {
        let role = match entry.get("role").and_then(|v| v.as_str()) {
            Some("assistant") => Role::Assistant,
            Some("system") => Role::System,
            _ => Role::User,
        };
        let content = flatten_content(entry.get("content"));
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
            reason: "no messages in history".into(),
        });
    }

    let started_at = started_at_from_path(path);
    let content_hash = conversation_hash(&Tool::Cline, &messages);
    Ok(Conversation {
        id: conversation_id(&content_hash),
        tool: Tool::Cline,
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

/// Flatten an Anthropic `content` field (string or block array) to plain text.
pub fn flatten_content(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(blocks)) => {
            let mut parts = Vec::new();
            for b in blocks {
                match b.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                            parts.push(t.to_string());
                        }
                    }
                    Some("tool_use") => {
                        let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                        let input = b.get("input").map(|v| v.to_string()).unwrap_or_default();
                        parts.push(format!("[tool: {name}] {input}"));
                    }
                    Some("tool_result") => {
                        let inner = flatten_content(b.get("content"));
                        if !inner.trim().is_empty() {
                            parts.push(inner);
                        }
                    }
                    _ => {
                        if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                            parts.push(t.to_string());
                        }
                    }
                }
            }
            parts.join("\n\n")
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_anthropic_blocks_and_strings() {
        let json = r#"[
            {"role":"user","content":"fix the flaky test"},
            {"role":"assistant","content":[
                {"type":"text","text":"I'll retry with a wait."},
                {"type":"tool_use","name":"bash","input":{"cmd":"pytest"}}
            ]},
            {"role":"user","content":[{"type":"tool_result","content":"1 passed"}]}
        ]"#;
        let c = parse_history(
            Path::new("/t/1700000000000/api_conversation_history.json"),
            json,
        )
        .unwrap();
        assert_eq!(c.tool, Tool::Cline);
        assert_eq!(c.messages.len(), 3);
        assert!(c.messages[1].content.contains("retry with a wait"));
        assert!(c.messages[1].content.contains("[tool: bash]"));
        assert!(c.messages[2].content.contains("1 passed"));
        assert!(c.started_at.is_some(), "taskId dir gives a timestamp");
    }

    #[test]
    fn empty_history_is_an_error() {
        assert!(parse_history(Path::new("/t/1/api_conversation_history.json"), "[]").is_err());
    }
}
