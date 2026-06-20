//! Store integration: persistence, dedup, and FTS keyword search.

use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Message, Role, Source, Tool};
use ct_store::{Store, Upsert};

fn msg(role: Role, content: &str) -> Message {
    Message {
        id: "m".into(),
        role,
        content: content.into(),
        timestamp: None,
        code_snippets: vec![],
        tool_calls: vec![],
        metadata: serde_json::Value::Null,
    }
}

fn convo(tool: Tool, project: &str, messages: Vec<Message>) -> Conversation {
    let content_hash = conversation_hash(&tool, &messages);
    Conversation {
        id: conversation_id(&content_hash),
        tool,
        kind: ct_core::model::Kind::Thread,
        title: None,
        project: Some(project.into()),
        model: None,
        started_at: None,
        ended_at: None,
        git_context: GitContext::default(),
        source: Source {
            path: "/x".into(),
            offset: None,
            fingerprint: "test/v1".into(),
        },
        content_hash,
        messages,
    }
}

#[test]
fn inserts_then_dedups_on_content_hash() {
    let mut store = Store::open_in_memory().unwrap();
    let c = convo(
        Tool::ClaudeCode,
        "/repo",
        vec![msg(Role::User, "implement token refresh with backoff")],
    );

    assert_eq!(store.upsert_conversation(&c).unwrap(), Upsert::Inserted);
    // Re-inserting the identical conversation is a no-op.
    assert_eq!(store.upsert_conversation(&c).unwrap(), Upsert::Duplicate);
    assert_eq!(store.conversation_count().unwrap(), 1);
}

#[test]
fn keyword_search_finds_and_ranks() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .upsert_conversation(&convo(
            Tool::ClaudeCode,
            "/acme-api",
            vec![
                msg(Role::User, "the OAuth token refresh is looping on 401"),
                msg(
                    Role::Assistant,
                    "wrap the refresh in exponential backoff and retry",
                ),
            ],
        ))
        .unwrap();
    store
        .upsert_conversation(&convo(
            Tool::Cursor,
            "/acme-web",
            vec![msg(Role::User, "add a dark mode toggle to the navbar")],
        ))
        .unwrap();

    let hits = store.search("token refresh", 10).unwrap();
    assert_eq!(
        hits.len(),
        1,
        "only one conversation mentions token refresh"
    );
    assert_eq!(hits[0].project.as_deref(), Some("/acme-api"));
    assert!(hits[0].snippet.contains('['), "snippet should mark matches");

    // Unrelated query hits the other conversation.
    let hits = store.search("dark mode", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].tool, "cursor");

    // No match -> empty.
    assert!(store
        .search("kubernetes helm chart", 10)
        .unwrap()
        .is_empty());
}

#[test]
fn search_collapses_to_one_hit_per_conversation() {
    let mut store = Store::open_in_memory().unwrap();
    // Two messages both match "retry" — should still yield a single hit.
    store
        .upsert_conversation(&convo(
            Tool::Aider,
            "/svc",
            vec![
                msg(Role::User, "should we retry on failure?"),
                msg(Role::Assistant, "yes, retry with backoff"),
            ],
        ))
        .unwrap();

    let hits = store.search("retry", 10).unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn bookmark_and_pin_flags_round_trip() {
    let mut store = Store::open_in_memory().unwrap();
    let c = convo(
        Tool::ClaudeCode,
        "/repo",
        vec![msg(Role::User, "set up the deploy pipeline")],
    );
    store.upsert_conversation(&c).unwrap();

    // Nothing saved yet.
    assert!(store.saved().unwrap().is_empty());

    // Pin it, then bookmark it.
    assert!(store.set_flags(&c.id, None, Some(true)).unwrap());
    assert!(store.set_flags(&c.id, Some(true), None).unwrap());

    let saved = store.saved().unwrap();
    assert_eq!(saved.len(), 1);
    assert!(saved[0].pinned);
    assert!(saved[0].bookmarked);

    // Flags surface on the fetched conversation and in search hits.
    let full = store.get_conversation(&c.id).unwrap().unwrap();
    assert!(full.pinned && full.bookmarked);
    let hit = &store.search("deploy pipeline", 10).unwrap()[0];
    assert!(hit.pinned && hit.bookmarked);

    // Clearing the pin leaves only the bookmark; both clear -> not saved.
    assert!(store.set_flags(&c.id, None, Some(false)).unwrap());
    assert_eq!(store.saved().unwrap().len(), 1);
    assert!(store.set_flags(&c.id, Some(false), None).unwrap());
    assert!(store.saved().unwrap().is_empty());

    // Unknown id reports no row updated.
    assert!(!store.set_flags("nope", Some(true), None).unwrap());
}

#[test]
fn bookmark_survives_a_growing_conversation() {
    let mut store = Store::open_in_memory().unwrap();
    // A conversation, then bookmark it.
    let first = convo(
        Tool::ClaudeCode,
        "/repo",
        vec![msg(Role::User, "wire up the websocket reconnect")],
    );
    store.upsert_conversation(&first).unwrap();
    assert!(store.set_flags(&first.id, Some(true), None).unwrap());

    // The same session continues: same tool + source + first message, one more
    // turn. Different content hash -> a new row, but the SAME stable session key.
    let grown = convo(
        Tool::ClaudeCode,
        "/repo",
        vec![
            msg(Role::User, "wire up the websocket reconnect"),
            msg(Role::Assistant, "added exponential backoff on close"),
        ],
    );
    store.upsert_conversation(&grown).unwrap();
    assert_ne!(first.id, grown.id, "growing changes the content-hash id");

    // The bookmark follows the live (grown) conversation.
    let full = store.get_conversation(&grown.id).unwrap().unwrap();
    assert!(
        full.bookmarked,
        "bookmark carried over to the grown session"
    );

    // saved() collapses the two versions to one row.
    let saved = store.saved().unwrap();
    assert_eq!(saved.len(), 1);
}

#[test]
fn notes_and_tags_round_trip_and_filter() {
    let mut store = Store::open_in_memory().unwrap();
    let c = convo(
        Tool::ClaudeCode,
        "/repo",
        vec![msg(Role::User, "design the billing webhook")],
    );
    store.upsert_conversation(&c).unwrap();

    // Set a note and (messy, mixed-case, duplicate) tags — normalized on store.
    assert!(store.set_note(&c.id, "revisit idempotency").unwrap());
    assert!(store
        .set_tags(
            &c.id,
            &["Billing".into(), " billing ".into(), "Webhooks".into()]
        )
        .unwrap());

    let full = store.get_conversation(&c.id).unwrap().unwrap();
    assert_eq!(full.note, "revisit idempotency");
    assert_eq!(full.tags, vec!["billing", "webhooks"]);
    assert_eq!(store.facets_tags().unwrap(), vec!["billing", "webhooks"]);

    // Search hits carry tags, and a tag filter narrows results.
    let hit = &store.search("billing webhook", 10).unwrap()[0];
    assert!(hit.tags.contains(&"billing".to_string()));

    use ct_store::Filters;
    let by_tag = store
        .search_filtered(
            "billing webhook",
            10,
            &Filters {
                tag: Some("webhooks".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(by_tag.len(), 1);
    let none = store
        .search_filtered(
            "billing webhook",
            10,
            &Filters {
                tag: Some("nope".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(none.is_empty());
}

#[test]
fn forget_deletes_and_tombstones_across_reindex() {
    let mut store = Store::open_in_memory().unwrap();
    let c = convo(
        Tool::ClaudeCode,
        "/repo",
        vec![msg(Role::User, "rotate the leaked api credentials")],
    );
    assert_eq!(store.upsert_conversation(&c).unwrap(), Upsert::Inserted);
    assert_eq!(store.search("leaked api credentials", 10).unwrap().len(), 1);

    // Forget it: gone from the index.
    assert!(store.forget(&c.id).unwrap());
    assert_eq!(store.conversation_count().unwrap(), 0);
    assert!(store
        .search("leaked api credentials", 10)
        .unwrap()
        .is_empty());
    assert_eq!(store.forgotten_count().unwrap(), 1);

    // Re-indexing the same conversation does NOT bring it back (tombstoned).
    assert_eq!(store.upsert_conversation(&c).unwrap(), Upsert::Forgotten);
    assert_eq!(store.conversation_count().unwrap(), 0);

    // Forgetting an unknown id is a no-op false.
    assert!(!store.forget("cv_nope").unwrap());
}

#[test]
fn punctuation_in_query_does_not_break_fts() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .upsert_conversation(&convo(
            Tool::ClaudeCode,
            "/repo",
            vec![msg(Role::User, "fix the auth bug")],
        ))
        .unwrap();
    // Quotes/operators that would otherwise be FTS5 syntax.
    let hits = store.search("auth: (bug)!", 10).unwrap();
    assert_eq!(hits.len(), 1);
}
