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
