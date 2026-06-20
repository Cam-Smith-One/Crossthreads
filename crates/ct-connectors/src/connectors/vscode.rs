//! Shared helpers for VS Code-family editors.
//!
//! Cline, GitHub Copilot Chat, and Windsurf all build on VS Code's storage
//! layout (`<config>/<Editor>/User/{globalStorage,workspaceStorage}`), so their
//! connectors share editor-directory discovery here. `<config>` is `~/.config`
//! on Linux, `~/Library/Application Support` on macOS, and `%APPDATA%` on
//! Windows (via [`dirs::config_dir`]).

use std::path::PathBuf;

/// Editor folder names we probe under the platform config dir. Covers stock VS
/// Code, Insiders, the MIT VSCodium build, and the Cursor/Windsurf forks (Cline
/// installs into any of them).
const EDITORS: &[&str] = &[
    "Code",
    "Code - Insiders",
    "VSCodium",
    "Cursor",
    "Windsurf",
    "Windsurf - Next",
];

/// `User` directories for every VS Code-family editor present on this machine.
/// `CT_VSCODE_USER_DIR` (colon-separated paths) overrides for tests and
/// non-standard installs.
pub fn family_user_dirs() -> Vec<PathBuf> {
    if let Ok(dir) = std::env::var("CT_VSCODE_USER_DIR") {
        return dir
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
    }
    let Some(config) = dirs::config_dir() else {
        return Vec::new();
    };
    EDITORS
        .iter()
        .map(|e| config.join(e).join("User"))
        .filter(|p| p.exists())
        .collect()
}

/// Open a `state.vscdb` read-only (it may be locked by a running editor).
pub fn open_vscdb_ro(path: &std::path::Path) -> Option<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()
}

/// Fetch a key/value entry (UTF-8 JSON, stored as TEXT or BLOB) from a vscdb
/// key/value table such as `ItemTable`.
pub fn vscdb_get(conn: &rusqlite::Connection, table: &str, key: &str) -> Option<String> {
    let sql = format!("SELECT value FROM {table} WHERE key = ?1");
    let mut stmt = conn.prepare(&sql).ok()?;
    stmt.query_row([key], |row| {
        use rusqlite::types::ValueRef;
        Ok(match row.get_ref(0)? {
            ValueRef::Text(b) | ValueRef::Blob(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        })
    })
    .ok()
    .flatten()
}
