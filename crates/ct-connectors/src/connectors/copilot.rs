//! GitHub Copilot Chat connector (VS Code core chat sessions).
//!
//! Copilot Chat persistence now lives in VS Code core's `ChatSessionStore`, so
//! this reads the editor's on-disk chat sessions:
//!
//! ```text
//! <config>/<Editor>/User/workspaceStorage/<hash>/chatSessions/<sessionId>.{json,jsonl}
//! <config>/<Editor>/User/globalStorage/emptyWindowChatSessions/<sessionId>.{json,jsonl}
//! <config>/<Editor>/User/globalStorage/transferredChatSessions/<sessionId>.{json,jsonl}
//! ```
//!
//! Two formats coexist, keyed by the same sessionId. The `.json` form is one
//! `ISerializableChatData` object; the `.jsonl` form (VS Code ≥1.109) is an
//! append-only mutation log we replay to reconstruct the same object:
//! `{"kind":0,"v":<full object>}` then `set` (1), `push` (2), `delete` (3)
//! ops over a JSON path. We glob the `chatSessions` dirs directly rather than
//! trust the (50-entry-capped) `state.vscdb` index.
//!
//! Per request: the user prompt is `message` (a string, or `IParsedChatRequest`
//! with `.text`); the assistant reply is `response[]`, where markdown parts are
//! serialized unwrapped as `{value:…}` (or bare strings).

use std::path::{Path, PathBuf};

use ct_core::connector::{Connector, ConnectorError, DiscoveredSession, Result};
use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Kind, Message, Role, Source, Tool};

use crate::connectors::vscode::family_user_dirs;
use crate::text::extract_code_snippets;

pub const FINGERPRINT: &str = "copilot/v1";

#[derive(Debug, Default)]
pub struct CopilotConnector;

impl CopilotConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Connector for CopilotConnector {
    fn tool(&self) -> Tool {
        Tool::Copilot
    }

    fn fingerprint(&self) -> &'static str {
        FINGERPRINT
    }

    fn roots(&self) -> Vec<PathBuf> {
        session_dirs().into_iter().filter(|p| p.exists()).collect()
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>> {
        let mut out = Vec::new();
        for dir in self.roots() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext == "json" || ext == "jsonl" {
                    out.push(DiscoveredSession::file(path));
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

/// Every `chatSessions` / `*ChatSessions` directory under all editors.
fn session_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for user in family_user_dirs() {
        let global = user.join("globalStorage");
        out.push(global.join("emptyWindowChatSessions"));
        out.push(global.join("transferredChatSessions"));
        let ws = user.join("workspaceStorage");
        if let Ok(entries) = std::fs::read_dir(&ws) {
            for entry in entries.flatten() {
                out.push(entry.path().join("chatSessions"));
            }
        }
    }
    out
}

/// Parse one session file into a conversation.
pub fn parse_session(path: &Path, text: &str) -> Result<Conversation> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let data = if ext == "jsonl" {
        replay_jsonl(text)
    } else {
        serde_json::from_str(text).ok()
    };
    let data = data.ok_or_else(|| ConnectorError::Parse {
        path: path.to_path_buf(),
        reason: "could not reconstruct chat session".into(),
    })?;

    let started_at = data
        .get("creationDate")
        .and_then(|v| v.as_i64())
        .and_then(chrono::DateTime::from_timestamp_millis);
    let title = data
        .get("customTitle")
        .or_else(|| data.get("computedTitle"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());

    let mut messages = Vec::new();
    let requests = data.get("requests").and_then(|v| v.as_array());
    for req in requests.into_iter().flatten() {
        let user = user_text(req.get("message"));
        if !user.trim().is_empty() {
            messages.push(make(messages.len(), Role::User, &user));
        }
        let reply = response_text(req.get("response"));
        if !reply.trim().is_empty() {
            messages.push(make(messages.len(), Role::Assistant, &reply));
        }
    }

    if messages.is_empty() {
        return Err(ConnectorError::Parse {
            path: path.to_path_buf(),
            reason: "session had no request/response text".into(),
        });
    }

    let content_hash = conversation_hash(&Tool::Copilot, &messages);
    Ok(Conversation {
        id: conversation_id(&content_hash),
        tool: Tool::Copilot,
        kind: Kind::Thread,
        title,
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

fn make(idx: usize, role: Role, text: &str) -> Message {
    Message {
        id: format!("ms_{idx}"),
        role,
        code_snippets: extract_code_snippets(text),
        content: text.to_string(),
        timestamp: None,
        tool_calls: vec![],
        metadata: serde_json::Value::Null,
    }
}

/// User prompt: a raw string (legacy) or `IParsedChatRequest.text`.
fn user_text(message: Option<&serde_json::Value>) -> String {
    match message {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(obj) => obj
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        None => String::new(),
    }
}

/// Assistant reply: concatenate markdown parts of `response[]`. Markdown parts
/// are serialized unwrapped (bare string or `{value:…}` / `{content:{value}}`);
/// non-text parts (tool calls, thinking, references) are skipped.
fn response_text(response: Option<&serde_json::Value>) -> String {
    let Some(parts) = response.and_then(|v| v.as_array()) else {
        // A single string/markdown response is also valid.
        return response
            .map(|v| markdown_of(v).unwrap_or_default())
            .unwrap_or_default();
    };
    parts
        .iter()
        .filter_map(markdown_of)
        .collect::<Vec<_>>()
        .join("")
}

fn markdown_of(part: &serde_json::Value) -> Option<String> {
    if let Some(s) = part.as_str() {
        return Some(s.to_string());
    }
    // Unwrapped markdown: { value: "…" }.
    if let Some(v) = part.get("value").and_then(|v| v.as_str()) {
        return Some(v.to_string());
    }
    // { kind: "markdownContent", content: { value: "…" } }.
    if let Some(v) = part
        .get("content")
        .and_then(|c| c.get("value"))
        .and_then(|v| v.as_str())
    {
        return Some(v.to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// JSONL mutation-log replay (ObjectMutationLog)
// ---------------------------------------------------------------------------

/// Reconstruct the final session object from an append-only mutation log.
fn replay_jsonl(text: &str) -> Option<serde_json::Value> {
    let mut root: Option<serde_json::Value> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(op) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = op.get("kind").and_then(|v| v.as_i64()).unwrap_or(-1);
        match kind {
            0 => root = op.get("v").cloned(),
            1 => {
                if let (Some(r), Some(path)) =
                    (root.as_mut(), op.get("k").and_then(|v| v.as_array()))
                {
                    apply_set(
                        r,
                        path,
                        op.get("v").cloned().unwrap_or(serde_json::Value::Null),
                    );
                }
            }
            2 => {
                if let (Some(r), Some(path)) =
                    (root.as_mut(), op.get("k").and_then(|v| v.as_array()))
                {
                    apply_push(r, path, op.get("v"), op.get("i").and_then(|v| v.as_u64()));
                }
            }
            3 => {
                if let (Some(r), Some(path)) =
                    (root.as_mut(), op.get("k").and_then(|v| v.as_array()))
                {
                    apply_delete(r, path);
                }
            }
            _ => {}
        }
    }
    root
}

/// Resolve a mutable reference to the container holding the final path segment.
fn nav<'a>(
    root: &'a mut serde_json::Value,
    path: &[serde_json::Value],
) -> Option<&'a mut serde_json::Value> {
    let mut cur = root;
    for seg in path {
        cur = match (cur, seg) {
            (serde_json::Value::Object(map), serde_json::Value::String(k)) => map.get_mut(k)?,
            (serde_json::Value::Array(arr), serde_json::Value::Number(n)) => {
                arr.get_mut(n.as_u64()? as usize)?
            }
            _ => return None,
        };
    }
    Some(cur)
}

fn apply_set(root: &mut serde_json::Value, path: &[serde_json::Value], val: serde_json::Value) {
    let Some((last, parents)) = path.split_last() else {
        *root = val;
        return;
    };
    let Some(container) = nav(root, parents) else {
        return;
    };
    match (container, last) {
        (serde_json::Value::Object(map), serde_json::Value::String(k)) => {
            map.insert(k.clone(), val);
        }
        (serde_json::Value::Array(arr), serde_json::Value::Number(n)) => {
            if let Some(i) = n.as_u64() {
                let i = i as usize;
                if i < arr.len() {
                    arr[i] = val;
                } else if i == arr.len() {
                    arr.push(val);
                }
            }
        }
        _ => {}
    }
}

fn apply_push(
    root: &mut serde_json::Value,
    path: &[serde_json::Value],
    items: Option<&serde_json::Value>,
    truncate_at: Option<u64>,
) {
    let Some(target) = nav(root, path) else {
        return;
    };
    let serde_json::Value::Array(arr) = target else {
        return;
    };
    if let Some(i) = truncate_at {
        arr.truncate(i as usize);
    }
    match items {
        Some(serde_json::Value::Array(new)) => arr.extend(new.iter().cloned()),
        Some(other) => arr.push(other.clone()),
        None => {}
    }
}

fn apply_delete(root: &mut serde_json::Value, path: &[serde_json::Value]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let Some(container) = nav(root, parents) else {
        return;
    };
    if let (serde_json::Value::Object(map), serde_json::Value::String(k)) = (container, last) {
        map.remove(k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_session_with_parsed_request() {
        let json = r#"{
            "version":3,"sessionId":"s1","creationDate":1700000000000,
            "customTitle":"Fixing auth",
            "requests":[
                {"message":{"text":"why does login 401?"},
                 "response":[{"value":"Your token expired. "},{"value":"Refresh it."}]}
            ]
        }"#;
        let c = parse_session(Path::new("/ws/chatSessions/s1.json"), json).unwrap();
        assert_eq!(c.tool, Tool::Copilot);
        assert_eq!(c.title.as_deref(), Some("Fixing auth"));
        assert_eq!(c.messages.len(), 2);
        assert!(c.messages[0].content.contains("login 401"));
        assert_eq!(c.messages[1].content, "Your token expired. Refresh it.");
        assert!(c.started_at.is_some());
    }

    #[test]
    fn replays_jsonl_push_log() {
        let jsonl = concat!(
            r#"{"kind":0,"v":{"version":3,"sessionId":"s","creationDate":1700000000000,"requests":[]}}"#,
            "\n",
            r#"{"kind":2,"k":["requests"],"v":[{"message":"hello","response":["hi there"]}]}"#,
            "\n",
            r#"{"kind":1,"k":["customTitle"],"v":"Greeting"}"#,
            "\n",
        );
        let c = parse_session(Path::new("/g/emptyWindowChatSessions/s.jsonl"), jsonl).unwrap();
        assert_eq!(c.title.as_deref(), Some("Greeting"));
        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.messages[0].content, "hello");
        assert_eq!(c.messages[1].content, "hi there");
    }
}
