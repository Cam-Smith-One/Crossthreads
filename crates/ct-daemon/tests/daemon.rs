//! Integration: start the daemon on an ephemeral port and drive it over the
//! wire with the client. Uses the deterministic hash embedder (no model).

use std::net::TcpListener;
use std::thread;

use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Message, Role, Source, Tool};
use ct_daemon::{Client, Daemon, Mode, Request, Response};
use ct_embed::HashEmbedder;
use ct_store::Store;

fn convo(tool: Tool, project: &str, text: &str) -> Conversation {
    let messages = vec![Message {
        id: "m".into(),
        role: Role::User,
        content: text.into(),
        timestamp: None,
        code_snippets: vec![],
        tool_calls: vec![],
        metadata: serde_json::Value::Null,
    }];
    let content_hash = conversation_hash(&tool, &messages);
    Conversation {
        id: conversation_id(&content_hash),
        tool,
        project: Some(project.into()),
        model: None,
        started_at: None,
        ended_at: None,
        git_context: GitContext::default(),
        source: Source { path: "/x".into(), offset: None, fingerprint: "t/v1".into() },
        content_hash,
        messages,
    }
}

/// Seed an in-memory store and start a daemon serving it on a random port.
fn start() -> (String, thread::JoinHandle<()>) {
    let mut store = Store::open_in_memory().unwrap();
    store.upsert_conversation(&convo(Tool::ClaudeCode, "/acme-api", "add retry logic with backoff")).unwrap();
    store.upsert_conversation(&convo(Tool::Cursor, "/acme-web", "dark mode toggle for the navbar")).unwrap();
    ct_index::embed_pending(&mut store, &HashEmbedder::default()).unwrap();

    let daemon = Daemon::new(store, Box::new(HashEmbedder::default()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let handle = thread::spawn(move || {
        let _ = daemon.serve(listener);
    });
    (addr, handle)
}

#[test]
fn ping_status_and_search_over_the_wire() {
    let (addr, _h) = start();
    let client = Client::new(addr);

    // Ping
    assert!(matches!(client.call(&Request::Ping).unwrap(), Response::Pong));

    // Status reflects the seeded data.
    match client.call(&Request::Status).unwrap() {
        Response::Status { conversations, embeddings, embedder } => {
            assert_eq!(conversations, 2);
            assert_eq!(embeddings, 2);
            assert_eq!(embedder, "hash-bow-v1");
        }
        other => panic!("unexpected: {other:?}"),
    }

    // Hybrid search returns the relevant conversation.
    let hits = client.search("retry backoff", Mode::Hybrid, 10, Default::default()).unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].project.as_deref(), Some("/acme-api"));

    // Lexical search for the other one.
    let hits = client.search("navbar", Mode::Lexical, 10, Default::default()).unwrap();
    assert!(hits.iter().any(|h| h.tool == "cursor"));
}

#[test]
fn multiple_clients_are_served() {
    let (addr, _h) = start();
    let mut handles = Vec::new();
    for _ in 0..4 {
        let addr = addr.clone();
        handles.push(thread::spawn(move || {
            let client = Client::new(addr);
            client.search("retry", Mode::Hybrid, 5, Default::default()).unwrap().len()
        }));
    }
    for h in handles {
        assert!(h.join().unwrap() >= 1);
    }
}
