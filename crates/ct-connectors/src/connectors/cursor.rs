//! Cursor connector.
//!
//! ## What Cursor stores, and where
//!
//! Cursor (a VS Code fork) persists chat history in SQLite databases named
//! `state.vscdb`:
//!
//! - **Global**: `<config>/Cursor/User/globalStorage/state.vscdb`
//! - **Per-workspace**: `<config>/Cursor/User/workspaceStorage/<hash>/state.vscdb`
//!   (a sibling `workspace.json` carries `{"folder":"file:///…"}` — our best
//!   source for the project path.)
//!
//! `<config>` is `~/.config` (Linux), `~/Library/Application Support` (macOS),
//! or `%APPDATA%` (Windows).
//!
//! ## Schema (reverse-engineered; unstable across versions)
//!
//! Each DB has key/value tables `ItemTable(key TEXT, value BLOB)` and — in the
//! global DB — `cursorDiskKV(key TEXT, value BLOB)`. Values are UTF-8 JSON.
//! Two chat formats coexist:
//!
//! 1. **Legacy chat** — `ItemTable["workbench.panel.aichat.view.aichat.chatdata"]`
//!    → `{"tabs":[{"tabId":…,"chatTitle"?:…,"bubbles":[{"type":"user"|"ai",
//!    "text":…}]}]}`. One tab = one conversation.
//! 2. **Composer** — `cursorDiskKV["composerData:<id>"]` →
//!    `{"composerId":…,"name"?:…,"createdAt"?:<ms>, "conversation":[{"bubbleId":…,
//!    "type":1|2,"text":…}]}`. Newer builds store headers only
//!    (`fullConversationHeadersOnly`) and put each bubble in a separate row
//!    `cursorDiskKV["bubbleId:<composerId>:<bubbleId>"]`. Bubble `type` is
//!    `1 = user`, `2 = assistant`.
//!
//! Because the format drifts, this connector is defensive: unknown shapes are
//! skipped, every conversation is stamped with a fingerprint (FR-ING-08), and
//! the JSON-shape logic is split into pure, unit-tested functions from the
//! SQLite access.

use std::path::{Path, PathBuf};

use ct_core::connector::{Connector, ConnectorError, DiscoveredSession, Result};
use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Message, Role, Source, Tool};

pub const FINGERPRINT: &str = "cursor/v1";

const LEGACY_CHATDATA_KEY: &str = "workbench.panel.aichat.view.aichat.chatdata";

#[derive(Debug, Default)]
pub struct CursorConnector;

impl CursorConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Connector for CursorConnector {
    fn tool(&self) -> Tool {
        Tool::Cursor
    }

    fn fingerprint(&self) -> &'static str {
        FINGERPRINT
    }

    fn roots(&self) -> Vec<PathBuf> {
        discover_db_files()
            .into_iter()
            .filter(|p| p.exists())
            .collect()
    }

    fn discover(&self) -> Result<Vec<DiscoveredSession>> {
        let mut out = Vec::new();
        for db in self.roots() {
            match discover_in_db(&db) {
                Ok(mut sessions) => out.append(&mut sessions),
                Err(e) => eprintln!("warn: cursor discover failed for {}: {e}", db.display()),
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
                reason: "cursor sessions require a locator".into(),
            })?;
        parse_in_db(&session.path, locator)
    }
}

// ---------------------------------------------------------------------------
// SQLite access
// ---------------------------------------------------------------------------

fn open_ro(path: &Path) -> Result<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| ConnectorError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::other(e.to_string()),
    })
}

/// Fetch a single value (as a JSON string) for `key` from `table`, if present.
fn get_value(conn: &rusqlite::Connection, table: &str, key: &str) -> Option<String> {
    let sql = format!("SELECT value FROM {table} WHERE key = ?1");
    let mut stmt = conn.prepare(&sql).ok()?;
    stmt.query_row([key], |row| {
        // Values are BLOBs holding UTF-8 JSON.
        let bytes: Vec<u8> = row.get(0)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })
    .ok()
}

/// Collect keys matching a `LIKE` pattern from `table`. Returns empty if the
/// table doesn't exist.
fn keys_like(conn: &rusqlite::Connection, table: &str, pattern: &str) -> Vec<String> {
    let sql = format!("SELECT key FROM {table} WHERE key LIKE ?1");
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let rows = stmt.query_map([pattern], |row| row.get::<_, String>(0));
    match rows {
        Ok(iter) => iter.filter_map(std::result::Result::ok).collect(),
        Err(_) => Vec::new(),
    }
}

/// List conversation locators in one DB. Locator forms:
/// - `itemtable/<tabId>`  (legacy chatdata)
/// - `diskkv/<composerId>` (composer)
pub fn discover_in_db(path: &Path) -> Result<Vec<DiscoveredSession>> {
    let conn = open_ro(path)?;
    let mut out = Vec::new();

    if let Some(raw) = get_value(&conn, "ItemTable", LEGACY_CHATDATA_KEY) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
            for tab in value
                .get("tabs")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
            {
                if let Some(tab_id) = tab.get("tabId").and_then(|v| v.as_str()) {
                    out.push(DiscoveredSession {
                        path: path.to_path_buf(),
                        locator: Some(format!("itemtable/{tab_id}")),
                    });
                }
            }
        }
    }

    for key in keys_like(&conn, "cursorDiskKV", "composerData:%") {
        if let Some(id) = key.strip_prefix("composerData:") {
            out.push(DiscoveredSession {
                path: path.to_path_buf(),
                locator: Some(format!("diskkv/{id}")),
            });
        }
    }

    Ok(out)
}

/// Parse one conversation identified by `locator` from `path`.
pub fn parse_in_db(path: &Path, locator: &str) -> Result<Conversation> {
    let conn = open_ro(path)?;
    let project = project_for_db(path);

    if let Some(tab_id) = locator.strip_prefix("itemtable/") {
        let raw = get_value(&conn, "ItemTable", LEGACY_CHATDATA_KEY).ok_or_else(|| {
            ConnectorError::Parse {
                path: path.to_path_buf(),
                reason: "legacy chatdata missing".into(),
            }
        })?;
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| ConnectorError::Parse {
                path: path.to_path_buf(),
                reason: format!("chatdata json: {e}"),
            })?;
        let tab = value
            .get("tabs")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .find(|t| t.get("tabId").and_then(|v| v.as_str()) == Some(tab_id))
            .ok_or_else(|| ConnectorError::Parse {
                path: path.to_path_buf(),
                reason: format!("tab {tab_id} not found"),
            })?;
        return tab_to_conversation(tab, path, project.clone()).ok_or_else(|| {
            ConnectorError::Parse {
                path: path.to_path_buf(),
                reason: "tab had no usable messages".into(),
            }
        });
    }

    if let Some(composer_id) = locator.strip_prefix("diskkv/") {
        let raw = get_value(
            &conn,
            "cursorDiskKV",
            &format!("composerData:{composer_id}"),
        )
        .ok_or_else(|| ConnectorError::Parse {
            path: path.to_path_buf(),
            reason: format!("composerData:{composer_id} missing"),
        })?;
        let composer: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| ConnectorError::Parse {
                path: path.to_path_buf(),
                reason: format!("composer json: {e}"),
            })?;
        let fetch_bubble = |bubble_id: &str| -> Option<serde_json::Value> {
            let key = format!("bubbleId:{composer_id}:{bubble_id}");
            get_value(&conn, "cursorDiskKV", &key).and_then(|s| serde_json::from_str(&s).ok())
        };
        return composer_to_conversation(&composer, fetch_bubble, path, project).ok_or_else(|| {
            ConnectorError::Parse {
                path: path.to_path_buf(),
                reason: "composer had no usable messages".into(),
            }
        });
    }

    Err(ConnectorError::Parse {
        path: path.to_path_buf(),
        reason: format!("unrecognized locator: {locator}"),
    })
}

// ---------------------------------------------------------------------------
// Pure JSON-shape parsing (unit-tested without SQLite)
// ---------------------------------------------------------------------------

/// Build a conversation from a legacy chat `tab`.
pub fn tab_to_conversation(
    tab: &serde_json::Value,
    path: &Path,
    project: Option<String>,
) -> Option<Conversation> {
    let bubbles = tab.get("bubbles").and_then(|v| v.as_array())?;
    let mut messages = Vec::new();
    for (idx, bubble) in bubbles.iter().enumerate() {
        let role = match bubble.get("type").and_then(|v| v.as_str()) {
            Some("user") => Role::User,
            Some("ai") | Some("assistant") => Role::Assistant,
            _ => continue,
        };
        if let Some(msg) = bubble_text_message(idx, bubble, role) {
            messages.push(msg);
        }
    }
    if messages.is_empty() {
        return None;
    }
    Some(finish(messages, project, None, path))
}

/// Build a conversation from a composer entry. `fetch_bubble` resolves a
/// `bubbleId` to its stored JSON for the headers-only layout.
pub fn composer_to_conversation(
    composer: &serde_json::Value,
    fetch_bubble: impl Fn(&str) -> Option<serde_json::Value>,
    path: &Path,
    project: Option<String>,
) -> Option<Conversation> {
    let mut messages = Vec::new();

    // Inline conversation array (older composer builds).
    if let Some(turns) = composer.get("conversation").and_then(|v| v.as_array()) {
        for (idx, turn) in turns.iter().enumerate() {
            if let Some(msg) = composer_turn_message(idx, turn) {
                messages.push(msg);
            }
        }
    }

    // Headers-only: resolve each bubble row separately.
    if messages.is_empty() {
        if let Some(headers) = composer
            .get("fullConversationHeadersOnly")
            .and_then(|v| v.as_array())
        {
            for (idx, header) in headers.iter().enumerate() {
                let Some(bubble_id) = header.get("bubbleId").and_then(|v| v.as_str()) else {
                    continue;
                };
                let Some(bubble) = fetch_bubble(bubble_id) else {
                    continue;
                };
                // Header carries the type; bubble row carries the text.
                let mut merged = bubble;
                if merged.get("type").is_none() {
                    if let Some(t) = header.get("type") {
                        merged["type"] = t.clone();
                    }
                }
                if let Some(msg) = composer_turn_message(idx, &merged) {
                    messages.push(msg);
                }
            }
        }
    }

    if messages.is_empty() {
        return None;
    }

    let started_at = composer
        .get("createdAt")
        .and_then(|v| v.as_i64())
        .and_then(chrono::DateTime::from_timestamp_millis);

    Some(finish(messages, project, started_at, path))
}

/// A composer turn uses numeric `type`: 1 = user, 2 = assistant.
fn composer_turn_message(idx: usize, turn: &serde_json::Value) -> Option<Message> {
    let role = match turn.get("type").and_then(|v| v.as_i64()) {
        Some(1) => Role::User,
        Some(2) => Role::Assistant,
        _ => return None,
    };
    bubble_text_message(idx, turn, role)
}

/// Shared: pull `text` out of a bubble/turn and make a [`Message`], skipping
/// empties (tool-only bubbles often have no text).
fn bubble_text_message(idx: usize, node: &serde_json::Value, role: Role) -> Option<Message> {
    let text = node
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return None;
    }
    let id = node
        .get("bubbleId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("ms_{idx}"));
    Some(Message {
        id,
        role,
        content: text.to_string(),
        timestamp: None,
        code_snippets: crate::text::extract_code_snippets(text),
        tool_calls: vec![],
        metadata: serde_json::Value::Null,
    })
}

fn finish(
    messages: Vec<Message>,
    project: Option<String>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    path: &Path,
) -> Conversation {
    let content_hash = conversation_hash(&Tool::Cursor, &messages);
    Conversation {
        id: conversation_id(&content_hash),
        tool: Tool::Cursor,
        project,
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
    }
}

// ---------------------------------------------------------------------------
// Path discovery
// ---------------------------------------------------------------------------

/// All `state.vscdb` files for Cursor on this machine (global + per-workspace).
fn discover_db_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for user_dir in cursor_user_dirs() {
        let global = user_dir.join("globalStorage").join("state.vscdb");
        if global.exists() {
            out.push(global);
        }
        let ws_root = user_dir.join("workspaceStorage");
        if let Ok(entries) = std::fs::read_dir(&ws_root) {
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

/// Candidate `Cursor/User` directories per OS. `CURSOR_CONFIG_DIR` overrides
/// (points at the `User` dir) for tests and non-standard installs.
fn cursor_user_dirs() -> Vec<PathBuf> {
    if let Ok(dir) = std::env::var("CURSOR_CONFIG_DIR") {
        return vec![PathBuf::from(dir)];
    }
    let mut dirs_out = Vec::new();
    if let Some(config) = dirs::config_dir() {
        // macOS: ~/Library/Application Support ; Linux: ~/.config ; Win: %APPDATA%
        dirs_out.push(config.join("Cursor").join("User"));
    }
    dirs_out
}

/// Resolve the workspace project path from a sibling `workspace.json`.
fn project_for_db(db_path: &Path) -> Option<String> {
    let ws_json = db_path.parent()?.join("workspace.json");
    let raw = std::fs::read_to_string(ws_json).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let folder = value.get("folder").and_then(|v| v.as_str())?;
    // Strip the file:// scheme for a clean path.
    Some(folder.strip_prefix("file://").unwrap_or(folder).to_string())
}
