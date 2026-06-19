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
                msg(Role::Assistant, "wrap the refresh in exponential backoff and retry"),
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
    assert_eq!(hits.len(), 1, "only one conversation mentions token refresh");
    assert_eq!(hits[0].project.as_deref(), Some("/acme-api"));
    assert!(hits[0].snippet.contains('['), "snippet should mark matches");

    // Unrelated query hits the other conversation.
    let hits = store.search("dark mode", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].tool, "cursor");

    // No match -> empty.
    assert!(store.search("kubernetes helm chart", 10).unwrap().is_empty());
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
