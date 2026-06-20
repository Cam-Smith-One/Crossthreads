//! Windsurf (Codeium Cascade) connector.
//!
//! Windsurf is a VS Code fork, so chat history sits in `state.vscdb` (the VS
//! Code key/value store) under the Windsurf editor dir — global and per
//! workspace:
//!
//! ```text
//! <config>/Windsurf/User/globalStorage/state.vscdb
//! <config>/Windsurf/User/workspaceStorage/<hash>/state.vscdb
//! ```
//!
//! The conversation JSON is stored in `ItemTable` under one of several keys
//! (the schema is undocumented and shifts between releases, so we try each):
//! `workbench.panel.aichat.view.aichat.chatdata`, `aiChat.chatdata`,
//! `chat.data`, `cascade.chatdata`. The value decodes to `{tabs:[…]}`; each tab
//! is a conversation whose messages carry a role (`type` as `user`/`assistant`
//! or numeric `1`/`2`, or a `role` field) and text under
//! `rawText` → `text` → `content`.
//!
//! Confidence is medium — fields are sniffed defensively and unknown shapes are
//! skipped rather than failing the run.

use std::path::{Path, PathBuf};

use ct_core::connector::{Connector, ConnectorError, DiscoveredSession, Result};
use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Kind, Message, Role, Source, Tool};

use crate::connectors::vscode::{open_vscdb_ro, vscdb_get};
use crate::text::extract_code_snippets;

pub const FINGERPRINT: &str = "windsurf/v1";

/// `ItemTable` keys that have been observed to hold the chat JSON, most likely
/// first.
const CHAT_KEYS: &[&str] = &[
    "workbench.panel.aichat.view.aichat.chatdata",
    "aiChat.chatdata",
    "chat.data",
    "cascade.chatdata",
];

#[derive(Debug, Default)]
pub struct WindsurfConnector;

impl WindsurfConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Connector for WindsurfConnector {
    fn tool(&self) -> Tool {
        Tool::Windsurf
    }

    fn fingerprint(&self) -> &'static str {
        FINGERPRINT
    }

    fn roots(&self) -> Vec<PathBuf> {
        db_files().into_iter().filter(|p| p.exists()).collect()
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>> {
        let mut out = Vec::new();
        for db in self.roots() {
            let Some(conn) = open_vscdb_ro(&db) else {
                continue;
            };
            for key in CHAT_KEYS {
                let Some(raw) = vscdb_get(&conn, "ItemTable", key) else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
                    continue;
                };
                for (i, tab) in value
                    .get("tabs")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let tab_id = tab
                        .get("tabId")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| i.to_string());
                    out.push(DiscoveredSession {
                        path: db.clone(),
                        locator: Some(format!("{key}#{tab_id}")),
                    });
                }
                break; // first key that exists wins
            }
        }
        Ok(out)
    }

    fn parse(&self, session: &DiscoveredSession) -> Result<Conversation> {
        let locator = session
            .locator
            .as_deref()
            .ok_or_else(|| ConnectorError::Parse {
                path: session.path.clone(),
                reason: "windsurf sessions require a locator".into(),
            })?;
        let (key, tab_id) = locator
            .split_once('#')
            .ok_or_else(|| ConnectorError::Parse {
                path: session.path.clone(),
                reason: format!("bad locator: {locator}"),
            })?;
        let conn = open_vscdb_ro(&session.path).ok_or_else(|| ConnectorError::Io {
            path: session.path.clone(),
            source: std::io::Error::other("cannot open state.vscdb"),
        })?;
        let raw = vscdb_get(&conn, "ItemTable", key).ok_or_else(|| ConnectorError::Parse {
            path: session.path.clone(),
            reason: format!("{key} missing"),
        })?;
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| ConnectorError::Parse {
                path: session.path.clone(),
                reason: format!("chatdata json: {e}"),
            })?;
        let tab = value
            .get("tabs")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .enumerate()
            .find(|(i, t)| {
                t.get("tabId").and_then(|v| v.as_str()) == Some(tab_id) || i.to_string() == tab_id
            })
            .map(|(_, t)| t)
            .ok_or_else(|| ConnectorError::Parse {
                path: session.path.clone(),
                reason: format!("tab {tab_id} not found"),
            })?;
        tab_to_conversation(tab, &session.path).ok_or_else(|| ConnectorError::Parse {
            path: session.path.clone(),
            reason: "tab had no usable messages".into(),
        })
    }
}

fn db_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for user in windsurf_user_dirs() {
        let global = user.join("globalStorage").join("state.vscdb");
        if global.exists() {
            out.push(global);
        }
        let ws = user.join("workspaceStorage");
        if let Ok(entries) = std::fs::read_dir(&ws) {
            for entry in entries.flatten() {
                let db = entry.path().join("state.vscdb");
                if db.exists() {
                    out.push(db);
                }
            }
        }
    }
    out
}

/// Windsurf `User` dirs. `WINDSURF_CONFIG_DIR` (a `User` dir) overrides.
fn windsurf_user_dirs() -> Vec<PathBuf> {
    if let Ok(dir) = std::env::var("WINDSURF_CONFIG_DIR") {
        return vec![PathBuf::from(dir)];
    }
    let Some(config) = dirs::config_dir() else {
        return Vec::new();
    };
    ["Windsurf", "Windsurf - Next"]
        .iter()
        .map(|e| config.join(e).join("User"))
        .filter(|p| p.exists())
        .collect()
}

/// Build a conversation from a Windsurf chat `tab`.
pub fn tab_to_conversation(tab: &serde_json::Value, path: &Path) -> Option<Conversation> {
    let bubbles = tab
        .get("messages")
        .or_else(|| tab.get("bubbles"))
        .and_then(|v| v.as_array())?;
    let mut messages = Vec::new();
    for (idx, bubble) in bubbles.iter().enumerate() {
        let role = role_of(bubble);
        let text = bubble_text(bubble);
        if text.trim().is_empty() {
            continue;
        }
        messages.push(Message {
            id: format!("ms_{idx}"),
            role,
            code_snippets: extract_code_snippets(&text),
            content: text,
            timestamp: None,
            tool_calls: vec![],
            metadata: serde_json::Value::Null,
        });
    }
    if messages.is_empty() {
        return None;
    }

    let title = tab
        .get("chatTitle")
        .or_else(|| tab.get("name"))
        .or_else(|| tab.get("title"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let started_at = tab
        .get("createdAt")
        .and_then(|v| v.as_i64())
        .and_then(chrono::DateTime::from_timestamp_millis);

    let content_hash = conversation_hash(&Tool::Windsurf, &messages);
    Some(Conversation {
        id: conversation_id(&content_hash),
        tool: Tool::Windsurf,
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

/// Sniff a bubble's role from `type` (string or 1/2) or a `role` field.
fn role_of(bubble: &serde_json::Value) -> Role {
    if let Some(t) = bubble.get("type").and_then(|v| v.as_str()) {
        return match t {
            "assistant" | "ai" | "bot" => Role::Assistant,
            "tool" | "function" => Role::Tool,
            _ => Role::User,
        };
    }
    if let Some(n) = bubble.get("type").and_then(|v| v.as_i64()) {
        return if n == 2 { Role::Assistant } else { Role::User };
    }
    match bubble.get("role").and_then(|v| v.as_str()) {
        Some("assistant") | Some("ai") | Some("bot") => Role::Assistant,
        Some("tool") | Some("function") => Role::Tool,
        _ => Role::User,
    }
}

/// Text via the `rawText` → `text` → `content` fallback chain.
fn bubble_text(bubble: &serde_json::Value) -> String {
    for key in ["rawText", "text", "content"] {
        if let Some(s) = bubble.get(key).and_then(|v| v.as_str()) {
            return s.to_string();
        }
        if let Some(arr) = bubble.get(key).and_then(|v| v.as_array()) {
            let joined: Vec<String> = arr
                .iter()
                .filter_map(|b| {
                    if b.get("type").and_then(|v| v.as_str()) == Some("text")
                        || b.get("text").is_some()
                    {
                        b.get("text")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    } else {
                        None
                    }
                })
                .collect();
            if !joined.is_empty() {
                return joined.join("\n\n");
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tab_with_mixed_roles() {
        let tab = serde_json::json!({
            "tabId":"t1","chatTitle":"Refactor",
            "messages":[
                {"type":"user","rawText":"extract this into a hook"},
                {"type":2,"text":"done — see useAuth()"},
                {"role":"assistant","content":"anything else?"}
            ]
        });
        let c = tab_to_conversation(&tab, Path::new("/w/state.vscdb")).unwrap();
        assert_eq!(c.tool, Tool::Windsurf);
        assert_eq!(c.title.as_deref(), Some("Refactor"));
        assert_eq!(c.messages.len(), 3);
        assert_eq!(c.messages[0].role, Role::User);
        assert_eq!(c.messages[1].role, Role::Assistant);
        assert_eq!(c.messages[2].role, Role::Assistant);
    }
}
