//! SQLite schema: the single index DB (ADR-007).
//!
//! One file holds conversations, messages, and an FTS5 index over message
//! content. `sqlite-vec` embeddings will join this same DB later — keeping one
//! store to back up, sync, or purge.

/// Bumped when the schema changes; the store refuses to open a newer version.
pub const SCHEMA_VERSION: i64 = 2;

/// Executed once on open. Idempotent (`IF NOT EXISTS`); FTS5 stays in sync with
/// `messages` via triggers so callers only ever touch the base table.
pub const SCHEMA_SQL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS conversations (
    id                 TEXT PRIMARY KEY,
    tool               TEXT NOT NULL,
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
    indexed_at         TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_conversations_tool    ON conversations(tool);
CREATE INDEX IF NOT EXISTS idx_conversations_project ON conversations(project);

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
"#;
