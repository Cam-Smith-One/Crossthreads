//! Drive the MCP server's JSON-RPC handlers against a live in-process daemon.

use std::net::TcpListener;
use std::thread;

use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Message, Role, Source, Tool};
use ct_daemon::{Client, Daemon};
use ct_embed::HashEmbedder;
use ct_mcp::Server;
use ct_store::Store;
use serde_json::json;

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

fn server_with_daemon() -> Server {
    let mut store = Store::open_in_memory().unwrap();
    store
        .upsert_conversation(&convo(
            Tool::ClaudeCode,
            "/acme-api",
            "add retry logic with backoff",
        ))
        .unwrap();
    ct_index::embed_pending(&mut store, &HashEmbedder::default()).unwrap();
    let daemon = Daemon::new(store, Box::new(HashEmbedder::default()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    thread::spawn(move || {
        let _ = daemon.serve(listener);
    });
    Server::new(Client::new(addr))
}

#[test]
fn initialize_advertises_server_and_tools() {
    let s = server_with_daemon();

    let init = s
        .handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                        "params":{"protocolVersion":"2024-11-05"}}))
        .unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "crossthreads");
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    // The initialized notification gets no response.
    assert!(s
        .handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .is_none());

    let list = s
        .handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"crossthreads_search"));
    assert!(names.contains(&"crossthreads_recall"));
    assert!(names.contains(&"crossthreads_build_context"));
    assert!(names.contains(&"crossthreads_status"));
    assert!(names.contains(&"crossthreads_threads"));
    assert!(names.contains(&"crossthreads_thread"));
    for tool in [
        "crossthreads_room_send",
        "crossthreads_room_wait",
        "crossthreads_room_history",
        "crossthreads_claim",
        "crossthreads_release",
    ] {
        assert!(names.contains(&tool), "{tool}");
    }
    // The room rules travel with the server.
    assert!(init["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("One call per turn"));
}

#[test]
fn room_tools_carry_a_conversation_between_two_agents() {
    let s = std::sync::Arc::new(server_with_daemon());
    let call = |s: &Server, name: &str, args: serde_json::Value| -> String {
        let resp = s
            .handle(&json!({"jsonrpc":"2.0","id":9,"method":"tools/call",
                "params":{"name":name,"arguments":args}}))
            .unwrap();
        assert_eq!(resp["result"]["isError"], false, "{resp}");
        resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let listener = {
        let s = s.clone();
        thread::spawn(move || {
            let got = call(
                &s,
                "crossthreads_room_wait",
                json!({"me":"codex","room":"demo","timeout":10}),
            );
            assert!(
                got.contains("claude→codex: please add the retry test"),
                "{got}"
            );
            call(
                &s,
                "crossthreads_room_send",
                json!({"me":"codex","room":"demo","to":"claude","text":"test added","wait":false}),
            )
        })
    };
    let reply = call(
        &s,
        "crossthreads_room_send",
        json!({"me":"claude","room":"demo","to":"codex","text":"please add the retry test","timeout":10}),
    );
    listener.join().unwrap();
    assert!(reply.contains("codex→claude: test added"), "{reply}");

    let claimed = call(
        &s,
        "crossthreads_claim",
        json!({"me":"codex","room":"demo","paths":["/tmp/ct-demo/src"]}),
    );
    assert!(claimed.starts_with("Claimed until"), "{claimed}");
    let history = call(
        &s,
        "crossthreads_room_history",
        json!({"me":"claude","room":"demo"}),
    );
    assert!(
        history.contains("/tmp/ct-demo/src (codex until"),
        "{history}"
    );
    assert_eq!(
        call(&s, "crossthreads_release", json!({"me":"codex"})),
        "Released 1 claim(s)."
    );

    // A missing required argument is a tool error, not a crash.
    let resp = s
        .handle(&json!({"jsonrpc":"2.0","id":9,"method":"tools/call",
            "params":{"name":"crossthreads_room_send","arguments":{"room":"demo"}}}))
        .unwrap();
    assert_eq!(resp["result"]["isError"], true);
}

#[test]
fn threads_tools_list_and_open_a_thread() {
    let s = server_with_daemon();
    let resp = s
        .handle(&json!({"jsonrpc":"2.0","id":5,"method":"tools/call",
            "params":{"name":"crossthreads_threads","arguments":{}}}))
        .unwrap();
    assert_eq!(resp["result"]["isError"], false);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("1 thread(s)"), "got: {text}");
    assert!(text.contains("add retry logic with backoff"), "got: {text}");

    // The listed thread id opens the thread and shows its session.
    let id = text
        .split_whitespace()
        .find(|w| w.starts_with("th_"))
        .unwrap();
    let resp = s
        .handle(&json!({"jsonrpc":"2.0","id":6,"method":"tools/call",
            "params":{"name":"crossthreads_thread","arguments":{"id": id}}}))
        .unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("claude-code") && text.contains("cv_"),
        "got: {text}"
    );

    // An unknown id is a plain answer; a missing argument is a tool error.
    let resp = s
        .handle(&json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"crossthreads_thread","arguments":{"id":"th_nope"}}}))
        .unwrap();
    assert!(resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("No thread or session"));
    let resp = s
        .handle(&json!({"jsonrpc":"2.0","id":8,"method":"tools/call",
            "params":{"name":"crossthreads_thread","arguments":{}}}))
        .unwrap();
    assert_eq!(resp["result"]["isError"], true);
}

#[test]
fn build_context_returns_paste_ready_markdown() {
    let s = server_with_daemon();
    let resp = s
        .handle(&json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"crossthreads_build_context",
                      "arguments":{"query":"retry backoff"}}}))
        .unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Prior context"), "got: {text}");
    assert!(text.contains("**user:**"), "got: {text}");
}

#[test]
fn recall_digests_relevant_sessions() {
    let s = server_with_daemon();
    let resp = s
        .handle(&json!({"jsonrpc":"2.0","id":8,"method":"tools/call",
            "params":{"name":"crossthreads_recall",
                      "arguments":{"question":"retry backoff"}}}))
        .unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("relevant past session"), "got: {text}");
    assert!(text.contains("acme-api"), "got: {text}");
}

#[test]
fn tools_call_search_returns_results_via_daemon() {
    let s = server_with_daemon();
    let resp = s
        .handle(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"crossthreads_search",
                      "arguments":{"query":"retry backoff","mode":"hybrid","limit":5}}}))
        .unwrap();
    let result = &resp["result"];
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("result(s)"), "got: {text}");
    assert!(text.contains("acme-api"), "got: {text}");
}

#[test]
fn tools_call_status_reports_counts() {
    let s = server_with_daemon();
    let resp = s
        .handle(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"crossthreads_status","arguments":{}}}))
        .unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("conversation"), "got: {text}");
}

#[test]
fn unknown_method_is_a_jsonrpc_error() {
    let s = server_with_daemon();
    let resp = s
        .handle(&json!({"jsonrpc":"2.0","id":5,"method":"frobnicate"}))
        .unwrap();
    assert_eq!(resp["error"]["code"], -32601);
}

#[test]
fn search_without_daemon_reports_tool_error() {
    // Point at a dead address; the tool call should surface isError, not panic.
    let s = Server::new(Client::new("127.0.0.1:1")); // unused port
    let resp = s
        .handle(&json!({"jsonrpc":"2.0","id":6,"method":"tools/call",
            "params":{"name":"crossthreads_search","arguments":{"query":"x"}}}))
        .unwrap();
    assert_eq!(resp["result"]["isError"], true);
}
