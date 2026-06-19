//! Codex connector.
//!
//! OpenAI's Codex CLI writes per-session "rollout" transcripts as JSONL under
//! `~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-*.jsonl` (`CODEX_HOME` overrides
//! `~/.codex`). Each line is a record; the shapes we care about:
//!
//! - **session meta** — `{"type":"session_meta","payload":{"cwd","timestamp",
//!   "model"?,"instructions"?}}` (also tolerated un-tagged at top level),
//! - **response items** — `{"type":"response_item","payload":{"type":"message",
//!   "role":…,"content":[{"type":"input_text"|"output_text","text":…}]}}`,
//!   plus `function_call` / `function_call_output` items.
//!
//! The format drifts across Codex versions, so this parser is defensive:
//! records are unwrapped flexibly, unknown shapes are skipped, and everything
//! is stamped with the `codex/v1` fingerprint (FR-ING-08). The regression
//! corpus (`tests/`) guards against silent breakage.

use std::path::{Path, PathBuf};

use ct_core::connector::{Connector, ConnectorError, DiscoveredSession, Result};
use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Message, Role, Source, Tool, ToolCall};

use crate::text::extract_code_snippets;

pub const FINGERPRINT: &str = "codex/v1";
const MAX_DEPTH: usize = 8;

#[derive(Debug, Default)]
pub struct CodexConnector;

impl CodexConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Connector for CodexConnector {
    fn tool(&self) -> Tool {
        Tool::Codex
    }

    fn fingerprint(&self) -> &'static str {
        FINGERPRINT
    }

    fn roots(&self) -> Vec<PathBuf> {
        default_roots().into_iter().filter(|p| p.exists()).collect()
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>> {
        let mut files = Vec::new();
        for root in self.roots() {
            collect_jsonl(&root, 0, &mut files);
        }
        files.sort();
        files.dedup();
        Ok(files.into_iter().map(DiscoveredSession::file).collect())
    }

    fn parse(&self, session: &DiscoveredSession) -> Result<Conversation> {
        let text = std::fs::read_to_string(&session.path).map_err(|source| ConnectorError::Io {
            path: session.path.clone(),
            source,
        })?;
        parse_rollout(&session.path, &text)
    }
}

fn default_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")));
    home.map(|h| vec![h.join("sessions")]).unwrap_or_default()
}

fn collect_jsonl(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < MAX_DEPTH {
                collect_jsonl(&path, depth + 1, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

/// Parse one rollout file into a conversation.
pub fn parse_rollout(path: &Path, text: &str) -> Result<Conversation> {
    let mut messages = Vec::new();
    let mut project: Option<String> = None;
    let mut model: Option<String> = None;
    let mut started_at = None;
    let mut ended_at = None;

    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue; // tolerate a bad line
        };

        // Records may be `{type, payload}` envelopes or bare items.
        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let payload = value.get("payload").unwrap_or(&value);

        // Conversation-level metadata, wherever it appears.
        if project.is_none() {
            if let Some(cwd) = payload.get("cwd").and_then(|v| v.as_str()) {
                project = Some(cwd.to_string());
            }
        }
        if model.is_none() {
            if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
                model = Some(m.to_string());
            }
        }
        if let Some(ts) = value
            .get("timestamp")
            .or_else(|| payload.get("timestamp"))
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
        {
            started_at = Some(started_at.map_or(ts, |c: chrono::DateTime<chrono::Utc>| c.min(ts)));
            ended_at = Some(ended_at.map_or(ts, |c: chrono::DateTime<chrono::Utc>| c.max(ts)));
        }

        if kind == "session_meta" {
            continue;
        }
        if let Some(msg) = item_to_message(idx, payload) {
            messages.push(msg);
        }
    }

    if messages.is_empty() {
        return Err(ConnectorError::Parse {
            path: path.to_path_buf(),
            reason: "no messages found in rollout".into(),
        });
    }

    let content_hash = conversation_hash(&Tool::Codex, &messages);
    Ok(Conversation {
        id: conversation_id(&content_hash),
        tool: Tool::Codex,
        project,
        model,
        started_at,
        ended_at,
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

/// Map a response item into a message, or `None` to skip (e.g. reasoning).
fn item_to_message(idx: usize, item: &serde_json::Value) -> Option<Message> {
    let itype = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match itype {
        "message" => {
            let role = match item.get("role").and_then(|v| v.as_str()) {
                Some("assistant") => Role::Assistant,
                Some("system") => Role::System,
                Some("tool") => Role::Tool,
                _ => Role::User,
            };
            let content = extract_text(item.get("content"));
            if content.trim().is_empty() {
                return None;
            }
            Some(make_message(idx, role, &content, vec![]))
        }
        "function_call" | "local_shell_call" | "custom_tool_call" => {
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("tool")
                .to_string();
            let args = item.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
            let content = format!("{name}({})", value_to_text(&args));
            Some(make_message(
                idx,
                Role::Tool,
                &content,
                vec![ToolCall { name, args, result_ref: None }],
            ))
        }
        "function_call_output" | "local_shell_call_output" => {
            let content = extract_text(item.get("output").or_else(|| item.get("content")));
            if content.trim().is_empty() {
                return None;
            }
            Some(make_message(idx, Role::Tool, &content, vec![]))
        }
        // reasoning, web_search_call, etc. — not searchable user/assistant text.
        _ => None,
    }
}

fn make_message(idx: usize, role: Role, content: &str, tool_calls: Vec<ToolCall>) -> Message {
    Message {
        id: format!("ms_{idx}"),
        role,
        content: content.to_string(),
        timestamp: None,
        code_snippets: extract_code_snippets(content),
        tool_calls,
        metadata: serde_json::Value::Null,
    }
}

/// Flatten a `content` field (string or block array) into plain text.
fn extract_text(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(blocks)) => {
            let mut out = String::new();
            for b in blocks {
                if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                    if !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    out.push_str(t);
                } else if let Some(s) = b.as_str() {
                    out.push_str(s);
                }
            }
            out
        }
        _ => String::new(),
    }
}

fn value_to_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}
