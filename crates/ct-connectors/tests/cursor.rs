//! Integration test for the Cursor connector.
//!
//! Cursor isn't installed in CI, so we synthesize a `state.vscdb` that mirrors
//! the reverse-engineered schema (legacy chatdata + both composer layouts) and
//! drive the connector's real SQLite path against it.

use ct_connectors::connectors::cursor::{discover_in_db, parse_in_db};
use ct_core::model::Role;
use rusqlite::Connection;

/// Build a temp `state.vscdb` with the three conversation shapes we support.
fn make_db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.vscdb");

    // Sibling workspace.json so project resolution has something to find.
    std::fs::write(
        dir.path().join("workspace.json"),
        r#"{"folder":"file:///home/dev/acme-api"}"#,
    )
    .unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value BLOB);
         CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value BLOB);",
    )
    .unwrap();

    // 1) Legacy chatdata: one tab, two bubbles.
    let chatdata = r#"{"tabs":[{"tabId":"tab-1","chatTitle":"legacy",
        "bubbles":[
            {"type":"user","text":"how do I add retries?"},
            {"type":"ai","text":"Use exponential backoff:\n\n```rust\nbackoff();\n```"}
        ]}]}"#;
    put(&conn, "ItemTable", "workbench.panel.aichat.view.aichat.chatdata", chatdata);

    // 2) Composer with inline conversation array.
    let comp1 = r#"{"composerId":"comp-1","name":"inline","createdAt":1747200000000,
        "conversation":[
            {"bubbleId":"b1","type":1,"text":"inline user turn"},
            {"bubbleId":"b2","type":2,"text":"inline assistant turn"}
        ]}"#;
    put(&conn, "cursorDiskKV", "composerData:comp-1", comp1);

    // 3) Composer headers-only: bubbles live in separate rows.
    let comp2 = r#"{"composerId":"comp-2","name":"headers",
        "fullConversationHeadersOnly":[
            {"bubbleId":"h1","type":1},
            {"bubbleId":"h2","type":2}
        ]}"#;
    put(&conn, "cursorDiskKV", "composerData:comp-2", comp2);
    put(&conn, "cursorDiskKV", "bubbleId:comp-2:h1", r#"{"text":"headers user turn"}"#);
    put(&conn, "cursorDiskKV", "bubbleId:comp-2:h2", r#"{"text":"headers assistant turn"}"#);

    (dir, db_path)
}

fn put(conn: &Connection, table: &str, key: &str, value: &str) {
    let sql = format!("INSERT INTO {table} (key, value) VALUES (?1, ?2)");
    // Bind as bytes so the column is a BLOB, matching real Cursor.
    conn.execute(&sql, rusqlite::params![key, value.as_bytes()])
        .unwrap();
}

#[test]
fn discovers_all_three_conversations() {
    let (_dir, db) = make_db();
    let mut locators: Vec<String> = discover_in_db(&db)
        .unwrap()
        .into_iter()
        .filter_map(|s| s.locator)
        .collect();
    locators.sort();
    assert_eq!(
        locators,
        vec!["diskkv/comp-1", "diskkv/comp-2", "itemtable/tab-1"]
    );
}

#[test]
fn parses_legacy_chatdata_tab() {
    let (_dir, db) = make_db();
    let convo = parse_in_db(&db, "itemtable/tab-1").unwrap();

    assert_eq!(convo.tool.slug(), "cursor");
    assert_eq!(convo.source.fingerprint, "cursor/v1");
    assert_eq!(convo.project.as_deref(), Some("/home/dev/acme-api"));
    assert_eq!(
        convo.messages.iter().map(|m| m.role).collect::<Vec<_>>(),
        vec![Role::User, Role::Assistant]
    );
    // Code fence extracted from the assistant bubble.
    let snip = &convo.messages[1].code_snippets;
    assert_eq!(snip.len(), 1);
    assert_eq!(snip[0].language.as_deref(), Some("rust"));
}

#[test]
fn parses_inline_composer() {
    let (_dir, db) = make_db();
    let convo = parse_in_db(&db, "diskkv/comp-1").unwrap();
    assert_eq!(convo.messages.len(), 2);
    assert_eq!(convo.messages[0].role, Role::User);
    assert_eq!(convo.messages[0].content, "inline user turn");
    // createdAt (ms) lifted into started_at.
    assert!(convo.started_at.is_some());
}

#[test]
fn parses_headers_only_composer_via_bubble_rows() {
    let (_dir, db) = make_db();
    let convo = parse_in_db(&db, "diskkv/comp-2").unwrap();
    assert_eq!(
        convo.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
        vec!["headers user turn", "headers assistant turn"]
    );
    assert_eq!(
        convo.messages.iter().map(|m| m.role).collect::<Vec<_>>(),
        vec![Role::User, Role::Assistant]
    );
}
