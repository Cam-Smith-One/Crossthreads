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

use ct_daemon::{Client, Filters, Mode, Request, Response, DEFAULT_ADDR};

/// MCP protocol version we implement (echoed if the client offers one).
pub const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "crossthreads";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The MCP server: forwards tool calls to a daemon [`Client`]. When configured
/// with an autostart address, it brings `crossthreadsd` up on demand so the
/// agent path is one step — no separate daemon to babysit.
pub struct Server {
    client: Client,
    autostart_addr: Option<String>,
    ensured: std::sync::atomic::AtomicBool,
}

impl Server {
    /// A server that talks to an already-running daemon (no autostart). Used by
    /// tests.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            autostart_addr: None,
            ensured: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// A server that auto-starts `crossthreadsd` at `addr` on the first tool
    /// call if it isn't already running.
    pub fn with_autostart(client: Client, addr: impl Into<String>) -> Self {
        Self {
            client,
            autostart_addr: Some(addr.into()),
            ensured: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Convenience: build from the environment (`CROSSTHREADS_ADDR`) with
    /// autostart enabled — the configuration `ct-mcp` uses in production.
    pub fn from_env_autostart() -> Self {
        let addr = std::env::var("CROSSTHREADS_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
        Self::with_autostart(Client::new(addr.clone()), addr)
    }

    /// Ensure a daemon is reachable, spawning a detached `crossthreadsd` once if
    /// not. Cheap when one is already running (a single loopback ping).
    fn ensure_daemon(&self) {
        use std::sync::atomic::Ordering;
        let Some(addr) = self.autostart_addr.as_deref() else {
            return;
        };
        // Fast path: already up.
        if self.client.call(&Request::Ping).is_ok() {
            return;
        }
        // Spawn at most one at a time; later calls take the fast-path ping above.
        if self.ensured.swap(true, Ordering::SeqCst) {
            return;
        }
        spawn_daemon(addr);
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if self.client.call(&Request::Ping).is_ok() {
                return;
            }
        }
        // The spawn didn't come up; clear the latch so a later call can retry
        // (otherwise a one-time failed spawn would wedge autostart for good).
        self.ensured.store(false, Ordering::SeqCst);
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
                            },
                            "tool": {
                                "type": "string",
                                "description": "Filter to one tool, e.g. claude-code, cursor, aider, codex."
                            },
                            "project": {
                                "type": "string",
                                "description": "Filter to projects whose path contains this substring."
                            },
                            "devices": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Cross-device search: names of the devices to search \
                                    (from crossthreads_devices). Omit to search this device and all \
                                    reachable peers."
                            }
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "crossthreads_recall",
                    "description": "Recall what was discussed or decided in past \
                        sessions about a topic — answer-oriented. Returns a concise \
                        digest of the most relevant past messages. Use for questions \
                        like \"what did we decide about the queue?\".",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string", "description": "What to recall." },
                            "limit": { "type": "integer", "description": "Max sources (default: 5)." }
                        },
                        "required": ["question"]
                    }
                },
                {
                    "name": "crossthreads_build_context",
                    "description": "Build a paste-ready context block from the past \
                        sessions most relevant to a query, for injecting prior work \
                        into the current task (\"resume where we left off\"). Returns \
                        markdown.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "What to pull context for." },
                            "mode": { "type": "string", "enum": ["lexical", "semantic", "hybrid"] },
                            "limit": { "type": "integer", "description": "Max conversations (default: 3)." },
                            "max_chars": { "type": "integer", "description": "Budget (default: 6000)." }
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "crossthreads_status",
                    "description": "Report Crossthreads index health: how many \
                        conversations and embeddings are indexed.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "crossthreads_devices",
                    "description": "List the devices available to search (this machine \
                        plus approved peers) and whether each is online. Use to discover \
                        the device names accepted by the other tools' `devices` argument.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "crossthreads_themes",
                    "description": "Cluster the user's indexed AI coding sessions into \
                        themes and return each theme's label, size, tool mix, and a few \
                        sample titles. Use to get an overview of what the user has been \
                        working on across tools.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "k": {
                                "type": "integer",
                                "description": "Number of themes (clusters) to produce. Default 8.",
                                "minimum": 1
                            }
                        }
                    }
                }
            ]
        })
    }

    fn tools_call(&self, msg: &Value) -> Result<Value, (i64, String)> {
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(json!({}));

        // Every tool below hits the daemon — make sure one is running first.
        self.ensure_daemon();

        match name {
            "crossthreads_search" => Ok(self.call_search(&args)),
            "crossthreads_recall" => Ok(self.call_recall(&args)),
            "crossthreads_build_context" => Ok(self.call_build_context(&args)),
            "crossthreads_status" => Ok(self.call_status()),
            "crossthreads_devices" => Ok(self.call_devices()),
            "crossthreads_themes" => Ok(self.call_themes(&args)),
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

        match self
            .client
            .search_devices(query, mode, limit, filters_from(args), devices_from(args))
        {
            Ok(hits) => tool_text(format_hits(query, &hits)),
            Err(e) => tool_error(format!("search failed ({e:#}). Is crossthreadsd running?")),
        }
    }

    fn call_recall(&self, args: &Value) -> Value {
        let Some(question) = args.get("question").and_then(|q| q.as_str()) else {
            return tool_error("missing required argument: question");
        };
        let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(5) as usize;
        match self.client.search_devices(
            question,
            Mode::Hybrid,
            limit,
            filters_from(args),
            devices_from(args),
        ) {
            Ok(hits) if hits.is_empty() => {
                tool_text(format!("No past sessions found about \"{question}\"."))
            }
            Ok(hits) => {
                let mut out = format!(
                    "Found {} relevant past session(s) for \"{question}\":\n",
                    hits.len()
                );
                for (i, h) in hits.iter().enumerate() {
                    out.push_str(&format!(
                        "\n{}. [{}] {} — {}\n   {}\n",
                        i + 1,
                        h.tool,
                        h.title.as_deref().unwrap_or("(untitled)"),
                        h.project.as_deref().unwrap_or("-"),
                        h.snippet.replace('\n', " "),
                    ));
                }
                tool_text(out)
            }
            Err(e) => tool_error(format!("recall failed ({e:#}). Is crossthreadsd running?")),
        }
    }

    fn call_build_context(&self, args: &Value) -> Value {
        let Some(query) = args.get("query").and_then(|q| q.as_str()) else {
            return tool_error("missing required argument: query");
        };
        let mode = match args.get("mode").and_then(|m| m.as_str()) {
            Some("lexical") => Mode::Lexical,
            Some("semantic") => Mode::Semantic,
            _ => Mode::Hybrid,
        };
        let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(3) as usize;
        let max_chars = args
            .get("max_chars")
            .and_then(|l| l.as_u64())
            .unwrap_or(6000) as usize;

        match self.client.build_context_devices(
            query,
            mode,
            limit,
            max_chars,
            filters_from(args),
            devices_from(args),
        ) {
            Ok((markdown, sources)) if sources.is_empty() => tool_text(format!(
                "No prior context found for \"{query}\".\n{markdown}"
            )),
            Ok((markdown, _)) => tool_text(markdown),
            Err(e) => tool_error(format!(
                "build_context failed ({e:#}). Is crossthreadsd running?"
            )),
        }
    }

    fn call_devices(&self) -> Value {
        match self.client.call(&Request::Devices) {
            Ok(Response::Devices {
                devices,
                federation,
            }) => {
                if !federation {
                    return tool_text(
                        "Cross-device search isn't enabled on this machine, so only \
                         this device is searched."
                            .to_string(),
                    );
                }
                let mut out = format!("{} device(s) available to search:\n", devices.len());
                for d in &devices {
                    out.push_str(&format!(
                        "\n- {}{} — {}",
                        d.name,
                        if d.local { " (this device)" } else { "" },
                        if d.online { "online" } else { "offline" },
                    ));
                }
                tool_text(out)
            }
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!("devices failed ({e:#}). Is crossthreadsd running?")),
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

    fn call_themes(&self, args: &Value) -> Value {
        let k = args.get("k").and_then(|v| v.as_u64()).map(|n| n as usize);
        match self.client.call(&Request::Themes { k }) {
            Ok(Response::Themes { themes }) => {
                if themes.is_empty() {
                    return tool_text(
                        "No themes yet — the index has no embedded conversations. \
                         Run `crossthreads index` first."
                            .to_string(),
                    );
                }
                let total: usize = themes.iter().map(|t| t.size).sum();
                let mut out = format!("{} themes across {total} conversations:\n", themes.len());
                for t in &themes {
                    let tools = t
                        .tools
                        .iter()
                        .take(3)
                        .map(|(name, n)| format!("{name} {n}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!(
                        "\n## {} ({} sessions · {tools})\n",
                        t.label, t.size
                    ));
                    for s in t.samples.iter().take(4) {
                        out.push_str(&format!(
                            "  - {}\n",
                            s.title.as_deref().unwrap_or("(untitled)").trim()
                        ));
                    }
                }
                tool_text(out)
            }
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!("themes failed ({e:#}). Is crossthreadsd running?")),
        }
    }
}

/// Optional `devices` routing arg (ADR-010): the device names to search.
/// Absent ⇒ this device + all reachable peers.
fn devices_from(args: &Value) -> Option<Vec<String>> {
    let arr = args.get("devices")?.as_array()?;
    let names: Vec<String> = arr
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    (!names.is_empty()).then_some(names)
}

/// Optional `tool` / `project` filter args, shared by the search-style tools.
fn filters_from(args: &Value) -> Filters {
    let s = |k: &str| args.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    Filters {
        tool: s("tool"),
        kind: s("kind"),
        project: s("project"),
        since: s("since"),
        until: s("until"),
        tag: s("tag"),
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

/// Spawn a detached `crossthreadsd` listening on `addr`. Best effort — failures
/// are surfaced later as the tool's "is the daemon running?" error.
///
/// The daemon's stdio is sent to null so it can never corrupt our JSON-RPC
/// stream, and it's put in its own process group so it outlives this process.
/// If the bundled web UI is found next to the binary, it's served too, so the
/// single daemon covers both the agent and browser surfaces.
fn spawn_daemon(addr: &str) {
    use std::process::{Command, Stdio};

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    // Prefer a `crossthreadsd` sibling of this binary; else rely on PATH.
    let program = exe_dir
        .as_ref()
        .map(|d| d.join(daemon_binary()))
        .filter(|p| p.exists())
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| daemon_binary().into());

    let mut cmd = Command::new(program);
    cmd.arg("--addr").arg(addr);
    if let Some(ui) = exe_dir.as_deref().and_then(bundled_ui_dir) {
        // Also serve the web UI so the same daemon backs the browser app.
        cmd.arg("--http").arg("127.0.0.1:47101").arg("--ui").arg(ui);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // detach: survive our exit, ignore our signals
    }
    let _ = cmd.spawn();
}

fn daemon_binary() -> &'static str {
    if cfg!(windows) {
        "crossthreadsd.exe"
    } else {
        "crossthreadsd"
    }
}

/// Locate the built web UI relative to the binary, for both the installed
/// release layout (`<bin>/../ui`) and a source build (`target/release/../../ui/dist`).
fn bundled_ui_dir(exe_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let candidates = [
        exe_dir.join("..").join("ui"), // release: ~/.crossthreads/ui
        exe_dir.join("..").join("..").join("ui").join("dist"), // source: repo/ui/dist
    ];
    candidates
        .into_iter()
        .find(|p| p.join("index.html").is_file())
}

fn format_hits(query: &str, hits: &[ct_daemon::SearchHit]) -> String {
    if hits.is_empty() {
        return format!("No prior sessions matched \"{query}\".");
    }
    let mut out = format!("{} result(s) for \"{query}\":\n", hits.len());
    for (i, h) in hits.iter().enumerate() {
        let when = h
            .started_at
            .as_deref()
            .unwrap_or("")
            .get(..10)
            .unwrap_or("");
        out.push_str(&format!(
            "\n{}. [{}] {}{}\n   {}\n   {}\n",
            i + 1,
            h.tool,
            h.title.as_deref().unwrap_or("(untitled)"),
            if when.is_empty() {
                String::new()
            } else {
                format!("  {when}")
            },
            h.project.as_deref().unwrap_or("-"),
            h.snippet.replace('\n', " "),
        ));
    }
    out
}

#[cfg(test)]
mod autostart_tests {
    use super::{bundled_ui_dir, daemon_binary};

    #[test]
    fn finds_bundled_ui_in_both_layouts() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();

        // Nothing there yet.
        assert!(bundled_ui_dir(&bin).is_none());

        // Release layout: <bin>/../ui/index.html
        let rel_ui = tmp.path().join("ui");
        std::fs::create_dir_all(&rel_ui).unwrap();
        std::fs::write(rel_ui.join("index.html"), "<html>").unwrap();
        let got = bundled_ui_dir(&bin).expect("should find the ui dir");
        assert!(got.join("index.html").is_file());
        assert_eq!(got.canonicalize().unwrap(), rel_ui.canonicalize().unwrap());
    }

    #[test]
    fn daemon_binary_has_platform_suffix() {
        let name = daemon_binary();
        assert!(name.starts_with("crossthreadsd"));
        assert_eq!(name.ends_with(".exe"), cfg!(windows));
    }
}
