//! Upgrading a pre-v5 database: session keys are backfilled and legacy
//! bookmark/pin flags are carried into the durable `user_state` table.

use ct_store::Store;
use rusqlite::Connection;

/// Build a v4-shaped database on disk (no `session_key`, flags stored on the
/// conversation row) with one bookmarked conversation, then return its path.
fn make_v4_db(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("v4.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        r#"
        PRAGMA user_version = 4;
        CREATE TABLE conversations (
            id TEXT PRIMARY KEY, tool TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'thread',
            project TEXT, model TEXT, started_at TEXT, ended_at TEXT,
            git_branch TEXT, git_commit TEXT, source_path TEXT NOT NULL,
            source_offset INTEGER, source_fingerprint TEXT NOT NULL,
            content_hash TEXT NOT NULL UNIQUE, title TEXT,
            message_count INTEGER NOT NULL, indexed_at TEXT NOT NULL,
            bookmarked INTEGER NOT NULL DEFAULT 0, pinned INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE messages (
            rowid INTEGER PRIMARY KEY, conversation_id TEXT NOT NULL,
            seq INTEGER NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL, timestamp TEXT
        );
        CREATE VIRTUAL TABLE messages_fts USING fts5(content, content='messages', content_rowid='rowid');
        CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
            INSERT INTO messages_fts(rowid, content) VALUES (new.rowid, new.content);
        END;
        CREATE TABLE embeddings (
            message_rowid INTEGER PRIMARY KEY, model TEXT NOT NULL, dim INTEGER NOT NULL, vec BLOB NOT NULL
        );
        INSERT INTO conversations
            (id, tool, kind, source_path, source_fingerprint, content_hash, title,
             message_count, indexed_at, bookmarked, pinned)
        VALUES
            ('cv_old', 'claude-code', 'thread', '/home/me/proj/session.jsonl',
             'claude-code/v1', 'hash_old', 'Old bookmarked thread', 1,
             '2026-05-01T00:00:00Z', 1, 0);
        INSERT INTO messages (conversation_id, seq, role, content)
        VALUES ('cv_old', 0, 'user', 'configure the postgres connection pool');
        "#,
    )
    .unwrap();
    path
}

#[test]
fn upgrades_v4_db_preserving_bookmarks_and_search() {
    let tmp = tempfile::tempdir().unwrap();
    let path = make_v4_db(tmp.path());

    // Open with the current code: this runs the v4 -> v5 migration + backfill.
    let store = Store::open(&path).unwrap();

    // The legacy bookmark survived into the durable user_state table.
    let saved = store.saved().unwrap();
    assert_eq!(saved.len(), 1, "the old bookmark should be preserved");
    assert_eq!(saved[0].conversation_id, "cv_old");
    assert!(saved[0].bookmarked);

    // The conversation still reads, now reporting the flag via the join.
    let convo = store.get_conversation("cv_old").unwrap().unwrap();
    assert!(convo.bookmarked);
    assert_eq!(convo.messages.len(), 1);

    // FTS search still works against the migrated rows, and carries the flag.
    let hits = store.search("postgres connection pool", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].conversation_id, "cv_old");
    assert!(hits[0].bookmarked);

    // Re-opening is idempotent (backfill is a no-op the second time).
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.saved().unwrap().len(), 1);
}
