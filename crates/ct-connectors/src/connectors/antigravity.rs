//! Antigravity (Google's agentic IDE) connector — best effort.
//!
//! Antigravity splits storage: the full conversation lives in per-conversation
//! **protobuf `.pb` files** under `~/.gemini/antigravity*/conversations/<UUID>.pb`
//! whose wire schema is **undocumented**, while human-readable working artifacts
//! (the task brief, implementation plan, walkthrough) are plain Markdown under
//! `~/.gemini/antigravity*/brain/<UUID>/*.md`.
//!
//! Rather than guess at the undocumented protobuf, this connector indexes the
//! Markdown brain artifacts: one conversation per `<UUID>`, each `.md` file a
//! message. That makes Antigravity work searchable today (plans, tasks,
//! walkthroughs) without fabricating a parse of the `.pb` content. When the
//! `.pb` format is documented, a richer parser can replace this.
//!
//! `ANTIGRAVITY_HOME` (a path containing `brain/`) overrides discovery.

use std::path::{Path, PathBuf};

use ct_core::connector::{Connector, ConnectorError, DiscoveredSession, Result};
use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Kind, Message, Role, Source, Tool};

use crate::text::extract_code_snippets;

pub const FINGERPRINT: &str = "antigravity-brain/v1";

/// Folder-name variants Antigravity has used across versions, under `~/.gemini`.
const HOME_VARIANTS: &[&str] = &[
    "antigravity",
    "antigravity-ide",
    "Antigravity",
    "Antigravity IDE",
];

#[derive(Debug, Default)]
pub struct AntigravityConnector;

impl AntigravityConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Connector for AntigravityConnector {
    fn tool(&self) -> Tool {
        Tool::Antigravity
    }

    fn fingerprint(&self) -> &'static str {
        FINGERPRINT
    }

    fn roots(&self) -> Vec<PathBuf> {
        brain_dirs().into_iter().filter(|p| p.exists()).collect()
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>> {
        let mut out = Vec::new();
        for brain in self.roots() {
            let Ok(entries) = std::fs::read_dir(&brain) else {
                continue;
            };
            for entry in entries.flatten() {
                let dir = entry.path();
                // One conversation per <UUID> dir that has any Markdown.
                if dir.is_dir() && has_markdown(&dir) {
                    out.push(DiscoveredSession::file(dir));
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out.dedup();
        Ok(out)
    }

    fn parse(&self, session: &DiscoveredSession) -> Result<Conversation> {
        parse_brain_dir(&session.path)
    }
}

fn brain_dirs() -> Vec<PathBuf> {
    if let Some(home) = std::env::var_os("ANTIGRAVITY_HOME") {
        return vec![PathBuf::from(home).join("brain")];
    }
    let Some(gemini) = dirs::home_dir().map(|h| h.join(".gemini")) else {
        return Vec::new();
    };
    HOME_VARIANTS
        .iter()
        .map(|v| gemini.join(v).join("brain"))
        .collect()
}

fn has_markdown(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries
        .flatten()
        .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
}

/// Build a conversation from a `brain/<UUID>/` directory of Markdown artifacts.
pub fn parse_brain_dir(dir: &Path) -> Result<Conversation> {
    // Read the .md files in a stable order (task brief first when present).
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|source| ConnectorError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("md"))
        .collect();
    files.sort_by_key(|p| {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // task.md sorts first, then the rest alphabetically.
        (name != "task.md", name.to_string())
    });

    let mut messages = Vec::new();
    let mut title: Option<String> = None;
    for (idx, file) in files.iter().enumerate() {
        let Ok(content) = std::fs::read_to_string(file) else {
            continue;
        };
        if content.trim().is_empty() {
            continue;
        }
        if title.is_none() {
            title = first_heading(&content);
        }
        let label = file.file_name().and_then(|n| n.to_str()).unwrap_or("note");
        let body = format!("## {label}\n\n{content}");
        messages.push(Message {
            id: format!("ms_{idx}"),
            role: Role::Assistant,
            code_snippets: extract_code_snippets(&body),
            content: body,
            timestamp: None,
            tool_calls: vec![],
            metadata: serde_json::Value::Null,
        });
    }

    if messages.is_empty() {
        return Err(ConnectorError::Parse {
            path: dir.to_path_buf(),
            reason: "no readable brain artifacts".into(),
        });
    }

    let uuid = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if title.is_none() && !uuid.is_empty() {
        title = Some(format!("Antigravity {}", short_uuid(uuid)));
    }

    let content_hash = conversation_hash(&Tool::Antigravity, &messages);
    Ok(Conversation {
        id: conversation_id(&content_hash),
        tool: Tool::Antigravity,
        kind: Kind::Thread,
        title,
        project: None,
        model: None,
        started_at: None,
        ended_at: None,
        git_context: GitContext::default(),
        source: Source {
            path: dir.to_string_lossy().into_owned(),
            offset: None,
            fingerprint: FINGERPRINT.into(),
        },
        content_hash,
        messages,
    })
}

fn first_heading(md: &str) -> Option<String> {
    md.lines()
        .map(str::trim)
        .find(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#').trim().to_string())
        .filter(|s| !s.is_empty())
}

fn short_uuid(uuid: &str) -> String {
    uuid.split('-').next().unwrap_or(uuid).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_brain_markdown() {
        let tmp = std::env::temp_dir().join(format!("ct-ag-{}", std::process::id()));
        let uuid_dir = tmp.join("11111111-2222-3333-4444-555555555555");
        std::fs::create_dir_all(&uuid_dir).unwrap();
        std::fs::write(
            uuid_dir.join("task.md"),
            "# Ship the indexer\nDo the thing.",
        )
        .unwrap();
        std::fs::write(uuid_dir.join("implementation_plan.md"), "Step 1.\nStep 2.").unwrap();

        let c = parse_brain_dir(&uuid_dir).unwrap();
        assert_eq!(c.tool, Tool::Antigravity);
        assert_eq!(c.title.as_deref(), Some("Ship the indexer"));
        assert_eq!(c.messages.len(), 2);
        // task.md sorts first.
        assert!(c.messages[0].content.contains("Ship the indexer"));
        std::fs::remove_dir_all(&tmp).ok();
    }
}
