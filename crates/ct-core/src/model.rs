//! The normalized schema every connector produces.
//!
//! Mirrors the data model sketched in `docs/ARCHITECTURE.md` §4. All source
//! tools are mapped onto these types so search, dedup, and the agent API see a
//! single shape regardless of origin.

use serde::{Deserialize, Serialize};

/// A source coding agent a conversation came from.
///
/// `Other` keeps an unknown/experimental tool's identifier without forcing a
/// new enum variant — connectors authored as plugins (P2) use it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tool {
    ClaudeCode,
    Cursor,
    Aider,
    Codex,
    GeminiCli,
    Windsurf,
    Cline,
    Copilot,
    Antigravity,
    #[serde(untagged)]
    Other(String),
}

impl Tool {
    /// Stable slug used in ids, paths, and filters.
    pub fn slug(&self) -> &str {
        match self {
            Tool::ClaudeCode => "claude-code",
            Tool::Cursor => "cursor",
            Tool::Aider => "aider",
            Tool::Codex => "codex",
            Tool::GeminiCli => "gemini-cli",
            Tool::Windsurf => "windsurf",
            Tool::Cline => "cline",
            Tool::Copilot => "copilot",
            Tool::Antigravity => "antigravity",
            Tool::Other(s) => s,
        }
    }
}

/// Who authored a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
}

/// Git state captured at conversation time, when discoverable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GitContext {
    pub branch: Option<String>,
    pub commit: Option<String>,
}

/// A fenced code block extracted from a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeSnippet {
    pub language: Option<String>,
    pub text: String,
    /// File the snippet was associated with, if the source linked one.
    pub linked_file: Option<String>,
}

/// A tool/function call recorded in a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    /// Raw arguments as the source recorded them (JSON when available).
    pub args: serde_json::Value,
    /// Opaque reference to the result, if the source stored one separately.
    pub result_ref: Option<String>,
}

/// A single turn in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: Role,
    pub content: String,
    /// ISO-8601 timestamp when known.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub code_snippets: Vec<CodeSnippet>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// Source-specific metadata we don't model first-class yet.
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// What kind of artifact a record is. Threads are conversations; skills are
/// reusable artifacts (Claude Code `SKILL.md`, Codex prompts, `AGENTS.md` /
/// `CLAUDE.md` instruction files). One index, scoped by `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Thread,
    Skill,
}

impl Kind {
    pub fn slug(&self) -> &'static str {
        match self {
            Kind::Thread => "thread",
            Kind::Skill => "skill",
        }
    }
}

/// A normalized conversation — the unit of indexing and search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    /// Stable, content-derived id (see [`crate::hash`]).
    pub id: String,
    pub tool: Tool,
    /// Whether this is a conversation thread or a reusable artifact (skill).
    #[serde(default)]
    pub kind: Kind,
    /// Explicit title, when the source has a natural name (e.g. a skill name).
    /// Falls back to [`Conversation::derived_title`].
    #[serde(default)]
    pub title: Option<String>,
    /// Workspace / repository path the conversation belongs to, if known.
    pub project: Option<String>,
    /// Model name, when the source records it.
    pub model: Option<String>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub git_context: GitContext,
    /// Exact source for "open original".
    pub source: Source,
    /// Content hash for deduplication.
    pub content_hash: String,
    pub messages: Vec<Message>,
}

/// Provenance back to the exact bytes a conversation came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// Absolute path to the source file.
    pub path: String,
    /// Byte/line offset within the file, when meaningful (e.g. JSONL line).
    pub offset: Option<u64>,
    /// Connector format fingerprint that produced this (e.g. "claude-code/v1").
    pub fingerprint: String,
}

impl Conversation {
    /// A short human title for result lists: the explicit `title` if set, else
    /// the first non-empty user message, truncated. Falls back to project/tool.
    pub fn derived_title(&self) -> String {
        if let Some(title) = &self.title {
            if !title.trim().is_empty() {
                return truncate(title.trim(), 80);
            }
        }
        if let Some(first) = self
            .messages
            .iter()
            .find(|m| m.role == Role::User && !m.content.trim().is_empty())
        {
            let line = first.content.trim().lines().next().unwrap_or("").trim();
            return truncate(line, 80);
        }
        self.project
            .clone()
            .unwrap_or_else(|| self.tool.slug().to_string())
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_slug_roundtrips_through_serde() {
        let t = Tool::ClaudeCode;
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "\"claude-code\"");
        let back: Tool = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Tool::ClaudeCode);
    }

    #[test]
    fn unknown_tool_falls_back_to_other() {
        let t: Tool = serde_json::from_str("\"some-new-agent\"").unwrap();
        assert_eq!(t, Tool::Other("some-new-agent".to_string()));
        assert_eq!(t.slug(), "some-new-agent");
    }

    #[test]
    fn derived_title_prefers_first_user_message() {
        let convo = Conversation {
            id: "x".into(),
            tool: Tool::ClaudeCode,
            kind: Kind::Thread,
            title: None,
            project: Some("/repo".into()),
            model: None,
            started_at: None,
            ended_at: None,
            git_context: GitContext::default(),
            source: Source {
                path: "/f".into(),
                offset: None,
                fingerprint: "claude-code/v1".into(),
            },
            content_hash: "h".into(),
            messages: vec![Message {
                id: "m1".into(),
                role: Role::User,
                content: "Fix the OAuth refresh loop\nmore detail".into(),
                timestamp: None,
                code_snippets: vec![],
                tool_calls: vec![],
                metadata: serde_json::Value::Null,
            }],
        };
        assert_eq!(convo.derived_title(), "Fix the OAuth refresh loop");
    }
}
