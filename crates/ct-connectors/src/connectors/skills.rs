//! Skills / prompts connectors — reusable artifacts, not conversations.
//!
//! These index the *definitions* you author and reuse across tools, as
//! `kind = skill` records so search can be scoped to them. Two clearly-located
//! sources, one per tool:
//!
//! - **Claude Code skills**: `~/.claude/skills/<name>/SKILL.md`
//!   (`CLAUDE_CONFIG_DIR` overrides `~/.claude`). YAML frontmatter gives a
//!   `name` and `description`; the markdown body is the instructions.
//! - **Codex prompts**: `~/.codex/prompts/<name>.md` (`CODEX_HOME` overrides
//!   `~/.codex`) — reusable slash-command prompts.
//!
//! Each artifact becomes a single-message record titled with its name, so
//! "which skill/prompt handles X?" is one (semantic) query across both tools.
//! Project-level `AGENTS.md` / `CLAUDE.md` instruction files are a natural
//! follow-up (they need project-tree scanning).

use std::path::{Path, PathBuf};

use ct_core::connector::{Connector, ConnectorError, DiscoveredSession, Result};
use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Kind, Message, Role, Source, Tool};

use crate::text::extract_code_snippets;

const MAX_DEPTH: usize = 6;

// ---------------------------------------------------------------------------
// Claude Code skills
// ---------------------------------------------------------------------------

pub const CLAUDE_SKILL_FINGERPRINT: &str = "claude-skill/v1";

#[derive(Debug, Default)]
pub struct ClaudeSkillsConnector;

impl ClaudeSkillsConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Connector for ClaudeSkillsConnector {
    fn tool(&self) -> Tool {
        Tool::ClaudeCode
    }

    fn fingerprint(&self) -> &'static str {
        CLAUDE_SKILL_FINGERPRINT
    }

    fn roots(&self) -> Vec<PathBuf> {
        let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".claude")));
        dir.map(|d| d.join("skills"))
            .filter(|p| p.exists())
            .into_iter()
            .collect()
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>> {
        let mut files = Vec::new();
        for root in self.roots() {
            collect_named(&root, "SKILL.md", 0, &mut files);
        }
        files.sort();
        files.dedup();
        Ok(files.into_iter().map(DiscoveredSession::file).collect())
    }

    fn parse(&self, session: &DiscoveredSession) -> Result<Conversation> {
        let text = read(&session.path)?;
        Ok(parse_skill_md(&session.path, &text))
    }
}

/// Parse a `SKILL.md` into a skill record. Title comes from frontmatter `name`
/// (else the parent directory name); the searchable body is description + body.
pub fn parse_skill_md(path: &Path, text: &str) -> Conversation {
    let (front, body) = split_frontmatter(text);
    let name = front_value(&front, "name").unwrap_or_else(|| parent_dir_name(path));
    let description = front_value(&front, "description").unwrap_or_default();

    let mut content = String::new();
    if !description.is_empty() {
        content.push_str(&description);
        content.push_str("\n\n");
    }
    content.push_str(body.trim());

    artifact(
        path,
        Tool::ClaudeCode,
        CLAUDE_SKILL_FINGERPRINT,
        &name,
        content,
    )
}

// ---------------------------------------------------------------------------
// Codex prompts
// ---------------------------------------------------------------------------

pub const CODEX_PROMPT_FINGERPRINT: &str = "codex-prompt/v1";

#[derive(Debug, Default)]
pub struct CodexPromptsConnector;

impl CodexPromptsConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Connector for CodexPromptsConnector {
    fn tool(&self) -> Tool {
        Tool::Codex
    }

    fn fingerprint(&self) -> &'static str {
        CODEX_PROMPT_FINGERPRINT
    }

    fn roots(&self) -> Vec<PathBuf> {
        let home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".codex")));
        home.map(|h| h.join("prompts"))
            .filter(|p| p.exists())
            .into_iter()
            .collect()
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>> {
        let mut files = Vec::new();
        for root in self.roots() {
            collect_ext(&root, "md", 0, &mut files);
        }
        files.sort();
        files.dedup();
        Ok(files.into_iter().map(DiscoveredSession::file).collect())
    }

    fn parse(&self, session: &DiscoveredSession) -> Result<Conversation> {
        let text = read(&session.path)?;
        let name = path_stem(&session.path);
        Ok(artifact(
            &session.path,
            Tool::Codex,
            CODEX_PROMPT_FINGERPRINT,
            &name,
            text.trim().to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// Build a `kind = skill` record: one system message carrying the artifact's
/// text, titled with its name.
fn artifact(
    path: &Path,
    tool: Tool,
    fingerprint: &str,
    name: &str,
    content: String,
) -> Conversation {
    let messages = vec![Message {
        id: "ms_0".into(),
        role: Role::System,
        content: content.clone(),
        timestamp: None,
        code_snippets: extract_code_snippets(&content),
        tool_calls: vec![],
        metadata: serde_json::Value::Null,
    }];
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(chrono::DateTime::<chrono::Utc>::from);
    let content_hash = conversation_hash(&tool, &messages);
    Conversation {
        id: conversation_id(&content_hash),
        tool,
        kind: Kind::Skill,
        title: Some(name.to_string()),
        project: None,
        model: None,
        started_at: modified,
        ended_at: modified,
        git_context: GitContext::default(),
        source: Source {
            path: path.to_string_lossy().into_owned(),
            offset: None,
            fingerprint: fingerprint.into(),
        },
        content_hash,
        messages,
    }
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| ConnectorError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Split optional leading YAML frontmatter (`--- … ---`) from the body.
fn split_frontmatter(text: &str) -> (String, String) {
    let trimmed = text.trim_start_matches('\u{feff}');
    if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let front = rest[..end].to_string();
            let body = rest[end + 4..].trim_start_matches('\n').to_string();
            return (front, body);
        }
    }
    (String::new(), text.to_string())
}

/// Read a `key: value` from simple frontmatter (quotes stripped).
fn front_value(front: &str, key: &str) -> Option<String> {
    for line in front.lines() {
        if let Some(rest) = line.trim().strip_prefix(key) {
            if let Some(v) = rest.trim_start().strip_prefix(':') {
                let v = v.trim().trim_matches(['"', '\'']).trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

fn parent_dir_name(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "skill".into())
}

fn path_stem(path: &Path) -> String {
    path.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "prompt".into())
}

/// Depth-limited recursive search for files with a given exact name.
fn collect_named(dir: &Path, name: &str, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < MAX_DEPTH {
                collect_named(&path, name, depth + 1, out);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            out.push(path);
        }
    }
}

/// Depth-limited recursive search for files with a given extension.
fn collect_ext(dir: &Path, ext: &str, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < MAX_DEPTH {
                collect_ext(&path, ext, depth + 1, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push(path);
        }
    }
}
