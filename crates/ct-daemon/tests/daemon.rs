//! Integration: start the daemon on an ephemeral port and drive it over the
//! wire with the client. Uses the deterministic hash embedder (no model).

use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Message, Role, Source, Tool};
use ct_daemon::{Client, Daemon, Federation, Mode, Peer, Request, Response};
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
            fingerprint: "t/v1".into(),
        },
        content_hash,
        messages,
    }
}

/// Seed an in-memory store and start a daemon serving it on a random port.
fn start() -> (String, thread::JoinHandle<()>) {
    let mut store = Store::open_in_memory().unwrap();
    store
        .upsert_conversation(&convo(
            Tool::ClaudeCode,
            "/acme-api",
            "add retry logic with backoff",
        ))
        .unwrap();
    store
        .upsert_conversation(&convo(
            Tool::Cursor,
            "/acme-web",
            "dark mode toggle for the navbar",
        ))
        .unwrap();
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
    assert!(matches!(
        client.call(&Request::Ping).unwrap(),
        Response::Pong
    ));

    // Status reflects the seeded data.
    match client.call(&Request::Status).unwrap() {
        Response::Status {
            conversations,
            embeddings,
            embedder,
        } => {
            assert_eq!(conversations, 2);
            assert_eq!(embeddings, 2);
            assert_eq!(embedder, "hash-bow-v1");
        }
        other => panic!("unexpected: {other:?}"),
    }

    // Hybrid search returns the relevant conversation.
    let hits = client
        .search("retry backoff", Mode::Hybrid, 10, Default::default())
        .unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].project.as_deref(), Some("/acme-api"));

    // Lexical search for the other one.
    let hits = client
        .search("navbar", Mode::Lexical, 10, Default::default())
        .unwrap();
    assert!(hits.iter().any(|h| h.tool == "cursor"));
}

// ---- Cross-device federation (ADR-010) --------------------------------------

/// Seed an in-memory store with the given conversations and embed them.
fn seed(convos: &[Conversation]) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    for c in convos {
        store.upsert_conversation(c).unwrap();
    }
    ct_index::embed_pending(&mut store, &HashEmbedder::default()).unwrap();
    store
}

/// Serve a daemon on a random loopback port; return its address.
fn serve(daemon: Daemon) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    thread::spawn(move || {
        let _ = daemon.serve(listener);
    });
    addr
}

fn fed(device: &str, token: &str, peers: Vec<Peer>) -> Federation {
    Federation {
        device: device.into(),
        token: Some(token.into()),
        peers,
        timeout: Duration::from_millis(1500),
    }
}

#[test]
fn federated_search_merges_and_tags_peer_hits() {
    // Device B (linux) is the only one that knows about "navbar".
    let b = Daemon::new(
        seed(&[convo(
            Tool::Cursor,
            "/web",
            "dark mode toggle for the navbar",
        )]),
        Box::new(HashEmbedder::default()),
    )
    .with_federation(fed("linux", "s3cret", vec![]));
    let b_addr = serve(b);

    // Device A (mac) knows "retry" and peers to B with a matching token.
    let a = Daemon::new(
        seed(&[convo(
            Tool::ClaudeCode,
            "/api",
            "add retry logic with backoff",
        )]),
        Box::new(HashEmbedder::default()),
    )
    .with_federation(fed(
        "mac",
        "s3cret",
        vec![Peer {
            name: "linux".into(),
            addr: b_addr,
        }],
    ));
    let client = Client::new(serve(a));

    // Searching A surfaces B's thread, tagged with B's configured device name.
    let hits = client
        .search("navbar", Mode::Lexical, 10, Default::default())
        .unwrap();
    let peer_hit = hits
        .iter()
        .find(|h| h.tool == "cursor")
        .expect("peer hit present");
    assert_eq!(peer_hit.device.as_deref(), Some("linux"));

    // A local hit carries A's own device name.
    let hits = client
        .search("retry backoff", Mode::Hybrid, 10, Default::default())
        .unwrap();
    assert_eq!(hits[0].device.as_deref(), Some("mac"));
}

#[test]
fn federated_duplicate_collapses_local_first() {
    // The same conversation exists on both devices (identical content → identical
    // id), so it must collapse to one result, keeping the local copy.
    let shared = convo(Tool::ClaudeCode, "/api", "notes on the caching layer");

    let b = Daemon::new(
        seed(std::slice::from_ref(&shared)),
        Box::new(HashEmbedder::default()),
    )
    .with_federation(fed("linux", "s3cret", vec![]));
    let b_addr = serve(b);

    let a = Daemon::new(
        seed(std::slice::from_ref(&shared)),
        Box::new(HashEmbedder::default()),
    )
    .with_federation(fed(
        "mac",
        "s3cret",
        vec![Peer {
            name: "linux".into(),
            addr: b_addr,
        }],
    ));
    let client = Client::new(serve(a));

    let hits = client
        .search("caching layer", Mode::Lexical, 10, Default::default())
        .unwrap();
    let dupes: Vec<_> = hits
        .iter()
        .filter(|h| h.conversation_id == shared.id)
        .collect();
    assert_eq!(dupes.len(), 1, "duplicate across devices should collapse");
    assert_eq!(dupes[0].device.as_deref(), Some("mac"), "local copy kept");
}

#[test]
fn federated_bad_token_skips_peer() {
    let b = Daemon::new(
        seed(&[convo(
            Tool::Cursor,
            "/web",
            "dark mode toggle for the navbar",
        )]),
        Box::new(HashEmbedder::default()),
    )
    .with_federation(fed("linux", "right-token", vec![]));
    let b_addr = serve(b);

    // A presents the wrong token, so B rejects and its hits are skipped — the
    // search still succeeds with whatever A has locally (nothing for "navbar").
    let a = Daemon::new(
        seed(&[convo(
            Tool::ClaudeCode,
            "/api",
            "add retry logic with backoff",
        )]),
        Box::new(HashEmbedder::default()),
    )
    .with_federation(fed(
        "mac",
        "wrong-token",
        vec![Peer {
            name: "linux".into(),
            addr: b_addr,
        }],
    ));
    let client = Client::new(serve(a));

    let hits = client
        .search("navbar", Mode::Lexical, 10, Default::default())
        .unwrap();
    assert!(
        hits.iter().all(|h| h.tool != "cursor"),
        "unauthorized peer must not contribute results"
    );
}

#[test]
fn multiple_clients_are_served() {
    let (addr, _h) = start();
    let mut handles = Vec::new();
    for _ in 0..4 {
        let addr = addr.clone();
        handles.push(thread::spawn(move || {
            let client = Client::new(addr);
            client
                .search("retry", Mode::Hybrid, 5, Default::default())
                .unwrap()
                .len()
        }));
    }
    for h in handles {
        assert!(h.join().unwrap() >= 1);
    }
}
