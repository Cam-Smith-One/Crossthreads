//! `ct_mcp` — a minimal Model Context Protocol server exposing Crossthreads
//! search to MCP-capable agents (FR-UI-04, `docs/AGENT_API.md` §7).
//!
//! Speaks JSON-RPC 2.0 over stdio (newline-delimited). It is a thin client of
//! the daemon (ADR-005): tool calls are forwarded to `crossthreadsd` over the
//! loopback protocol, so the agent shares the same single index as everything
//! else.
//!
//! Implemented without an SDK to stay dependency-light; the surface is small
//! (`initialize`, `tools/list`, `tools/call`, `ping`).

use serde_json::{json, Value};

use ct_daemon::{Client, Mode, Request, Response};

/// MCP protocol version we implement (echoed if the client offers one).
pub const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "crossthreads";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The MCP server: forwards tool calls to a daemon [`Client`].
pub struct Server {
    client: Client,
}

impl Server {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Handle one incoming JSON-RPC message. Returns `Some(response)` for
    /// requests (those with an `id`) and `None` for notifications.
    pub fn handle(&self, msg: &Value) -> Option<Value> {
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

        // Notifications have no id and expect no response.
        let id = msg.get("id").cloned()?;

        let result = match method {
            "initialize" => Ok(self.initialize(msg)),
            "tools/list" => Ok(self.tools_list()),
            "tools/call" => self.tools_call(msg),
            "ping" => Ok(json!({})),
            other => Err((-32601, format!("method not found: {other}"))),
        };

        Some(match result {
            Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
            Err((code, message)) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            }
        })
    }

    fn initialize(&self, msg: &Value) -> Value {
        let version = msg
            .get("params")
            .and_then(|p| p.get("protocolVersion"))
            .and_then(|v| v.as_str())
            .unwrap_or(PROTOCOL_VERSION)
            .to_string();
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION }
        })
    }

    fn tools_list(&self) -> Value {
        json!({
            "tools": [
                {
                    "name": "crossthreads_search",
                    "description": "Search your past AI coding sessions across tools \
                        (Claude Code, Cursor, …). Use this to recall prior work, \
                        decisions, or implementations — e.g. \"where did we set up the \
                        OAuth refresh retry?\". Hybrid lexical + semantic search.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Natural-language or keyword query."
                            },
                            "mode": {
                                "type": "string",
                                "enum": ["lexical", "semantic", "hybrid"],
                                "description": "Retrieval mode (default: hybrid)."
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Max results (default: 5)."
                            }
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "crossthreads_status",
                    "description": "Report Crossthreads index health: how many \
                        conversations and embeddings are indexed.",
                    "inputSchema": { "type": "object", "properties": {} }
                }
            ]
        })
    }

    fn tools_call(&self, msg: &Value) -> Result<Value, (i64, String)> {
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(json!({}));

        match name {
            "crossthreads_search" => Ok(self.call_search(&args)),
            "crossthreads_status" => Ok(self.call_status()),
            other => Err((-32602, format!("unknown tool: {other}"))),
        }
    }

    fn call_search(&self, args: &Value) -> Value {
        let Some(query) = args.get("query").and_then(|q| q.as_str()) else {
            return tool_error("missing required argument: query");
        };
        let mode = match args.get("mode").and_then(|m| m.as_str()) {
            Some("lexical") => Mode::Lexical,
            Some("semantic") => Mode::Semantic,
            _ => Mode::Hybrid,
        };
        let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(5) as usize;

        match self.client.search(query, mode, limit) {
            Ok(hits) => tool_text(format_hits(query, &hits)),
            Err(e) => tool_error(format!(
                "search failed ({e:#}). Is crossthreadsd running?"
            )),
        }
    }

    fn call_status(&self) -> Value {
        match self.client.call(&Request::Status) {
            Ok(Response::Status { conversations, embeddings, embedder }) => tool_text(format!(
                "Crossthreads index: {conversations} conversations, {embeddings} embeddings ({embedder})."
            )),
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!("status failed ({e:#}). Is crossthreadsd running?")),
        }
    }
}

/// A successful tool result with a single text block.
fn tool_text(text: String) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ], "isError": false })
}

/// A tool result flagged as an error (per MCP, execution errors go in the
/// result with `isError: true`, not as a JSON-RPC error).
fn tool_error(message: impl Into<String>) -> Value {
    json!({ "content": [ { "type": "text", "text": message.into() } ], "isError": true })
}

fn format_hits(query: &str, hits: &[ct_daemon::SearchHit]) -> String {
    if hits.is_empty() {
        return format!("No prior sessions matched \"{query}\".");
    }
    let mut out = format!("{} result(s) for \"{query}\":\n", hits.len());
    for (i, h) in hits.iter().enumerate() {
        let when = h.started_at.as_deref().unwrap_or("").get(..10).unwrap_or("");
        out.push_str(&format!(
            "\n{}. [{}] {}{}\n   {}\n   {}\n",
            i + 1,
            h.tool,
            h.title.as_deref().unwrap_or("(untitled)"),
            if when.is_empty() { String::new() } else { format!("  {when}") },
            h.project.as_deref().unwrap_or("-"),
            h.snippet.replace('\n', " "),
        ));
    }
    out
}
