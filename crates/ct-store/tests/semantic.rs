//! Vector storage + semantic + hybrid (RRF) search.
//!
//! Uses the deterministic [`HashEmbedder`] so the test needs no model/network.
//! It captures lexical overlap, which is enough to verify the storage, cosine
//! ranking, and RRF fusion mechanics end-to-end.

use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Message, Role, Source, Tool};
use ct_embed::{Embedder, HashEmbedder};
use ct_store::Store;

fn msg(content: &str) -> Message {
    Message {
        id: "m".into(),
        role: Role::User,
        content: content.into(),
        timestamp: None,
        code_snippets: vec![],
        tool_calls: vec![],
        metadata: serde_json::Value::Null,
    }
}

fn convo(tool: Tool, project: &str, contents: &[&str]) -> Conversation {
    let messages: Vec<Message> = contents.iter().map(|c| msg(c)).collect();
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

/// Embed every pending message with the given embedder (mirrors what the CLI
/// does after indexing).
fn embed_all(store: &mut Store, embedder: &dyn Embedder) {
    let pending = store.pending_embeddings(10_000).unwrap();
    let texts: Vec<String> = pending.iter().map(|(_, c)| c.clone()).collect();
    let vecs = embedder.embed(&texts).unwrap();
    let rows: Vec<(i64, Vec<f32>)> = pending.iter().map(|(rowid, _)| *rowid).zip(vecs).collect();
    store.store_embeddings(embedder.id(), &rows).unwrap();
}

fn seed() -> (Store, HashEmbedder) {
    let mut store = Store::open_in_memory().unwrap();
    store
        .upsert_conversation(&convo(
            Tool::ClaudeCode,
            "/acme-api",
            &["add retry logic with exponential backoff to the http client"],
        ))
        .unwrap();
    store
        .upsert_conversation(&convo(
            Tool::Cursor,
            "/acme-web",
            &["add a dark mode toggle to the navbar with a theme hook"],
        ))
        .unwrap();
    let embedder = HashEmbedder::default();
    embed_all(&mut store, &embedder);
    (store, embedder)
}

#[test]
fn embeddings_are_stored_for_every_message() {
    let (store, _) = seed();
    assert_eq!(store.embedding_count().unwrap(), 2);
    // Nothing left pending.
    assert!(store.pending_embeddings(100).unwrap().is_empty());
}

#[test]
fn semantic_search_ranks_by_cosine() {
    let (store, embedder) = seed();
    let q = embedder.embed_one("retry with backoff").unwrap();
    let hits = store.search_semantic(&q, 10).unwrap();
    assert!(!hits.is_empty());
    // The retry/backoff conversation should rank first.
    assert_eq!(hits[0].project.as_deref(), Some("/acme-api"));
}

#[test]
fn hybrid_fuses_lexical_and_semantic() {
    let (store, embedder) = seed();
    let q = embedder.embed_one("backoff retry").unwrap();
    let hits = store.search_hybrid("backoff retry", &q, 10).unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].project.as_deref(), Some("/acme-api"));
    // Fused score is positive (RRF sums reciprocal ranks).
    assert!(hits[0].score > 0.0);
}

#[test]
fn hybrid_finds_lexical_only_match_when_vectors_are_weak() {
    // A rare exact token the embedder can't relate semantically still surfaces
    // via the lexical half of the fusion.
    let (store, embedder) = seed();
    let q = embedder.embed_one("navbar").unwrap();
    let hits = store.search_hybrid("navbar", &q, 10).unwrap();
    assert!(hits
        .iter()
        .any(|h| h.project.as_deref() == Some("/acme-web")));
}

#[test]
fn semantic_scan_scales_and_stays_correct() {
    // Build a corpus larger than the parallel-scan threshold (8_192) so the
    // parallel path is exercised, with one planted "needle" conversation whose
    // vector matches the query and a big "haystack" of orthogonal vectors.
    let mut store = Store::open_in_memory().unwrap();

    // Haystack: one conversation with many short messages.
    let n = 12_000usize;
    let haystack: Vec<String> = (0..n)
        .map(|i| format!("noise message number {i}"))
        .collect();
    let haystack_refs: Vec<&str> = haystack.iter().map(String::as_str).collect();
    store
        .upsert_conversation(&convo(Tool::ClaudeCode, "/haystack", &haystack_refs))
        .unwrap();
    // Needle: its own conversation with a single, distinctive message.
    store
        .upsert_conversation(&convo(
            Tool::Cursor,
            "/needle",
            &["the planted needle message"],
        ))
        .unwrap();

    // Assign vectors directly (dim 16): the needle points at dim 0 (matching the
    // query); every haystack vector points at a non-zero dim, so its cosine with
    // the query is 0 and it's filtered by MIN_SIMILARITY.
    let pending = store.pending_embeddings(100_000).unwrap();
    let rows: Vec<(i64, Vec<f32>)> = pending
        .iter()
        .map(|(rowid, content)| {
            let mut v = vec![0.0f32; 16];
            if content.contains("planted needle") {
                v[0] = 1.0;
            } else {
                v[1 + (*rowid as usize % 15)] = 1.0;
            }
            (*rowid, v)
        })
        .collect();
    store.store_embeddings("test", &rows).unwrap();
    assert!(store.embedding_count().unwrap() as usize >= n);

    // Query aligned with the needle.
    let mut q = vec![0.0f32; 16];
    q[0] = 1.0;
    let start = std::time::Instant::now();
    let hits = store.search_semantic(&q, 5).unwrap();
    let elapsed = start.elapsed();

    // Only the needle clears the similarity floor; it ranks first.
    assert_eq!(hits.len(), 1, "only the needle should clear MIN_SIMILARITY");
    assert_eq!(hits[0].project.as_deref(), Some("/needle"));
    // Generous bound: a 12k-vector scan should be well under this even in CI.
    assert!(elapsed.as_secs() < 5, "scan too slow: {elapsed:?}");
}
