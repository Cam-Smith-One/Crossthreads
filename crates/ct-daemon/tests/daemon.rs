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
fn activity_over_the_wire() {
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
fn quota_sidecar_flows_into_provider() {
    // With an on-device sidecar present, the matching provider carries the live
    // window end-to-end through `build`.
    let mut store = Store::open_in_memory().unwrap();
    let now = chrono::Utc::now();
    let today = now.format("%Y-%m-%d").to_string();
    store
        .upsert_conversation(&dated(Tool::ClaudeCode, "/code/api", "ship it", &today))
        .unwrap();

    let dir = std::env::temp_dir().join(format!("ct-glance-quota-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let resets = (now + chrono::Duration::hours(4)).to_rfc3339();
    std::fs::write(
        dir.join("claude-code.json"),
        format!(r#"{{"used":320,"limit":500,"resets_at":"{resets}","source":"demo-reader"}}"#),
    )
    .unwrap();

    let glance = ct_daemon::glance::build_in(&store, now, Some(&dir)).unwrap();
    let p = glance
        .providers
        .iter()
        .find(|p| p.tool == "claude-code")
        .expect("claude-code active today");
    let q = p
        .quota
        .as_ref()
        .expect("sidecar window attaches to the provider");
    assert_eq!(q.limit, 500.0);
    assert_eq!(q.source, "demo-reader");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resume_over_the_wire() {
    // A claude-code session resolves to a runnable `claude --resume <id>` command.
    let mut store = Store::open_in_memory().unwrap();
    let mut c = convo(Tool::ClaudeCode, "/home/me/app", "refactor the parser");
    c.source = Source {
        path: "/home/me/.claude/projects/-home-me-app/sess-abc-123.jsonl".into(),
        offset: None,
        fingerprint: "claude-code/v1".into(),
    };
    let id = c.id.clone();
    store.upsert_conversation(&c).unwrap();
    let addr = serve(Daemon::new(store, Box::new(HashEmbedder::default())));
    let client = Client::new(addr);

    match client.call(&Request::Resume { id }).unwrap() {
        Response::Resume { resume } => {
            let r = resume.expect("a known id resolves");
            match r.action {
                ct_daemon::resume::Action::Resume { display, args, .. } => {
                    assert_eq!(args, vec!["--resume", "sess-abc-123"]);
                    assert_eq!(display, "cd /home/me/app && claude --resume sess-abc-123");
                }
                other => panic!("expected a resume command, got {other:?}"),
            }
        }
        other => panic!("unexpected: {other:?}"),
    }

    // An unknown id resolves to no plan (not an error).
    match client
        .call(&Request::Resume {
            id: "cv_does_not_exist".into(),
        })
        .unwrap()
    {
        Response::Resume { resume } => assert!(resume.is_none()),
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
            rooms: false,
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
            rooms: false,
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
            rooms: false,
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
            rooms: false,
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
            rooms: false,
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
            rooms: false,
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
            rooms: false,
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
            rooms: false,
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

#[test]
fn threads_link_edit_and_unlink_over_the_wire() {
    let a = dated(
        Tool::ClaudeCode,
        "/acme-api",
        "fix the oauth refresh retry",
        "2026-11-01",
    );
    let b = dated(
        Tool::Codex,
        "/acme-api",
        "review the oauth refresh fix",
        "2026-11-02",
    );
    let c = dated(Tool::Cursor, "/acme-web", "dark mode toggle", "2026-11-03");
    let store = seed(&[a.clone(), b.clone(), c.clone()]);
    store
        .set_repo_key("/acme-api", "github.com/acme/api")
        .unwrap();
    let addr = serve(Daemon::new(store, Box::new(HashEmbedder::default())));
    let client = Client::new(addr);

    // Three sessions, three threads, newest first.
    let Response::Threads { threads, .. } = client
        .call(&Request::Threads {
            filters: Default::default(),
            devices: None,
        })
        .unwrap()
    else {
        panic!("expected threads");
    };
    assert_eq!(threads.len(), 3);
    assert_eq!(threads[0].tools, vec!["cursor"]);
    assert_eq!(threads[2].repo_label.as_deref(), Some("acme/api"));

    // Link the two oauth sessions; the thread now spans two tools.
    let Response::ThreadId { id: Some(id) } = client
        .call(&Request::LinkSessions {
            a: a.id.clone(),
            b: b.id.clone(),
            kind: Some("reviewed".into()),
        })
        .unwrap()
    else {
        panic!("expected a thread id");
    };
    let Response::ThreadId { id: Some(same) } = client
        .call(&Request::SetThread {
            id: id.clone(),
            title: Some("OAuth refresh".into()),
            status: Some("done".into()),
        })
        .unwrap()
    else {
        panic!("expected a thread id");
    };
    assert_eq!(same, id);

    let Response::Thread { thread: Some(t) } = client
        .call(&Request::Thread {
            id: b.id.clone(),
            device: None,
        })
        .unwrap()
    else {
        panic!("expected a thread");
    };
    assert_eq!(t.summary.id, id);
    assert_eq!(t.summary.title, "OAuth refresh");
    assert_eq!(t.summary.status, "done");
    assert_eq!(t.summary.tools, vec!["claude-code", "codex"]);
    assert_eq!(t.links[0].kind, "reviewed");

    // Filters apply on the daemon.
    let Response::Threads { threads, .. } = client
        .call(&Request::Threads {
            filters: ct_store::ThreadFilters {
                status: Some("done".into()),
                ..Default::default()
            },
            devices: None,
        })
        .unwrap()
    else {
        panic!("expected threads");
    };
    assert_eq!(threads.len(), 1);

    // A bad status is an error, not a silent no-op.
    assert!(matches!(
        client
            .call(&Request::SetThread {
                id: id.clone(),
                title: None,
                status: Some("archived".into()),
            })
            .unwrap(),
        Response::Error { .. }
    ));

    // Unlink puts the session back on its own.
    assert!(matches!(
        client
            .call(&Request::UnlinkSession { id: b.id.clone() })
            .unwrap(),
        Response::Ok { ok: true }
    ));
    let Response::Thread { thread: Some(t) } = client
        .call(&Request::Thread {
            id: b.id,
            device: None,
        })
        .unwrap()
    else {
        panic!("expected a thread");
    };
    assert_eq!(t.sessions.len(), 1);
    assert!(matches!(
        client
            .call(&Request::Thread {
                id: "th_missing".into(),
                device: None
            })
            .unwrap(),
        Response::Thread { thread: None }
    ));
}

#[test]
fn hook_events_show_up_as_live_state_on_threads() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("session.jsonl");
    // An old file, so nothing is inferred from its modification time.
    std::fs::write(&transcript, "{}\n").unwrap();
    let mut a = dated(
        Tool::Cursor,
        "/acme-api",
        "fix the oauth refresh retry",
        "2026-11-01",
    );
    a.source.path = transcript.to_str().unwrap().into();
    let b = dated(Tool::Codex, "/acme-web", "dark mode toggle", "2026-11-02");
    let addr = serve(Daemon::new(
        seed(&[a.clone(), b]),
        Box::new(HashEmbedder::default()),
    ));
    let client = Client::new(addr);
    let live_of = |client: &Client| -> Option<(String, bool)> {
        let Response::Thread { thread: Some(t) } = client
            .call(&Request::Thread {
                id: a.id.clone(),
                device: None,
            })
            .unwrap()
        else {
            panic!("expected a thread");
        };
        assert_eq!(
            t.summary.live.as_ref().map(|l| &l.state),
            t.sessions[0].live.as_ref().map(|l| &l.state)
        );
        t.summary.live.map(|l| (l.state, l.inferred))
    };
    assert_eq!(live_of(&client), None);

    // The same JSON `crossthreads hook` sends: the op plus the event's fields.
    let hook = |event: &str, message: Option<&str>| {
        let req: Request = serde_json::from_value(serde_json::json!({
            "op": "hook_event",
            "tool": "cursor",
            "event": event,
            "transcript_path": transcript.to_str().unwrap(),
            "session_id": "abc",
            "message": message,
        }))
        .unwrap();
        client.call(&req).unwrap()
    };
    assert!(matches!(
        hook("UserPromptSubmit", None),
        Response::Ok { ok: true }
    ));
    assert_eq!(live_of(&client), Some(("running".into(), false)));
    hook(
        "Notification",
        Some("Claude needs your permission to use Bash"),
    );
    assert_eq!(live_of(&client), Some(("needs_you".into(), false)));

    // The list carries the state too, and only for the session that reported.
    let Response::Threads { threads, .. } = client
        .call(&Request::Threads {
            filters: Default::default(),
            devices: None,
        })
        .unwrap()
    else {
        panic!("expected threads");
    };
    let states: Vec<Option<&str>> = threads
        .iter()
        .map(|t| t.live.as_ref().map(|l| l.state.as_str()))
        .collect();
    assert_eq!(states, vec![None, Some("needs_you")]);

    hook("Stop", None);
    assert_eq!(live_of(&client), Some(("idle".into(), false)));
    // An event that names no transcript can't be tied to a session.
    let req: Request = serde_json::from_value(
        serde_json::json!({"op":"hook_event","tool":"codex","event":"Stop"}),
    )
    .unwrap();
    assert!(matches!(
        client.call(&req).unwrap(),
        Response::Ok { ok: false }
    ));
}

#[test]
fn the_board_spans_devices_and_opens_remote_threads() {
    let mine = dated(
        Tool::ClaudeCode,
        "/acme-api",
        "fix the oauth refresh retry",
        "2026-11-01",
    );
    let theirs = dated(
        Tool::Codex,
        "/srv/acme-api",
        "review the oauth refresh fix",
        "2026-11-03",
    );
    let hidden = dated(
        Tool::Cursor,
        "/srv/secret-project",
        "do not share this",
        "2026-11-04",
    );

    let desk_store = seed(&[theirs.clone(), hidden.clone()]);
    desk_store
        .set_repo_key("/srv/acme-api", "github.com/acme/api")
        .unwrap();
    let desk = Daemon::new(desk_store, Box::new(HashEmbedder::default())).with_federation(
        Federation::new(
            "desk".into(),
            Some("s3cret".into()),
            vec![],
            Duration::from_secs(2),
        )
        .with_scope(vec![], vec!["secret".into()]),
    );
    let desk_addr = serve(desk);

    let lap_store = seed(std::slice::from_ref(&mine));
    lap_store
        .set_repo_key("/acme-api", "github.com/acme/api")
        .unwrap();
    let peers = vec![
        Peer {
            name: "desk".into(),
            addr: desk_addr.clone(),
            rooms: false,
        },
        Peer {
            name: "off".into(),
            addr: "127.0.0.1:1".into(),
            rooms: false,
        },
    ];
    let lap = Daemon::new(lap_store, Box::new(HashEmbedder::default()))
        .with_federation(fed("lap", "s3cret", peers));
    let client = Client::new(serve(lap));

    let list = |devices: Option<Vec<String>>| {
        let Response::Threads {
            threads,
            unreachable,
        } = client
            .call(&Request::Threads {
                filters: Default::default(),
                devices,
            })
            .unwrap()
        else {
            panic!("expected threads");
        };
        (threads, unreachable)
    };

    // One list, newest first, each thread tagged with its device. The same
    // repository has the same key on both machines despite different paths.
    let (threads, unreachable) = list(None);
    let seen: Vec<(&str, &str)> = threads
        .iter()
        .map(|t| (t.device.as_deref().unwrap(), t.tools[0].as_str()))
        .collect();
    assert_eq!(seen, vec![("desk", "codex"), ("lap", "claude-code")]);
    assert!(threads
        .iter()
        .all(|t| t.repo_key.as_deref() == Some("github.com/acme/api")));
    assert_eq!(
        unreachable,
        vec!["off"],
        "an offline peer is named, not fatal"
    );
    assert!(
        !threads.iter().any(|t| t.title.contains("do not share")),
        "serve-scope hides the excluded project"
    );

    // Routing: only the named devices are asked.
    let (only_lap, unreachable) = list(Some(vec!["lap".into()]));
    assert_eq!(only_lap.len(), 1);
    assert!(unreachable.is_empty());

    // Open the remote thread through this daemon.
    let remote_id = threads[0].id.clone();
    let Response::Thread { thread: Some(t) } = client
        .call(&Request::Thread {
            id: remote_id,
            device: Some("desk".into()),
        })
        .unwrap()
    else {
        panic!("expected the remote thread");
    };
    assert_eq!(t.summary.device.as_deref(), Some("desk"));
    assert_eq!(t.sessions[0].conversation_id, theirs.id);

    // Peers are token-gated, and a scoped-out thread can't be fetched by id.
    let desk_direct = Client::new(desk_addr);
    assert!(matches!(
        desk_direct
            .call(&Request::PeerThreads {
                token: Some("wrong".into()),
                filters: Default::default(),
            })
            .unwrap(),
        Response::Error { .. }
    ));
    assert!(matches!(
        desk_direct
            .call(&Request::PeerThread {
                token: Some("s3cret".into()),
                id: hidden.id,
            })
            .unwrap(),
        Response::Thread { thread: None }
    ));
    // An unknown device is an error the UI can show.
    assert!(matches!(
        client
            .call(&Request::Thread {
                id: "th_x".into(),
                device: Some("nowhere".into()),
            })
            .unwrap(),
        Response::Error { .. }
    ));
}

// ---- rooms -----------------------------------------------------------------

/// Call a room tool and return its text.
fn room(client: &Client, req: serde_json::Value) -> String {
    let req: Request = serde_json::from_value(req).unwrap();
    match client.call(&req).unwrap() {
        Response::Text { text } => text,
        other => panic!("expected text, got {other:?}"),
    }
}

fn rooms_daemon(cap: i64) -> Client {
    Client::new(serve(
        Daemon::new(
            Store::open_in_memory().unwrap(),
            Box::new(HashEmbedder::default()),
        )
        .with_room_turn_cap(cap),
    ))
}

#[test]
fn rooms_ping_pong_dms_and_isolation() {
    let client = rooms_daemon(0);
    use serde_json::json;

    // send blocks for the reply; bob waits, then answers.
    let bob = {
        let client = Client::new(client_addr(&client));
        thread::spawn(move || {
            let got = room(
                &client,
                json!({"op":"room_wait","me":"Bob","room":"demo","timeout":10}),
            );
            assert!(got.contains("alice→bob: ping"), "{got}");
            room(
                &client,
                json!({"op":"room_send","me":"bob","room":"demo","to":"alice","text":"pong","wait":false}),
            )
        })
    };
    let a = room(
        &client,
        json!({"op":"room_send","me":"alice","room":"demo","to":"bob","text":"ping","timeout":10}),
    );
    bob.join().unwrap();
    assert!(a.starts_with("Posted #"), "{a}");
    let reply = a.split_once('\n').unwrap().1;
    assert!(
        reply.contains("bob→alice: pong") && !reply.contains("ping"),
        "{a}"
    );

    // A direct message stays private, and nobody reads their own messages back.
    assert!(!room(
        &client,
        json!({"op":"room_wait","me":"carol","room":"demo","timeout":0})
    )
    .contains("ping"));
    assert!(room(
        &client,
        json!({"op":"room_wait","me":"bob","room":"demo","timeout":0})
    )
    .contains("No new messages"));

    // Rooms are isolated.
    room(
        &client,
        json!({"op":"room_send","me":"alice","room":"other","text":"elsewhere","wait":false}),
    );
    assert!(!room(
        &client,
        json!({"op":"room_wait","me":"bob","room":"demo","timeout":0})
    )
    .contains("elsewhere"));
    let history = room(
        &client,
        json!({"op":"room_history","me":"bob","room":"demo"}),
    );
    assert!(
        history.starts_with("Room 'demo'. Agents: alice active"),
        "{history}"
    );
    assert!(
        history.contains("Claims: none.") && history.contains("alice→bob: ping"),
        "{history}"
    );
    // Missing arguments are reported to the agent, not thrown.
    assert!(room(
        &client,
        json!({"op":"room_send","me":" ","room":"demo","text":"x"})
    )
    .starts_with("Error"));
}

/// The address a client talks to, so a second client can be opened on it.
fn client_addr(client: &Client) -> String {
    client.addr().to_string()
}

#[test]
fn rooms_turn_cap_holds_until_done_or_the_human_steps_in() {
    let client = rooms_daemon(4);
    use serde_json::json;
    let say = |text: &str| {
        room(
            &client,
            json!({"op":"room_send","me":"alice","room":"cap","text":text,"wait":false}),
        )
    };
    assert_eq!(say("t0"), "Posted #1.");
    assert!(say("t1").contains("[turn 2/4: wrap up soon]"));
    say("t2");
    say("t3");
    assert!(say("t4").starts_with("Refused: turn cap reached (4/4)"));

    // The human extends it from the board.
    let Response::RoomPosted { .. } = client
        .call(&Request::RoomPost {
            room: "cap".into(),
            text: "keep going".into(),
            to: None,
            author: None,
        })
        .unwrap()
    else {
        panic!("expected a posted id");
    };
    assert!(say("t5").starts_with("Posted"));
    let got = room(
        &client,
        json!({"op":"room_wait","me":"bob","room":"cap","timeout":0}),
    );
    assert!(got.contains("human (human): keep going"), "{got}");
    // DONE is always allowed and starts a fresh exchange.
    for t in ["t6", "t7", "t8"] {
        say(t);
    }
    assert!(say("t9").starts_with("Refused"));
    assert!(say("DONE wrapped up").contains("Exchange closed"));
    assert!(say("next").starts_with("Posted"));
    // An empty human message is rejected.
    assert!(matches!(
        client
            .call(&Request::RoomPost {
                room: "cap".into(),
                text: "  ".into(),
                to: None,
                author: None
            })
            .unwrap(),
        Response::Error { .. }
    ));
}

#[test]
fn rooms_claims_guard_edits_and_done_releases_them() {
    let client = rooms_daemon(0);
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    let app = src.join("app.py");
    std::fs::write(&app, "x").unwrap();
    let (src, app) = (src.to_str().unwrap(), app.to_str().unwrap());

    let got = room(
        &client,
        json!({"op":"room_claim","me":"alice","room":"demo","paths":[src]}),
    );
    assert!(got.starts_with("Claimed until"), "{got}");
    let got = room(
        &client,
        json!({"op":"room_claim","me":"bob","room":"demo","paths":[app, "relative.py"]}),
    );
    assert!(
        got.contains("held by alice") && got.contains("must be an absolute path"),
        "{got}"
    );

    // What the PreToolUse hook asks before an edit.
    let guard = |path: &str| {
        let Response::RoomGuard { claims } = client
            .call(&Request::RoomGuard {
                paths: vec![path.into()],
            })
            .unwrap()
        else {
            panic!("expected claims");
        };
        claims.into_iter().map(|c| c.holder).collect::<Vec<_>>()
    };
    assert_eq!(guard(app), ["alice"]);
    assert!(guard(dir.path().join("docs/readme.md").to_str().unwrap()).is_empty());

    // The board lists the claim against its room.
    let Response::Rooms { rooms } = client.call(&Request::Rooms).unwrap() else {
        panic!("expected rooms");
    };
    // Claiming put alice in the room, so it is listed (with the claim) before
    // anyone has said anything.
    assert_eq!(
        rooms.len(),
        1,
        "a room appears as soon as an agent is in it"
    );
    assert_eq!(
        (
            rooms[0].label.as_str(),
            rooms[0].messages,
            rooms[0].claims.len()
        ),
        ("demo", 0, 1)
    );
    room(
        &client,
        json!({"op":"room_send","me":"alice","room":"demo","text":"working","wait":false}),
    );
    let Response::Rooms { rooms } = client.call(&Request::Rooms).unwrap() else {
        panic!("expected rooms");
    };
    assert_eq!(
        (rooms[0].label.as_str(), rooms[0].claims.len()),
        ("demo", 1)
    );
    assert_eq!(
        rooms[0]
            .agents
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["alice", "bob"]
    );

    // DONE releases everything the sender holds in that room.
    room(
        &client,
        json!({"op":"room_send","me":"alice","room":"demo","text":"DONE shipped","wait":false}),
    );
    assert!(guard(app).is_empty());
    let history = room(&client, json!({"op":"room_history","room":"demo"}));
    assert!(history.contains("Claims: none."), "{history}");
    // Explicit release.
    room(
        &client,
        json!({"op":"room_claim","me":"bob","room":"demo","paths":[app]}),
    );
    assert_eq!(
        room(&client, json!({"op":"room_release","me":"bob"})),
        "Released 1 claim(s)."
    );
}

#[test]
fn rooms_stop_hook_wakes_an_agent_mid_exchange_only() {
    let client = rooms_daemon(0);
    use serde_json::json;
    let stop = |me: &str, wait: u64| {
        let Response::RoomStop { messages, room } = client
            .call(&Request::RoomStop {
                me: me.into(),
                room: "demo".into(),
                wait,
                transcript_path: None,
            })
            .unwrap()
        else {
            panic!("expected a stop answer");
        };
        messages.map(|m| (room.unwrap(), m))
    };
    // Never joined: not held.
    assert_eq!(stop("bob", 0), None);

    room(
        &client,
        json!({"op":"room_wait","me":"bob","room":"demo","timeout":0}),
    );
    room(
        &client,
        json!({"op":"room_send","me":"alice","room":"demo","to":"bob","text":"wake up","wait":false}),
    );
    let (label, got) = stop("bob", 0).unwrap();
    assert_eq!(label, "demo");
    assert!(got.contains("alice→bob: wake up"), "{got}");
    // Nothing new: let it stop.
    assert_eq!(stop("bob", 0), None);

    // A message that arrives while the hook is waiting is handed straight back.
    let waiter = {
        let client = Client::new(client_addr(&client));
        thread::spawn(move || {
            let Response::RoomStop { messages, .. } = client
                .call(&Request::RoomStop {
                    me: "bob".into(),
                    room: "demo".into(),
                    wait: 10,
                    transcript_path: None,
                })
                .unwrap()
            else {
                panic!("expected a stop answer");
            };
            messages
        })
    };
    thread::sleep(Duration::from_millis(300));
    let Response::Rooms { rooms } = client.call(&Request::Rooms).unwrap() else {
        panic!("expected rooms");
    };
    assert!(
        rooms[0]
            .agents
            .iter()
            .any(|a| a.name == "bob" && a.listening),
        "the board shows who is listening"
    );
    room(
        &client,
        json!({"op":"room_send","me":"alice","room":"demo","to":"bob","text":"second","wait":false}),
    );
    assert!(waiter.join().unwrap().unwrap().contains("second"));

    // Once the exchange is closed, a stopping agent is not held.
    room(
        &client,
        json!({"op":"room_send","me":"carol","room":"demo","to":"bob","text":"late","wait":false}),
    );
    room(
        &client,
        json!({"op":"room_send","me":"carol","room":"demo","text":"DONE","wait":false}),
    );
    assert_eq!(stop("bob", 0), None);
}

#[test]
fn a_closed_exchange_is_searchable_and_links_the_sessions_that_took_part() {
    use serde_json::json;
    let mut a = dated(
        Tool::ClaudeCode,
        "/acme-api",
        "plan the oauth refresh fix",
        "2026-11-01",
    );
    a.source.path = "/t/claude.jsonl".into();
    let mut b = dated(
        Tool::Codex,
        "/acme-api",
        "implement what claude briefs",
        "2026-11-02",
    );
    b.source.path = "/t/codex.jsonl".into();
    let client = Client::new(serve(Daemon::new(
        seed(&[a.clone(), b.clone()]),
        Box::new(HashEmbedder::default()),
    )));

    // The Stop hook is how the daemon learns which session each agent is.
    for (me, path) in [("claude", "/t/claude.jsonl"), ("codex", "/t/codex.jsonl")] {
        client
            .call(&Request::RoomStop {
                me: me.into(),
                room: "launch".into(),
                wait: 0,
                transcript_path: Some(path.into()),
            })
            .unwrap();
    }
    room(
        &client,
        json!({"op":"room_send","me":"claude","room":"launch","to":"codex","text":"add exponential backoff to the token refresh","wait":false}),
    );
    room(
        &client,
        json!({"op":"room_send","me":"codex","room":"launch","to":"claude","text":"added backoff with jitter in src/auth/refresh.rs","wait":false}),
    );
    room(
        &client,
        json!({"op":"room_send","me":"claude","room":"launch","text":"DONE backoff shipped and reviewed","wait":false}),
    );

    // Recall finds the exchange itself.
    let hits = client
        .search(
            "jitter refresh backoff",
            Mode::Lexical,
            10,
            Default::default(),
        )
        .unwrap();
    let hit = hits
        .iter()
        .find(|h| h.tool == "room")
        .expect("exchange indexed");
    assert_eq!(hit.kind, "exchange");
    assert!(hit
        .title
        .as_deref()
        .unwrap()
        .starts_with("launch: add exponential backoff"));

    // And the two sessions are now one thread.
    let Response::Thread { thread: Some(t) } = client
        .call(&Request::Thread {
            id: a.id.clone(),
            device: None,
        })
        .unwrap()
    else {
        panic!("expected a thread");
    };
    assert_eq!(t.summary.tools, vec!["claude-code", "codex"]);
    assert_eq!(
        (t.links[0].kind.as_str(), t.links[0].origin.as_str()),
        ("conversed", "explicit")
    );
    // The exchange is not a session, so it doesn't appear on the board.
    let Response::Threads { threads, .. } = client
        .call(&Request::Threads {
            filters: Default::default(),
            devices: None,
        })
        .unwrap()
    else {
        panic!("expected threads");
    };
    assert_eq!(threads.len(), 1);
}

/// Send a raw HTTP request and return (status, body).
fn http(addr: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    let mut req = format!(
        "POST /api/rpc HTTP/1.1\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    stream.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    let status = out.split_whitespace().nth(1).unwrap().parse().unwrap();
    (
        status,
        out.split("\r\n\r\n").nth(1).unwrap_or("").to_string(),
    )
}

#[test]
fn the_http_api_refuses_other_websites() {
    let daemon = Daemon::new(
        Store::open_in_memory().unwrap(),
        Box::new(HashEmbedder::default()),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    {
        let (daemon, addr) = (daemon.clone(), addr.clone());
        thread::spawn(move || daemon.serve_http(&addr, None));
    }
    // Wait for the server to come up.
    for _ in 0..50 {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let post = r#"{"op":"room_post","room":"demo","text":"rm -rf everything"}"#;
    let json = ("Content-Type", "application/json");
    let host = ("Host", addr.as_str());
    let origin = format!("http://{addr}");

    // Our own page, and a local script with no Origin, are fine.
    assert_eq!(
        http(
            &addr,
            &[json, host, ("Origin", &origin)],
            r#"{"op":"ping"}"#
        )
        .0,
        200
    );
    assert_eq!(http(&addr, &[json, host], r#"{"op":"ping"}"#).0, 200);

    // Another site open in the browser: wrong origin.
    assert_eq!(
        http(
            &addr,
            &[json, host, ("Origin", "https://evil.example")],
            post
        )
        .0,
        403
    );
    // A cross-site form can't send JSON without a preflight.
    assert_eq!(
        http(&addr, &[("Content-Type", "text/plain"), host], post).0,
        403
    );
    assert_eq!(http(&addr, &[host], post).0, 403);
    // DNS rebinding: a hostile name pointed at loopback, origin "matching".
    let evil_host = format!("evil.example:{}", addr.rsplit(':').next().unwrap());
    assert_eq!(
        http(
            &addr,
            &[
                json,
                ("Host", &evil_host),
                ("Origin", &format!("http://{evil_host}"))
            ],
            post
        )
        .0,
        403
    );
    // None of the refused posts reached the room.
    let (_, body) = http(&addr, &[json, host], r#"{"op":"rooms"}"#);
    assert!(body.contains(r#""rooms":[]"#), "{body}");

    // A blocking call over HTTP returns at once instead of freezing the UI.
    let started = std::time::Instant::now();
    let (status, body) = http(
        &addr,
        &[json, host],
        r#"{"op":"room_wait","me":"bob","room":"demo","timeout":30}"#,
    );
    assert_eq!(status, 200);
    assert!(body.contains("No new messages"), "{body}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

// ---- rooms across devices (SPEC FR-ROOM-08) ---------------------------------

/// Two daemons, `mac` and `linux`, that know each other. `grant` sets whether
/// each lets the other message.
fn two_devices(mac_grants: bool, linux_grants: bool) -> (Daemon, Client, Daemon, Client) {
    let mac_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let linux_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mac_addr = mac_listener.local_addr().unwrap().to_string();
    let linux_addr = linux_listener.local_addr().unwrap().to_string();
    let device = |name: &str, peer: &str, addr: &str, rooms: bool| {
        Daemon::new(
            Store::open_in_memory().unwrap(),
            Box::new(HashEmbedder::default()),
        )
        .with_federation(fed(
            name,
            "s3cret",
            vec![Peer {
                name: peer.into(),
                addr: addr.into(),
                rooms,
            }],
        ))
    };
    let mac = device("mac", "linux", &linux_addr, mac_grants);
    let linux = device("linux", "mac", &mac_addr, linux_grants);
    for (d, l) in [(mac.clone(), mac_listener), (linux.clone(), linux_listener)] {
        thread::spawn(move || {
            let _ = d.serve(l);
        });
    }
    (mac, Client::new(mac_addr), linux, Client::new(linux_addr))
}

#[test]
fn rooms_work_across_devices_that_may_message() {
    use serde_json::json;
    let (_mac, mac, _linux, linux) = two_devices(true, true);
    const ROOM: &str = "github.com/acme/api";

    // codex on linux listens; claude on mac asks it directly and blocks.
    let codex = thread::spawn(move || {
        let got = room(
            &linux,
            json!({"op":"room_wait","me":"codex","room":ROOM,"timeout":10}),
        );
        assert!(
            got.contains("claude@mac→codex@linux: review src/auth?"),
            "{got}"
        );
        room(
            &linux,
            json!({"op":"room_send","me":"codex","room":ROOM,"to":"claude@mac","text":"DONE looks fine","wait":false}),
        );
        linux
    });
    let asked = room(
        &mac,
        json!({"op":"room_send","me":"claude","room":ROOM,"to":"codex@linux","text":"review src/auth?","timeout":10}),
    );
    let linux = codex.join().unwrap();
    assert!(
        asked.contains("codex@linux→claude@mac: DONE looks fine"),
        "{asked}"
    );

    // Both devices list the other's agent, and both archived the exchange
    // under the same id, so cross-device search shows it once.
    let agents = |c: &Client| match c.call(&Request::Rooms).unwrap() {
        Response::Rooms { rooms } => rooms[0]
            .agents
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };
    assert!(agents(&mac).contains(&"codex@linux".to_string()));
    assert!(agents(&linux).contains(&"claude@mac".to_string()));
    let exchange = |c: &Client, device: &str| match c
        .call(&Request::Search {
            query: "review".into(),
            mode: Mode::Lexical,
            limit: 5,
            filters: Default::default(),
            devices: Some(vec![device.into()]),
        })
        .unwrap()
    {
        Response::Hits { hits, .. } => hits
            .into_iter()
            .find(|h| h.kind == "exchange")
            .map(|h| h.conversation_id),
        other => panic!("{other:?}"),
    };
    let on_mac = exchange(&mac, "mac").expect("mac archived the exchange");
    assert_eq!(Some(on_mac), exchange(&linux, "linux"));
}

#[test]
fn rooms_need_the_grant_on_the_receiving_device_and_catch_up_later() {
    use serde_json::json;
    // mac lets linux message, but linux hasn't allowed mac yet.
    let (_mac, mac, linux_d, linux) = two_devices(true, false);
    const ROOM: &str = "github.com/acme/api";
    room(
        &mac,
        json!({"op":"room_send","me":"claude","room":ROOM,"text":"anyone there?","wait":false}),
    );
    thread::sleep(Duration::from_millis(300));
    let peek = |c: &Client| room(c, json!({"op":"room_history","room":ROOM,"limit":10}));
    assert!(!peek(&linux).contains("anyone there?"));

    // linux turns the grant on; its next catch-up brings the message over.
    match linux
        .call(
            &serde_json::from_value(json!({"op":"set_peer_rooms","name":"mac","rooms":true}))
                .unwrap(),
        )
        .unwrap()
    {
        Response::Devices { devices, .. } => {
            assert!(devices.iter().any(|d| d.name == "mac" && d.rooms))
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(linux_d.room_catch_up(), 1);
    assert_eq!(linux_d.room_catch_up(), 0, "nothing is fetched twice");
    assert!(
        peek(&linux).contains("claude@mac: anyone there?"),
        "{}",
        peek(&linux)
    );
}

// ---- the board from a phone (SPEC FR-BRD-04) --------------------------------

/// One HTTP request over a fresh connection; returns (status, headers, body).
fn http_raw(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    let status = head.split(' ').nth(1).unwrap_or("0").parse().unwrap_or(0);
    (status, head.to_string(), body.to_string())
}

#[test]
fn a_paired_phone_reads_the_board_and_posts_only_when_allowed() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = format!("127.0.0.1:{port}");
    std::env::set_var("CROSSTHREADS_PHONE_ADDR", &addr);
    let daemon = Daemon::new(
        Store::open_in_memory().unwrap(),
        Box::new(HashEmbedder::default()),
    );

    // Off until turned on; then it listens and offers a pairing code.
    assert!(matches!(
        daemon.handle(Request::PhonePair),
        Response::Error { .. }
    ));
    match daemon.handle(Request::PhoneSetEnabled { enabled: true }) {
        Response::Phone { listening, url, .. } => {
            assert!(listening);
            assert_eq!(url.as_deref(), Some(format!("http://{addr}/").as_str()));
        }
        other => panic!("{other:?}"),
    }
    let json = ("Content-Type", "application/json");
    let origin_value = format!("http://{addr}");
    let origin = ("Origin", origin_value.as_str());
    let rpc = |cookie: Option<&str>, body: &str| {
        let c;
        let mut h = vec![json, origin];
        if let Some(t) = cookie {
            c = format!("ct_phone={t}");
            h.push(("Cookie", c.as_str()));
        }
        http_raw(&addr, "POST", "/api/rpc", &h, body)
    };

    // Not paired: nothing but a request to pair.
    let (status, _, body) = rpc(None, r#"{"op":"threads"}"#);
    assert_eq!(status, 401, "{body}");
    assert!(body.contains("unpaired"));

    let Response::PhonePairing {
        code, url, qr_svg, ..
    } = daemon.handle(Request::PhonePair)
    else {
        panic!()
    };
    assert!(url.ends_with(&format!("/pair#{code}")) && qr_svg.is_some());
    let pair = |code: &str| {
        http_raw(
            &addr,
            "POST",
            "/api/pair",
            &[json, origin],
            &format!(r#"{{"code":"{code}","name":"Pixel"}}"#),
        )
    };
    assert_eq!(pair("WRONGXYZ").0, 403);
    let (status, head, _) = pair(&code);
    assert_eq!(status, 200);
    let cookie_line = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("set-cookie"))
        .expect("sets a cookie")
        .to_string();
    assert!(cookie_line.contains("HttpOnly") && cookie_line.contains("SameSite=Strict"));
    let token = cookie_line
        .split("ct_phone=")
        .nth(1)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(pair(&code).0, 403, "a code works once");

    // Paired: reading works, and it knows it's a phone.
    let (status, _, body) = rpc(Some(&token), r#"{"op":"threads"}"#);
    assert_eq!(status, 200, "{body}");
    let (_, _, body) = rpc(Some(&token), r#"{"op":"session"}"#);
    assert!(
        body.contains(r#""phone":true"#) && body.contains("Pixel"),
        "{body}"
    );

    // Settings and pairing are off limits, and so is posting without the grant.
    assert_eq!(rpc(Some(&token), r#"{"op":"phone_pair"}"#).0, 403);
    assert_eq!(rpc(Some(&token), r#"{"op":"reindex"}"#).0, 403);
    let post = r#"{"op":"room_post","room":"github.com/acme/api","text":"ship it"}"#;
    assert_eq!(rpc(Some(&token), post).0, 403);

    let Response::Phone { phones, .. } = daemon.handle(Request::PhoneStatus) else {
        panic!()
    };
    assert_eq!(phones.len(), 1);
    daemon.handle(Request::PhoneSetPost {
        id: phones[0].id.clone(),
        can_post: true,
    });
    let (status, _, body) = rpc(Some(&token), post);
    assert_eq!(status, 200, "{body}");
    match daemon.handle(Request::RoomMessages {
        room: "github.com/acme/api".into(),
        after: 0,
        limit: 10,
    }) {
        Response::RoomMessages { messages } => {
            assert!(messages[0].human);
            assert_eq!(messages[0].author, "pixel", "signed with the phone's name");
        }
        other => panic!("{other:?}"),
    }

    // A request that doesn't come from a browser page (no Origin) is refused.
    let (status, _, _) = http_raw(
        &addr,
        "POST",
        "/api/rpc",
        &[json, ("Cookie", &format!("ct_phone={token}"))],
        r#"{"op":"threads"}"#,
    );
    assert_eq!(status, 403);

    // Another website can't use the phone's session.
    let (status, _, _) = http_raw(
        &addr,
        "POST",
        "/api/rpc",
        &[
            json,
            ("Origin", "https://evil.example"),
            ("Cookie", &format!("ct_phone={token}")),
        ],
        r#"{"op":"threads"}"#,
    );
    assert_eq!(status, 403);

    // Revoked: locked out again. Turned off: no longer listening.
    daemon.handle(Request::PhoneRevoke {
        id: phones[0].id.clone(),
    });
    assert_eq!(rpc(Some(&token), r#"{"op":"threads"}"#).0, 401);
    daemon.handle(Request::PhoneSetEnabled { enabled: false });
    thread::sleep(Duration::from_millis(200));
    assert!(
        std::net::TcpStream::connect(&addr).is_err() || {
            // tiny_http may accept one last connection while shutting down.
            thread::sleep(Duration::from_millis(500));
            std::net::TcpStream::connect(&addr).is_err()
        }
    );
}
