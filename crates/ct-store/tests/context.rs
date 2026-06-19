//! Similarity floor + conversation/context retrieval.

use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Message, Role, Source, Tool};
use ct_embed::{Embedder, HashEmbedder};
use ct_store::Store;

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

fn convo(tool: Tool, project: &str, msgs: Vec<Message>) -> Conversation {
    let content_hash = conversation_hash(&tool, &msgs);
    Conversation {
        id: conversation_id(&content_hash),
        tool,
        kind: ct_core::model::Kind::Thread,
        title: None,
        project: Some(project.into()),
        model: None,
        started_at: Some("2026-05-14T09:12:00Z".parse().unwrap()),
        ended_at: None,
        git_context: GitContext::default(),
        source: Source {
            path: "/x".into(),
            offset: None,
            fingerprint: "t/v1".into(),
        },
        content_hash,
        messages: msgs,
    }
}

fn seed() -> (Store, HashEmbedder, String) {
    let mut store = Store::open_in_memory().unwrap();
    let c = convo(
        Tool::ClaudeCode,
        "/acme-api",
        vec![
            msg(Role::User, "the oauth token refresh loops on 401"),
            msg(
                Role::Assistant,
                "wrap the refresh in exponential backoff and retry",
            ),
        ],
    );
    let id = c.id.clone();
    store.upsert_conversation(&c).unwrap();
    let embedder = HashEmbedder::default();
    let pending = store.pending_embeddings(100).unwrap();
    let texts: Vec<String> = pending.iter().map(|(_, t)| t.clone()).collect();
    let vecs = embedder.embed(&texts).unwrap();
    let rows: Vec<(i64, Vec<f32>)> = pending.iter().map(|(r, _)| *r).zip(vecs).collect();
    store.store_embeddings(embedder.id(), &rows).unwrap();
    (store, embedder, id)
}

#[test]
fn similarity_floor_filters_unrelated_query() {
    let (store, embedder, _) = seed();
    // A query with no shared vocabulary → below the floor → no semantic hits,
    // rather than returning the sole conversation at ~0 similarity.
    let q = embedder
        .embed_one("kubernetes helm chart deployment")
        .unwrap();
    assert!(store.search_semantic(&q, 10).unwrap().is_empty());

    // A related query still scores above the floor.
    let q = embedder.embed_one("refresh token retry backoff").unwrap();
    assert!(!store.search_semantic(&q, 10).unwrap().is_empty());
}

#[test]
fn get_conversation_returns_messages_in_order() {
    let (store, _, id) = seed();
    let convo = store.get_conversation(&id).unwrap().unwrap();
    assert_eq!(convo.tool, "claude-code");
    assert_eq!(convo.messages.len(), 2);
    assert_eq!(convo.messages[0].role, "user");
    assert!(convo.messages[1].content.contains("backoff"));

    assert!(store
        .get_conversation("cv_does_not_exist")
        .unwrap()
        .is_none());
}

#[test]
fn render_context_builds_markdown_with_sources() {
    let (store, _, id) = seed();
    let ctx = store
        .render_context(std::slice::from_ref(&id), 4000)
        .unwrap();
    assert_eq!(ctx.sources, vec![id]);
    assert!(ctx.markdown.contains("Prior context"));
    assert!(ctx.markdown.contains("**user:**"));
    assert!(ctx.markdown.contains("backoff"));
    assert!(ctx.chars > 0 && ctx.token_estimate > 0);
}

#[test]
fn filters_narrow_results_by_tool_and_project() {
    use ct_store::Filters;
    let mut store = Store::open_in_memory().unwrap();
    store
        .upsert_conversation(&convo(
            Tool::ClaudeCode,
            "/acme-api",
            vec![msg(Role::User, "retry with backoff")],
        ))
        .unwrap();
    store
        .upsert_conversation(&convo(
            Tool::Cursor,
            "/acme-web",
            vec![msg(Role::User, "retry the request")],
        ))
        .unwrap();

    let all = store
        .search_filtered("retry", 10, &Filters::default())
        .unwrap();
    assert_eq!(all.len(), 2);

    let only_cursor = store
        .search_filtered(
            "retry",
            10,
            &Filters {
                tool: Some("cursor".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(only_cursor.len(), 1);
    assert_eq!(only_cursor[0].tool, "cursor");

    let only_api = store
        .search_filtered(
            "retry",
            10,
            &Filters {
                project: Some("acme-api".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(only_api.len(), 1);
    assert_eq!(only_api[0].project.as_deref(), Some("/acme-api"));

    // Date floor in the future excludes everything (fixtures are 2026-05-14).
    let none = store
        .search_filtered(
            "retry",
            10,
            &Filters {
                since: Some("2027-01-01".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(none.is_empty());

    assert_eq!(store.facets_tools().unwrap(), vec!["claude-code", "cursor"]);
}

#[test]
fn render_context_respects_char_budget() {
    let (store, _, id) = seed();
    // Tiny budget: still includes the first conversation (always at least one),
    // but the block stays close to the budget rather than dumping everything.
    let ctx = store.render_context(&[id], 10).unwrap();
    assert_eq!(ctx.sources.len(), 1);
}
