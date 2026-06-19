//! Claude Code connector.
//!
//! Claude Code stores one session per JSONL file under
//! `~/.claude/projects/<project-slug>/<session-id>.jsonl`. Each line is an
//! event; the ones we care about carry a `message` object with a `role` and
//! `content` (a string, or an array of typed blocks: `text`, `tool_use`,
//! `tool_result`, `thinking`). Top-level fields give us `timestamp`, `cwd`
//! (project), and `gitBranch`.
//!
//! This is a best-effort parser: unknown line shapes are skipped, not fatal
//! (FR-ING-07).

use std::fs;
use std::path::{Path, PathBuf};

use ct_core::connector::{Connector, ConnectorError, DiscoveredSession, Result};
use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{
    Conversation, GitContext, Message, Role, Source, Tool, ToolCall,
};

use crate::text::extract_code_snippets;

pub const FINGERPRINT: &str = "claude-code/v1";

#[derive(Debug, Default)]
pub struct ClaudeCodeConnector;

impl ClaudeCodeConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Connector for ClaudeCodeConnector {
    fn tool(&self) -> Tool {
        Tool::ClaudeCode
    }

    fn fingerprint(&self) -> &'static str {
        FINGERPRINT
    }

    fn roots(&self) -> Vec<PathBuf> {
        default_roots()
            .into_iter()
            .filter(|p| p.exists())
            .collect()
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>> {
        let mut sessions = Vec::new();
        for root in self.roots() {
            collect_jsonl(&root, &mut sessions).map_err(|source| ConnectorError::Io {
                path: root.clone(),
                source,
            })?;
        }
        Ok(sessions)
    }

    fn parse(&self, session: &DiscoveredSession) -> Result<Conversation> {
        let text = fs::read_to_string(&session.path).map_err(|source| ConnectorError::Io {
            path: session.path.clone(),
            source,
        })?;
        parse_jsonl(&session.path, &text)
    }
}

/// Candidate `projects` roots across platforms. `CLAUDE_CONFIG_DIR` overrides.
fn default_roots() -> Vec<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        return vec![PathBuf::from(dir).join("projects")];
    }
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".claude").join("projects"));
    }
    roots
}

fn collect_jsonl(dir: &Path, out: &mut Vec<DiscoveredSession>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(DiscoveredSession::file(path));
        }
    }
    Ok(())
}

/// Parse the full text of one JSONL session file.
pub fn parse_jsonl(path: &Path, text: &str) -> Result<Conversation> {
    let mut messages = Vec::new();
    let mut project: Option<String> = None;
    let mut model: Option<String> = None;
    let mut branch: Option<String> = None;
    let mut commit: Option<String> = None;
    let mut started_at = None;
    let mut ended_at = None;

    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            // A single malformed line shouldn't sink the session.
            Err(_) => continue,
        };

        // Capture conversation-level metadata wherever it first appears.
        if project.is_none() {
            if let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()) {
                project = Some(cwd.to_string());
            }
        }
        if branch.is_none() {
            if let Some(b) = value.get("gitBranch").and_then(|v| v.as_str()) {
                if !b.is_empty() {
                    branch = Some(b.to_string());
                }
            }
        }
        if commit.is_none() {
            if let Some(c) = value.get("gitCommit").and_then(|v| v.as_str()) {
                commit = Some(c.to_string());
            }
        }

        let ts = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        if let Some(ts) = ts {
            started_at = Some(started_at.map_or(ts, |cur: chrono::DateTime<chrono::Utc>| cur.min(ts)));
            ended_at = Some(ended_at.map_or(ts, |cur: chrono::DateTime<chrono::Utc>| cur.max(ts)));
        }

        let Some(msg) = value.get("message") else {
            continue; // summary / meta lines carry no message
        };
        if model.is_none() {
            if let Some(m) = msg.get("model").and_then(|v| v.as_str()) {
                model = Some(m.to_string());
            }
        }

        if let Some(message) = event_to_message(idx, &value, msg, ts) {
            messages.push(message);
        }
    }

    if messages.is_empty() {
        return Err(ConnectorError::Parse {
            path: path.to_path_buf(),
            reason: "no messages found in session".into(),
        });
    }

    let content_hash = conversation_hash(&Tool::ClaudeCode, &messages);
    Ok(Conversation {
        id: conversation_id(&content_hash),
        tool: Tool::ClaudeCode,
        project,
        model,
        started_at,
        ended_at,
        git_context: GitContext { branch, commit },
        source: Source {
            path: path.to_string_lossy().into_owned(),
            offset: None,
            fingerprint: FINGERPRINT.into(),
        },
        content_hash,
        messages,
    })
}

/// Map one event line's `message` object into a normalized [`Message`].
fn event_to_message(
    idx: usize,
    event: &serde_json::Value,
    msg: &serde_json::Value,
    ts: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<Message> {
    let role_str = msg.get("role").and_then(|v| v.as_str()).unwrap_or_else(|| {
        event.get("type").and_then(|v| v.as_str()).unwrap_or("")
    });

    let (content, tool_calls, only_tool_result) = extract_content(msg.get("content"));
    if content.trim().is_empty() && tool_calls.is_empty() {
        return None;
    }

    // Tool results arrive as role "user"; reclassify so search/UX can tell them
    // apart from genuine user turns.
    let role = match role_str {
        "assistant" => Role::Assistant,
        "system" => Role::System,
        "user" if only_tool_result => Role::Tool,
        "user" => Role::User,
        _ => Role::User,
    };

    let id = event
        .get("uuid")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("ms_{idx}"));

    let code_snippets = extract_code_snippets(&content);

    Some(Message {
        id,
        role,
        content,
        timestamp: ts,
        code_snippets,
        tool_calls,
        metadata: serde_json::Value::Null,
    })
}

/// Flatten a `content` field (string or block array) into plain text plus any
/// tool calls. The bool indicates the content was *only* tool_result blocks.
fn extract_content(content: Option<&serde_json::Value>) -> (String, Vec<ToolCall>, bool) {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut saw_block = false;
    let mut saw_non_tool_result = false;

    match content {
        Some(serde_json::Value::String(s)) => {
            return (s.clone(), tool_calls, false);
        }
        Some(serde_json::Value::Array(blocks)) => {
            for block in blocks {
                saw_block = true;
                let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match btype {
                    "text" => {
                        saw_non_tool_result = true;
                        if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                            push_para(&mut text, t);
                        }
                    }
                    "tool_use" => {
                        saw_non_tool_result = true;
                        let name = block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args = block.get("input").cloned().unwrap_or(serde_json::Value::Null);
                        tool_calls.push(ToolCall {
                            name,
                            args,
                            result_ref: block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        });
                    }
                    "tool_result" => {
                        // Keep result text searchable; flag for role reclassification.
                        if let Some(rt) = tool_result_text(block.get("content")) {
                            push_para(&mut text, &rt);
                        }
                    }
                    "thinking" => {
                        // Reasoning traces are intentionally excluded from the
                        // searchable body for now (noisy); revisit if useful.
                        saw_non_tool_result = true;
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }

    // "Only tool_result" means we saw blocks but none were text/tool_use/
    // thinking — i.e. every block was a tool_result. Used to reclassify the
    // message role from User to Tool.
    let only_tool_result = saw_block && !saw_non_tool_result;
    (text, tool_calls, only_tool_result)
}

fn tool_result_text(content: Option<&serde_json::Value>) -> Option<String> {
    match content {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(blocks)) => {
            let mut out = String::new();
            for b in blocks {
                if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                    push_para(&mut out, t);
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        }
        _ => None,
    }
}

fn push_para(buf: &mut String, s: &str) {
    if !buf.is_empty() {
        buf.push_str("\n\n");
    }
    buf.push_str(s);
}
