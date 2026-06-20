//! SQLite schema: the single index DB (ADR-007).
//!
//! One file holds conversations, messages, and an FTS5 index over message
//! content. `sqlite-vec` embeddings will join this same DB later — keeping one
//! store to back up, sync, or purge.

/// Bumped when the schema changes; the store refuses to open a newer version.
pub const SCHEMA_VERSION: i64 = 6;

/// Executed once on open. Idempotent (`IF NOT EXISTS`); FTS5 stays in sync with
/// `messages` via triggers so callers only ever touch the base table.
pub const SCHEMA_SQL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS conversations (
    id                 TEXT PRIMARY KEY,
    tool               TEXT NOT NULL,
    kind               TEXT NOT NULL DEFAULT 'thread',
    project            TEXT,
    model              TEXT,
    started_at         TEXT,
    ended_at           TEXT,
    git_branch         TEXT,
    git_commit         TEXT,
    source_path        TEXT NOT NULL,
    source_offset      INTEGER,
    source_fingerprint TEXT NOT NULL,
    content_hash       TEXT NOT NULL UNIQUE,
    title              TEXT,
    message_count      INTEGER NOT NULL,
    indexed_at         TEXT NOT NULL,
    -- Legacy (schema v4) flag columns. User state now lives in `user_state`
    -- keyed by `session_key`; these remain only so old rows still read.
    bookmarked         INTEGER NOT NULL DEFAULT 0,
    pinned             INTEGER NOT NULL DEFAULT 0,
    -- Stable identity for user state: constant as a conversation grows, unlike
    -- the content-hash `id`. See ct_core::hash::session_key.
    session_key        TEXT NOT NULL DEFAULT ''
);

-- Indexes on columns that have existed since v1. Indexes on columns added by
-- migration (`kind`, `session_key`) are created in code AFTER those ALTERs run,
-- because on an upgrading DB the column does not exist yet at this point.
CREATE INDEX IF NOT EXISTS idx_conversations_tool    ON conversations(tool);
CREATE INDEX IF NOT EXISTS idx_conversations_project ON conversations(project);

-- Durable user state (bookmarks, pins), kept SEPARATE from the rebuildable
-- index and keyed by the stable `session_key`. Deleting/rebuilding the index
-- does not touch this table, and a continuing conversation keeps its flags.
CREATE TABLE IF NOT EXISTS user_state (
    session_key TEXT PRIMARY KEY,
    bookmarked  INTEGER NOT NULL DEFAULT 0,
    pinned      INTEGER NOT NULL DEFAULT 0,
    updated_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    rowid           INTEGER PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    role            TEXT NOT NULL,
    content         TEXT NOT NULL,
    timestamp       TEXT
);

CREATE INDEX IF NOT EXISTS idx_messages_convo ON messages(conversation_id);

-- External-content FTS5 over message bodies; kept in sync by the triggers below.
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    content,
    content='messages',
    content_rowid='rowid'
);

CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', old.rowid, old.content);
END;

CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', old.rowid, old.content);
    INSERT INTO messages_fts(rowid, content) VALUES (new.rowid, new.content);
END;

-- Semantic vectors, one per message, in the same DB (ADR-007). Stored as raw
-- little-endian f32 BLOBs; brute-force cosine KNN for now, with sqlite-vec ANN
-- as a drop-in once corpora outgrow a linear scan.
CREATE TABLE IF NOT EXISTS embeddings (
    message_rowid INTEGER PRIMARY KEY REFERENCES messages(rowid) ON DELETE CASCADE,
    model         TEXT NOT NULL,
    dim           INTEGER NOT NULL,
    vec           BLOB NOT NULL
);

-- Tombstones for "forget this thread" (FR-PRIV). Keyed by the stable
-- session_key so a forgotten conversation stays forgotten across re-indexing,
-- even though its source file still exists on disk.
CREATE TABLE IF NOT EXISTS forgotten (
    session_key  TEXT PRIMARY KEY,
    forgotten_at TEXT NOT NULL
);
"#;
