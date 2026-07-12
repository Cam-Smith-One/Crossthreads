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
            tools,
        } => {
            assert_eq!(conversations, 2);
            assert_eq!(embeddings, 2);
            assert_eq!(embedder, "hash-bow-v1");
            // Two tools seeded (claude-code + cursor), one thread each.
            assert_eq!(tools.len(), 2);
            assert!(tools.iter().any(|(t, n, _)| t == "claude-code" && *n == 1));
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

#[test]
fn themes_cluster_over_the_wire() {
    let (addr, _h) = start();
    let client = Client::new(addr);

    // `name: false` needs no model — pure clustering must work everywhere.
    match client
        .call(&Request::Themes {
            k: Some(2),
            name: false,
        })
        .unwrap()
    {
        Response::Themes { themes } => {
            assert!(!themes.is_empty(), "two seeded convos should cluster");
            let total: usize = themes.iter().map(|t| t.size).sum();
            assert_eq!(total, 2);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

/// A dated conversation (so it shows up in the temporal view).
fn dated(tool: Tool, project: &str, text: &str, date: &str) -> Conversation {
    let mut c = convo(tool, project, text);
    c.started_at = Some(format!("{date}T09:00:00Z").parse().unwrap());
    c
}

#[test]
fn activity_and_graph_over_the_wire() {
    let mut store = Store::open_in_memory().unwrap();
    for c in [
        dated(
            Tool::ClaudeCode,
            "/code/acme-api",
            "retry one",
            "2026-06-01",
        ),
        dated(
            Tool::ClaudeCode,
            "/code/acme-api",
            "retry two",
            "2026-06-01",
        ),
        dated(
            Tool::Cursor,
            "/code/acme-web",
            "navbar dark mode",
            "2026-06-08",
        ),
    ] {
        store.upsert_conversation(&c).unwrap();
    }
    ct_index::embed_pending(&mut store, &HashEmbedder::default()).unwrap();
    let addr = serve(Daemon::new(store, Box::new(HashEmbedder::default())));
    let client = Client::new(addr);

    // Activity by day: two buckets, the first with two sessions.
    match client
        .call(&Request::Activity {
            bucket: Some("day".into()),
            filters: Default::default(),
        })
        .unwrap()
    {
        Response::Activity { bucket, periods } => {
            assert_eq!(bucket, "day");
            assert_eq!(periods.len(), 2, "two distinct days");
            let first = periods.iter().find(|p| p.period == "2026-06-01").unwrap();
            assert_eq!(first.total, 2);
            assert_eq!(first.by_tool[0].0, "claude-code");
        }
        other => panic!("unexpected: {other:?}"),
    }

    // Graph: project + tool nodes, with project↔tool edges.
    match client.call(&Request::Graph { limit: Some(40) }).unwrap() {
        Response::Graph { graph } => {
            assert!(graph.nodes.iter().any(|n| n.kind == "project"));
            assert!(graph
                .nodes
                .iter()
                .any(|n| n.kind == "tool" && n.label == "claude-code"));
            assert!(graph
                .edges
                .iter()
                .any(|e| e.source.starts_with("project:") && e.target.starts_with("tool:")));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn weekly_review_caches_and_returns() {
    let (addr, _h) = start();
    let client = Client::new(addr);

    // First call generates (fresh = true) and returns markdown + a period.
    let first = match client
        .call(&Request::WeeklyReview { force: false })
        .unwrap()
    {
        Response::WeeklyReview {
            markdown,
            period_start,
            fresh,
            ..
        } => {
            assert!(fresh, "first call should generate");
            assert!(!markdown.trim().is_empty());
            assert_eq!(period_start.len(), 10, "YYYY-MM-DD period");
            markdown
        }
        other => panic!("unexpected: {other:?}"),
    };

    // Second call (not forced) returns the cached one (fresh = false), same body.
    match client
        .call(&Request::WeeklyReview { force: false })
        .unwrap()
    {
        Response::WeeklyReview {
            markdown, fresh, ..
        } => {
            assert!(!fresh, "second call should be cached");
            assert_eq!(markdown, first);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn optimize_and_trends_over_the_wire() {
    let (addr, _h) = start();
    let client = Client::new(addr);

    // Optimize returns a report (deterministic; no model). The seeded convos are
    // undated so the 30-day window is empty — that's a valid, clean response.
    match client.call(&Request::Optimize { force: false }).unwrap() {
        Response::Optimize { markdown, .. } => assert!(!markdown.trim().is_empty()),
        other => panic!("unexpected: {other:?}"),
    }
    // Trends returns one row per tracked metric.
    match client.call(&Request::Trends { days: Some(30) }).unwrap() {
        Response::Trends { rows } => assert!(!rows.is_empty()),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn fluency_over_the_wire() {
    // One dated session inside the window so the four dimensions actually score.
    let mut store = Store::open_in_memory().unwrap();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let convo = dated(
        Tool::ClaudeCode,
        "/code/api",
        "add a retry with backoff, it must be idempotent, and write a test",
        &today,
    );
    store.upsert_conversation(&convo).unwrap();
    ct_index::embed_pending(&mut store, &HashEmbedder::default()).unwrap();
    let addr = serve(Daemon::new(store, Box::new(HashEmbedder::default())));
    let client = Client::new(addr);

    match client
        .call(&Request::Fluency { window_days: None })
        .unwrap()
    {
        Response::Fluency { report } => {
            assert_eq!(report.sessions, 1);
            assert_eq!(report.dimensions.len(), 4, "the four fluency dimensions");
            assert!(report
                .dimensions
                .iter()
                .any(|d| d.key == "description" && d.score > 0));
            // A reflection is always a question, never a directive.
            let r = report.reflection.expect("a reflection should surface");
            assert!(r.prompt.contains('?'));
            assert!(report.markdown.contains("AI-fluency"));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn glance_over_the_wire() {
    // A dated session today so usage + insight both populate.
    let mut store = Store::open_in_memory().unwrap();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    store
        .upsert_conversation(&dated(
            Tool::ClaudeCode,
            "/code/api",
            "add a retry with backoff, write a test",
            &today,
        ))
        .unwrap();
    ct_index::embed_pending(&mut store, &HashEmbedder::default()).unwrap();
    let addr = serve(Daemon::new(store, Box::new(HashEmbedder::default())));
    let client = Client::new(addr);

    match client.call(&Request::Glance).unwrap() {
        Response::Glance { glance } => {
            assert_eq!(glance.today_sessions, 1);
            assert!(glance.providers.iter().any(|p| p.tool == "claude-code"));
            assert!(glance.fluency_overall <= 100);
            // No quota is fabricated in a headless build.
            assert!(glance.providers.iter().all(|p| p.quota.is_none()));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn metrics_over_the_wire() {
    // Two user turns, one a correction → correction_rate should register.
    let mut store = Store::open_in_memory().unwrap();
    let convo = {
        let mut c = dated(
            Tool::ClaudeCode,
            "/code/api",
            "add retry, it must be idempotent",
            "2026-06-01",
        );
        c.messages.push(Message {
            id: "m2".into(),
            role: Role::User,
            content: "no, that's wrong — revert".into(),
            timestamp: None,
            code_snippets: vec![],
            tool_calls: vec![],
            metadata: serde_json::Value::Null,
        });
        c
    };
    store.upsert_conversation(&convo).unwrap();
    ct_index::embed_pending(&mut store, &HashEmbedder::default()).unwrap();
    let addr = serve(Daemon::new(store, Box::new(HashEmbedder::default())));
    let client = Client::new(addr);

    match client
        .call(&Request::Metrics {
            filters: Default::default(),
        })
        .unwrap()
    {
        Response::Metrics { metrics } => {
            assert_eq!(metrics.sessions, 1);
            assert!(
                metrics.correction_rate > 0.0,
                "the correction should register"
            );
            assert!(
                metrics.specificity_rate > 0.0,
                "\"must\" makes the opener specific"
            );
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn ask_retrieves_and_answers() {
    let (addr, _h) = start();
    let client = Client::new(addr);

    // `ask` must return a cited Answer drawn from the retrieved sessions whether
    // or not a model is configured (it synthesizes when one is, and hands back
    // the retrieved context when not). Either way the sources come from retrieval.
    match client
        .call(&Request::Ask {
            question: "retry backoff".into(),
            limit: 6,
            filters: Default::default(),
            devices: None,
        })
        .unwrap()
    {
        Response::Answer {
            markdown, sources, ..
        } => {
            assert!(!sources.is_empty(), "should retrieve the seeded match");
            assert!(!markdown.trim().is_empty(), "answer should not be empty");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn insight_errors_without_a_model_login() {
    // No model configured in CI → the insight op must surface a clean Error
    // response (not panic, not hang). A bad kind is also a clean error.
    let (addr, _h) = start();
    let client = Client::new(addr);

    match client
        .call(&Request::Insight {
            kind: "not_a_kind".into(),
            limit: None,
        })
        .unwrap()
    {
        Response::Error { message } => assert!(message.contains("unknown insight kind")),
        other => panic!("expected Error for bad kind, got {other:?}"),
    }

    // A valid kind with no model login also returns Error (gathering works; the
    // model call is what fails), as long as a key isn't present in the env.
    if std::env::var_os("ANTHROPIC_API_KEY").is_none()
        && std::env::var_os("OPENAI_API_KEY").is_none()
        && std::env::var_os("GEMINI_API_KEY").is_none()
    {
        match client
            .call(&Request::Insight {
                kind: "open_loops".into(),
                limit: Some(5),
            })
            .unwrap()
        {
            Response::Error { .. } => {}
            Response::Insight { .. } => { /* a CLI login happened to be present */ }
            other => panic!("unexpected: {other:?}"),
        }
    }
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
    Federation::new(
        device.into(),
        Some(token.into()),
        peers,
        Duration::from_millis(1500),
    )
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
fn device_selection_routes_to_named_devices() {
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

    // Restricting to "linux" surfaces the peer's thread...
    let only_linux = client
        .search_devices(
            "navbar",
            Mode::Lexical,
            10,
            Default::default(),
            Some(vec!["linux".into()]),
        )
        .unwrap();
    assert!(only_linux.iter().any(|h| h.tool == "cursor"));

    // ...but restricting to "mac" (local only) does not query the peer.
    let only_mac = client
        .search_devices(
            "navbar",
            Mode::Lexical,
            10,
            Default::default(),
            Some(vec!["mac".into()]),
        )
        .unwrap();
    assert!(only_mac.iter().all(|h| h.tool != "cursor"));
}

#[test]
fn federated_build_context_includes_remote_transcript() {
    // Only B has this conversation; A must fetch its transcript to build context.
    let b = Daemon::new(
        seed(&[convo(
            Tool::Cursor,
            "/web",
            "the remote caching insight lives here",
        )]),
        Box::new(HashEmbedder::default()),
    )
    .with_federation(fed("linux", "s3cret", vec![]));
    let b_addr = serve(b);

    let a = Daemon::new(
        seed(&[convo(Tool::ClaudeCode, "/api", "unrelated local thread")]),
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

    let (markdown, sources) = client
        .build_context(
            "caching insight",
            Mode::Lexical,
            3,
            6000,
            Default::default(),
        )
        .unwrap();
    assert!(
        markdown.contains("remote caching insight"),
        "context should include the peer's transcript, got: {markdown}"
    );
    assert!(!sources.is_empty());
}

#[test]
fn opens_a_remote_conversation() {
    // The conversation lives only on B; A must fetch it via GetConversation{device}.
    let remote = convo(Tool::Cursor, "/web", "remote navbar thread to open");
    let b = Daemon::new(
        seed(std::slice::from_ref(&remote)),
        Box::new(HashEmbedder::default()),
    )
    .with_federation(fed("linux", "s3cret", vec![]));
    let b_addr = serve(b);

    let a = Daemon::new(
        seed(&[convo(Tool::ClaudeCode, "/api", "unrelated local")]),
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

    let resp = client
        .call(&Request::GetConversation {
            id: remote.id.clone(),
            device: Some("linux".into()),
        })
        .unwrap();
    match resp {
        Response::Conversation { conversation } => {
            let c = conversation.expect("remote conversation fetched");
            assert_eq!(c.id, remote.id);
            assert!(c.messages.iter().any(|m| m.content.contains("navbar")));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn search_reports_unreachable_peers() {
    // A peers a daemon that isn't there; the search still returns local hits and
    // names the dead peer as unreachable.
    let a = Daemon::new(
        seed(&[convo(Tool::ClaudeCode, "/api", "local retry backoff")]),
        Box::new(HashEmbedder::default()),
    )
    .with_federation(fed(
        "mac",
        "s3cret",
        vec![Peer {
            name: "ghost".into(),
            addr: "127.0.0.1:9".into(),
        }],
    ));
    let client = Client::new(serve(a));

    let resp = client
        .call(&Request::Search {
            query: "retry".into(),
            mode: Mode::Hybrid,
            limit: 10,
            filters: Default::default(),
            devices: None,
        })
        .unwrap();
    match resp {
        Response::Hits { hits, unreachable } => {
            assert!(!hits.is_empty(), "local hits still returned");
            assert!(unreachable.contains(&"ghost".to_string()));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn serve_scope_hides_excluded_tool_from_peers() {
    // B excludes `cursor` from what it serves, so A never sees B's cursor thread
    // — neither in search nor when trying to open it.
    let scoped = convo(Tool::Cursor, "/web", "navbar thread that is scoped out");
    let b = Daemon::new(
        seed(std::slice::from_ref(&scoped)),
        Box::new(HashEmbedder::default()),
    )
    .with_federation(fed("linux", "s3cret", vec![]).with_scope(vec!["cursor".into()], vec![]));
    let b_addr = serve(b);

    let a = Daemon::new(
        seed(&[convo(Tool::ClaudeCode, "/api", "local thread")]),
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
        .search("navbar", Mode::Lexical, 10, Default::default())
        .unwrap();
    assert!(
        hits.iter().all(|h| h.tool != "cursor"),
        "excluded tool hidden"
    );

    // Direct fetch of the excluded conversation is refused too.
    let resp = client
        .call(&Request::GetConversation {
            id: scoped.id.clone(),
            device: Some("linux".into()),
        })
        .unwrap();
    assert!(matches!(
        resp,
        Response::Conversation { conversation: None }
    ));
}

#[test]
fn pair_with_code_adopts_token_and_peer() {
    // B has no token yet; pairing with a code from "mac" adopts it + approves mac.
    let b = Daemon::new(seed(&[]), Box::new(HashEmbedder::default())).with_federation(
        Federation::new("linux".into(), None, vec![], Duration::from_millis(500)),
    );
    let client = Client::new(serve(b));

    let mac = Federation::new(
        "mac".into(),
        Some("sec".into()),
        vec![],
        Duration::from_millis(1),
    )
    .with_listen_addr(Some("100.9.9.9:47100".into()));
    let code = mac.pairing_code().expect("mac issues a pairing code");

    match client.call(&Request::PairWithCode { code }).unwrap() {
        Response::Devices {
            devices,
            federation,
        } => {
            assert!(federation);
            assert!(devices.iter().any(|d| d.name == "mac"));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn read_pool_serves_concurrent_searches() {
    // A file-backed index with a read pool: concurrent searches run on separate
    // read-only connections (WAL) and return correct results.
    let path = std::env::temp_dir().join(format!("ct-pool-{}.db", std::process::id()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }

    let mut store = Store::open(&path).unwrap();
    store
        .upsert_conversation(&convo(
            Tool::ClaudeCode,
            "/api",
            "add retry logic with backoff",
        ))
        .unwrap();
    store
        .upsert_conversation(&convo(
            Tool::Cursor,
            "/web",
            "dark mode toggle for the navbar",
        ))
        .unwrap();
    ct_index::embed_pending(&mut store, &HashEmbedder::default()).unwrap();

    let daemon = Daemon::new(store, Box::new(HashEmbedder::default())).with_read_pool(&path, 4);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    thread::spawn(move || {
        let _ = daemon.serve(listener);
    });

    let mut handles = Vec::new();
    for _ in 0..12 {
        let addr = addr.clone();
        handles.push(thread::spawn(move || {
            let client = Client::new(addr);
            let hits = client
                .search("retry backoff", Mode::Hybrid, 10, Default::default())
                .unwrap();
            assert!(hits.iter().any(|h| h.project.as_deref() == Some("/api")));
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
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
