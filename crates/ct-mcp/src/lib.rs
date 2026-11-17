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
            "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
            // The room protocol travels with the server, so agents follow it
            // even without the skill installed.
            "instructions": ct_daemon::rooms::INSTRUCTIONS
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
                            },
                            "name": {
                                "type": "boolean",
                                "description": "Label each theme with a short LLM-generated \
                                    name instead of keyword terms (needs a model login)."
                            }
                        }
                    }
                },
                {
                    "name": "crossthreads_open_loops",
                    "description": "Surface UNRESOLVED work across the user's recent AI \
                        coding sessions: unanswered questions, half-finished tasks, \
                        dangling TODOs, and errors never confirmed fixed. Use at the start \
                        of a work session to see what to pick back up. Needs a model login.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {
                                "type": "integer",
                                "description": "How many recent sessions to scan (default ~40)."
                            }
                        }
                    }
                },
                {
                    "name": "crossthreads_knowledge_cards",
                    "description": "Distill durable, reusable KNOWLEDGE CARDS (canonical \
                        Q→A pairs) from things the user solved or decided across recent \
                        sessions. Use to capture what's worth remembering. Needs a model login.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {
                                "type": "integer",
                                "description": "How many recent sessions to scan (default ~80)."
                            }
                        }
                    }
                },
                {
                    "name": "crossthreads_decision_log",
                    "description": "Extract notable DECISIONS (\"chose X over Y because Z\") \
                        with rationale from the user's recent sessions — a lightweight, \
                        auto-built decision log / ADR feed. Needs a model login.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {
                                "type": "integer",
                                "description": "How many recent sessions to scan (default ~80)."
                            }
                        }
                    }
                },
                {
                    "name": "crossthreads_recurring",
                    "description": "Surface RECURRING problems and patterns — things the user \
                        keeps hitting across sessions (the same bug class, repeated setup \
                        friction, a re-asked question, a re-derived workaround), each with a \
                        suggestion to break the loop. Use to spot what's worth automating or \
                        documenting. Needs a model login.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {
                                "type": "integer",
                                "description": "How many recent sessions to scan (default ~400)."
                            }
                        }
                    }
                },
                {
                    "name": "crossthreads_ask",
                    "description": "Ask a natural-language question and get a synthesized, cited \
                        answer drawn from the user's own past AI coding sessions across every \
                        tool (and device) — e.g. \"how did we end up handling token refresh?\". \
                        Higher-level than search/recall: it retrieves the most relevant sessions \
                        and answers from them, citing which session each claim came from. With a \
                        model it synthesizes; without one it returns the retrieved context.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string", "description": "The question to answer." },
                            "limit": {
                                "type": "integer",
                                "description": "How many past sessions to draw on (default 6)."
                            },
                            "tool": { "type": "string", "description": "Filter to one tool." },
                            "project": {
                                "type": "string",
                                "description": "Filter to projects whose path contains this substring."
                            },
                            "devices": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Cross-device: device names to draw from (omit for all reachable)."
                            }
                        },
                        "required": ["question"]
                    }
                },
                {
                    "name": "crossthreads_threads",
                    "description": "List the user's threads: pieces of work, each one or more \
                        sessions linked across tools (e.g. a Claude Code session continued in \
                        Codex), most recently active first. Use to see what the user has been \
                        working on and where it stands. Offline; no model needed.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "status": {
                                "type": "string",
                                "enum": ["active", "parked", "done"],
                                "description": "Only threads with this status."
                            },
                            "tool": { "type": "string", "description": "Only threads this tool took part in." },
                            "repo": { "type": "string", "description": "Only repositories whose name contains this." },
                            "limit": { "type": "integer", "description": "Max threads (default 20)." }
                        }
                    }
                },
                {
                    "name": "crossthreads_thread",
                    "description": "Show one thread in full: every session that touched the work, \
                        in time order across tools, with how they are linked. Pass a thread id \
                        (from crossthreads_threads) or the id of any session in it (from \
                        crossthreads_search). Offline; no model needed.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "description": "Thread id or session id." },
                            "handoff": {
                                "type": "boolean",
                                "description": "Return a paste-ready handoff instead: the goal, every \
                                    session, and where the latest one left off. Use it to continue \
                                    work another tool or agent started."
                            }
                        },
                        "required": ["id"]
                    }
                },
                {
                    "name": "crossthreads_thread_context",
                    "description": "What you should know about the work you're in: the thread \
                        this folder's sessions belong to, across tools (Claude Code, Codex, …) and \
                        across the user's devices — its decisions, open todos, notes, and the \
                        other sessions in it (who did what, which are running now). Call it at \
                        the start of a task, or before picking up work another agent started. \
                        Defaults to the thread for your working directory.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "thread": {
                                "type": "string",
                                "description": "A thread id (th_…) or session id. Omit for the thread of your working directory."
                            },
                            "cwd": {
                                "type": "string",
                                "description": "Working directory to find the thread for (default: this server's)."
                            }
                        }
                    }
                },
                {
                    "name": "crossthreads_note",
                    "description": "Keep a note on the current thread so other agents (in any \
                        tool) and the user's other devices see it: a decision and why, a todo \
                        left open, context someone picking this up needs. Mark a todo done by \
                        passing its id with kind \"done\". Keep notes short and factual.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "string",
                                "description": "The note (one or two sentences)."
                            },
                            "kind": {
                                "type": "string",
                                "enum": ["decision", "todo", "done", "context"],
                                "description": "What sort of note (default: context)."
                            },
                            "id": {
                                "type": "string",
                                "description": "An existing note's id (n_…) to change its kind, e.g. a todo to done. Then text is ignored."
                            },
                            "me": {
                                "type": "string",
                                "description": "Your name, e.g. claude or codex."
                            },
                            "thread": {
                                "type": "string",
                                "description": "A thread id. Omit for the thread of your working directory."
                            },
                            "cwd": {
                                "type": "string",
                                "description": "Working directory (default: this server's)."
                            }
                        }
                    }
                },
                {
                    "name": "crossthreads_room_send",
                    "description": "Post a message to another AI agent in this project's room, \
                        then block until the reply arrives and return it. The main room tool: \
                        one call per turn. Use to brief, hand off to, or answer another agent \
                        (Claude, Codex, ...) without the user copy-pasting. Start the text with \
                        DONE to close the exchange.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "me": { "type": "string", "description": "Your agent name, e.g. claude or codex." },
                            "text": { "type": "string", "description": "The message. Short: what you did, what you need, what's next." },
                            "room": { "type": "string", "description": "Your project root path (git rev-parse --show-toplevel), or a room name." },
                            "to": { "type": "string", "description": "Recipient name; omit to broadcast." },
                            "wait": { "type": "boolean", "description": "Block for the reply (default true)." },
                            "timeout": { "type": "integer", "description": "Seconds to wait for a reply (default 50)." }
                        },
                        "required": ["me", "text", "room"]
                    }
                },
                {
                    "name": "crossthreads_room_wait",
                    "description": "Return your unread room messages, blocking up to `timeout` \
                        seconds until one arrives. Use to listen for another agent's instructions. \
                        Don't call it straight after crossthreads_room_send, which already waits.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "me": { "type": "string", "description": "Your agent name." },
                            "room": { "type": "string", "description": "Your project root path, or a room name." },
                            "timeout": { "type": "integer", "description": "Seconds to wait (default 50, max 600; 0 = just check)." }
                        },
                        "required": ["me", "room"]
                    }
                },
                {
                    "name": "crossthreads_room_history",
                    "description": "Catch up on a room: who is here, turns used, active file \
                        claims, and the last messages. Call once when you join.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "me": { "type": "string", "description": "Your agent name." },
                            "room": { "type": "string", "description": "Your project root path, or a room name." },
                            "limit": { "type": "integer", "description": "How many messages (default 30)." }
                        },
                        "required": ["room"]
                    }
                },
                {
                    "name": "crossthreads_claim",
                    "description": "Reserve files or folders (absolute paths) before editing them, \
                        so another agent doesn't edit them at the same time. Paths someone else \
                        holds are reported as conflicts and not claimed. Claim again to extend. \
                        Claims expire, and are released when you send DONE.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "me": { "type": "string", "description": "Your agent name." },
                            "paths": { "type": "array", "items": { "type": "string" }, "description": "Absolute file or folder paths." },
                            "room": { "type": "string", "description": "Your project root path, or a room name." },
                            "minutes": { "type": "integer", "description": "How long to hold them (default 20, max 240)." }
                        },
                        "required": ["me", "paths", "room"]
                    }
                },
                {
                    "name": "crossthreads_release",
                    "description": "Release your file claims on `paths`, or all of them if omitted.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "me": { "type": "string", "description": "Your agent name." },
                            "paths": { "type": "array", "items": { "type": "string" }, "description": "Absolute paths to release; omit for all." }
                        },
                        "required": ["me"]
                    }
                },
                {
                    "name": "crossthreads_activity",
                    "description": "Report the user's session activity over time, bucketed by day \
                        or week, with a per-tool breakdown. Use to see when and with which tools \
                        the user has been active. Offline; no model needed.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "bucket": {
                                "type": "string",
                                "enum": ["day", "week"],
                                "description": "Time bucket (default day)."
                            },
                            "tool": { "type": "string", "description": "Filter to one tool." },
                            "project": { "type": "string", "description": "Filter by project substring." },
                            "since": { "type": "string", "description": "Only on/after this date (YYYY-MM-DD)." },
                            "until": { "type": "string", "description": "Only on/before this date (YYYY-MM-DD)." }
                        }
                    }
                },
                {
                    "name": "crossthreads_metrics",
                    "description": "Hard behavioral metrics on HOW the user works, computed across \
                        their whole history (no model): turns per task, correction/rework rate, \
                        first-prompt-miss rate, abandonment, opening-prompt specificity, code/error \
                        paste rates, test/commit mention rates, tempo by hour, and the \
                        highest-friction projects. Use this to ground any 'how does this user \
                        work' reasoning in real numbers.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "tool": { "type": "string", "description": "Filter to one tool." },
                            "project": { "type": "string", "description": "Filter by project substring." },
                            "since": { "type": "string", "description": "Only on/after this date (YYYY-MM-DD)." },
                            "until": { "type": "string", "description": "Only on/before this date (YYYY-MM-DD)." }
                        }
                    }
                },
                {
                    "name": "crossthreads_work_dna",
                    "description": "A quantified, evidence-backed profile of how the user works — \
                        their rhythm, where friction concentrates and why, how they open tasks, and \
                        signature habits — grounded in the behavioral metrics. Needs a model login.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "crossthreads_gaps",
                    "description": "The user's gaps and blind spots: practices largely missing (e.g. \
                        tests, commits), work they start but don't finish, and friction they keep \
                        absorbing — plus a personalized draft CLAUDE.md to close them. Grounded in \
                        the metrics. Needs a model login.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "crossthreads_prompt_coach",
                    "description": "Prompt-craft coaching from the user's own history: contrasts their \
                        smoothest vs roughest sessions (by friction) and gives concrete tips with \
                        before→after rewrites drawn from their actual prompts. Needs a model login.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "crossthreads_process_miner",
                    "description": "Mine repeatable procedures the user keeps re-deriving across \
                        sessions and turn the strongest ones into draft, installable SKILL.md \
                        artifacts. Use to find what's worth automating. Needs a model login.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "crossthreads_weekly_review",
                    "description": "The user's weekly review — what they worked on in the last 7 \
                        days, how they worked (grounded in metrics), and one concrete thing to try \
                        next week. Returns a cached review when fresh; pass force=true to \
                        regenerate. Uses the model when configured, else returns the week's metrics.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "force": {
                                "type": "boolean",
                                "description": "Regenerate even if a fresh cached review exists."
                            }
                        }
                    }
                },
                {
                    "name": "crossthreads_trends",
                    "description": "How the user's working metrics are trending — the last 30 days \
                        vs the previous 30 — with direction (better/worse) per metric: rework, \
                        first-prompt miss, unresolved rate, prompt specificity, tests, context \
                        switching, median turns. Deterministic; no model.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "days": {
                                "type": "integer",
                                "description": "Window size in days to compare (default 30)."
                            }
                        }
                    }
                },
                {
                    "name": "crossthreads_optimize",
                    "description": "The 'optimize how you work' loop: the single highest-impact \
                        change the user should make right now (a concrete, testable prescription \
                        grounded in their metrics), a measured verdict on the last change they \
                        tried (did the metric actually move?), and their trends. Use to give \
                        focused, evidence-backed, actionable coaching. Deterministic; no model.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "force": {
                                "type": "boolean",
                                "description": "Grade the active prescription now instead of waiting ~7 days."
                            }
                        }
                    }
                },
                {
                    "name": "crossthreads_fluency",
                    "description": "The user's AI-fluency across four dimensions — Delegation \
                        (what they hand to the agent), Description (how clearly they instruct), \
                        Discernment (how well they evaluate output), and Diligence (verified, \
                        responsible use) — each scored 0–100 with its movement vs. the previous \
                        window, plus a single reflective question about what they might want to \
                        keep doing themselves. Use for a structured, self-understanding read (the \
                        reflective complement to optimize's prescriptions). Deterministic; no model.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "window_days": {
                                "type": "integer",
                                "description": "Window size in days to score over (default 30)."
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
            "crossthreads_open_loops" => Ok(self.call_insight("open_loops", &args)),
            "crossthreads_knowledge_cards" => Ok(self.call_insight("knowledge_cards", &args)),
            "crossthreads_decision_log" => Ok(self.call_insight("decision_log", &args)),
            "crossthreads_recurring" => Ok(self.call_insight("recurrence", &args)),
            "crossthreads_ask" => Ok(self.call_ask(&args)),
            "crossthreads_threads" => Ok(self.call_threads(&args)),
            "crossthreads_thread" => Ok(self.call_thread(&args)),
            "crossthreads_thread_context" => Ok(self.call_thread_context(&args)),
            "crossthreads_note" => Ok(self.call_note(&args)),
            "crossthreads_room_send"
            | "crossthreads_room_wait"
            | "crossthreads_room_history"
            | "crossthreads_claim"
            | "crossthreads_release" => Ok(self.call_room(name, &args)),
            "crossthreads_activity" => Ok(self.call_activity(&args)),
            "crossthreads_metrics" => Ok(self.call_metrics(&args)),
            "crossthreads_work_dna" => Ok(self.call_insight("work_dna", &args)),
            "crossthreads_gaps" => Ok(self.call_insight("gaps", &args)),
            "crossthreads_prompt_coach" => Ok(self.call_insight("prompt_coach", &args)),
            "crossthreads_process_miner" => Ok(self.call_insight("process_miner", &args)),
            "crossthreads_weekly_review" => Ok(self.call_weekly(&args)),
            "crossthreads_trends" => Ok(self.call_trends(&args)),
            "crossthreads_optimize" => Ok(self.call_optimize(&args)),
            "crossthreads_fluency" => Ok(self.call_fluency(&args)),
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
            Ok(Response::Status {
                conversations,
                embeddings,
                embedder,
                tools,
            }) => {
                let mix = tools
                    .iter()
                    .map(|(t, threads, skills)| {
                        if *skills > 0 {
                            format!("{t} {threads} (+{skills} skills)")
                        } else {
                            format!("{t} {threads}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                tool_text(format!(
                    "Crossthreads index: {conversations} conversations, {embeddings} embeddings ({embedder}).\nIndexed tools: {mix}."
                ))
            }
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!("status failed ({e:#}). Is crossthreadsd running?")),
        }
    }

    fn call_themes(&self, args: &Value) -> Value {
        let k = args.get("k").and_then(|v| v.as_u64()).map(|n| n as usize);
        let name = args.get("name").and_then(|v| v.as_bool()).unwrap_or(false);
        match self.client.call(&Request::Themes { k, name }) {
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

    fn call_insight(&self, kind: &str, args: &Value) -> Value {
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        match self.client.call(&Request::Insight {
            kind: kind.to_string(),
            limit,
        }) {
            Ok(Response::Insight { markdown, sources }) => {
                if sources.is_empty() {
                    tool_text(markdown)
                } else {
                    tool_text(format!(
                        "{markdown}\n\n_Drawn from {} recent session(s)._",
                        sources.len()
                    ))
                }
            }
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!(
                "{kind} failed ({e:#}). Needs a model login (Settings → Models) and a \
                 running daemon."
            )),
        }
    }

    fn call_ask(&self, args: &Value) -> Value {
        let Some(question) = args.get("question").and_then(|q| q.as_str()) else {
            return tool_error("missing required argument: question");
        };
        let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(6) as usize;
        match self.client.call(&Request::Ask {
            question: question.to_string(),
            limit,
            filters: filters_from(args),
            devices: devices_from(args),
        }) {
            Ok(Response::Answer {
                markdown, sources, ..
            }) => {
                if sources.is_empty() {
                    tool_text(markdown)
                } else {
                    tool_text(format!(
                        "{markdown}\n\n_Answered from {} past session(s)._",
                        sources.len()
                    ))
                }
            }
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!("ask failed ({e:#}). Is crossthreadsd running?")),
        }
    }

    fn call_threads(&self, args: &Value) -> Value {
        let text = |k: &str| args.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let filters = ct_daemon::ThreadFilters {
            status: text("status"),
            tool: text("tool"),
            repo: text("repo"),
            limit: Some(args.get("limit").and_then(|l| l.as_u64()).unwrap_or(20) as usize),
        };
        match self.client.call(&Request::Threads {
            filters,
            devices: None,
        }) {
            Ok(Response::Threads { threads, .. }) => {
                if threads.is_empty() {
                    return tool_text("No threads match.".to_string());
                }
                let mut out = format!("{} thread(s), most recently active first:\n", threads.len());
                for t in &threads {
                    let live = match t.live.as_ref().map(|l| l.state.as_str()) {
                        Some("needs_you") => " — WAITING ON THE USER",
                        Some("running") => " — running now",
                        _ => "",
                    };
                    let device = t
                        .device
                        .as_deref()
                        .map(|d| format!(" · on {d}"))
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "\n- {} [{}] {}{live}\n  {} · {} · {} session(s) · last active {}{device}",
                        t.id,
                        t.status,
                        t.title,
                        t.repo_label.as_deref().unwrap_or("no repository"),
                        t.tools.join(" + "),
                        t.session_count,
                        t.last_activity
                            .as_deref()
                            .map(|d| &d[..d.len().min(10)])
                            .unwrap_or("unknown"),
                    ));
                }
                tool_text(out)
            }
            Ok(Response::Error { message }) => tool_error(message),
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!("threads failed ({e:#}). Is crossthreadsd running?")),
        }
    }

    fn call_thread(&self, args: &Value) -> Value {
        let Some(id) = args.get("id").and_then(|q| q.as_str()) else {
            return tool_error("missing required argument: id".to_string());
        };
        if args.get("handoff").and_then(|h| h.as_bool()) == Some(true) {
            return match self.client.call(&Request::ThreadHandoff {
                id: id.to_string(),
                device: None,
                max_chars: 8000,
            }) {
                Ok(Response::Context { markdown, .. }) => tool_text(markdown),
                Ok(Response::Error { message }) => tool_error(message),
                Ok(other) => tool_error(format!("unexpected response: {other:?}")),
                Err(e) => tool_error(format!("handoff failed ({e:#}). Is crossthreadsd running?")),
            };
        }
        match self.client.call(&Request::Thread {
            id: id.to_string(),
            device: None,
        }) {
            Ok(Response::Thread { thread: None }) => {
                tool_text(format!("No thread or session with id {id}."))
            }
            Ok(Response::Thread { thread: Some(t) }) => {
                let s = &t.summary;
                let mut out = format!(
                    "{} [{}]\n{} · {} · {} session(s), {} messages\n\nSessions, oldest first:",
                    s.title,
                    s.status,
                    s.id,
                    s.repo_label.as_deref().unwrap_or("no repository"),
                    s.session_count,
                    s.message_count,
                );
                for sess in &t.sessions {
                    out.push_str(&format!(
                        "\n- {} {}{} {} — {}",
                        sess.started_at
                            .as_deref()
                            .map(|d| &d[..d.len().min(10)])
                            .unwrap_or("undated"),
                        sess.tool,
                        sess.device
                            .as_deref()
                            .filter(|_| !t.parts.is_empty())
                            .map(|d| format!(" on {d}"))
                            .unwrap_or_default(),
                        sess.conversation_id,
                        sess.title.as_deref().unwrap_or("(untitled)"),
                    ));
                }
                for l in &t.links {
                    out.push_str(&format!(
                        "\n\n{} → {}: {} ({})",
                        l.from, l.to, l.kind, l.origin
                    ));
                }
                if !t.notes.is_empty() {
                    out.push_str("\n\nNotes:");
                    for n in &t.notes {
                        out.push_str(&format!(
                            "\n- [{}] {} {}: {}",
                            n.id, n.kind, n.author, n.text
                        ));
                    }
                }
                if !t.suggestions.is_empty() {
                    out.push_str(&format!(
                        "\n\n{} possibly related session(s) not yet linked.",
                        t.suggestions.len()
                    ));
                }
                tool_text(out)
            }
            Ok(Response::Error { message }) => tool_error(message),
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!("thread failed ({e:#}). Is crossthreadsd running?")),
        }
    }

    fn call_thread_context(&self, args: &Value) -> Value {
        let thread = args
            .get("thread")
            .and_then(Value::as_str)
            .map(str::to_string);
        match self.client.call(&Request::ThreadContext {
            thread,
            device: None,
            cwd: cwd_from(args),
            brief: false,
        }) {
            Ok(Response::ThreadContext { markdown, .. }) => tool_text(markdown),
            Ok(Response::Error { message }) => tool_error(message),
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!(
                "thread context failed ({e:#}). Is crossthreadsd running?"
            )),
        }
    }

    fn call_note(&self, args: &Value) -> Value {
        let kind = args
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("context")
            .to_string();
        let req = if let Some(id) = args.get("id").and_then(Value::as_str) {
            Request::ThreadNoteEdit {
                id: id.to_string(),
                kind: Some(kind),
                removed: false,
            }
        } else {
            let Some(text) = args.get("text").and_then(Value::as_str) else {
                return tool_error("missing required argument: text");
            };
            Request::ThreadNoteAdd {
                thread: args
                    .get("thread")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                device: None,
                cwd: cwd_from(args),
                kind,
                text: text.to_string(),
                author: args.get("me").and_then(Value::as_str).map(str::to_string),
            }
        };
        match self.client.call(&req) {
            Ok(Response::ThreadNote { note }) => tool_text(format!(
                "Noted on thread {} as {} [{}]. Other agents and devices will see it.",
                note.thread_id, note.kind, note.id
            )),
            Ok(Response::Error { message }) => tool_error(message),
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!("note failed ({e:#}). Is crossthreadsd running?")),
        }
    }

    /// The room tools: hand the arguments to the daemon and return its text.
    /// `send` and `wait` block by design, so the daemon gets that long to answer.
    fn call_room(&self, name: &str, args: &Value) -> Value {
        let op = match name {
            "crossthreads_room_send" => "room_send",
            "crossthreads_room_wait" => "room_wait",
            "crossthreads_room_history" => "room_history",
            "crossthreads_claim" => "room_claim",
            _ => "room_release",
        };
        let mut body = match args {
            Value::Object(map) => map.clone(),
            _ => serde_json::Map::new(),
        };
        body.insert("op".into(), json!(op));
        let req: Request = match serde_json::from_value(Value::Object(body)) {
            Ok(req) => req,
            Err(e) => return tool_error(format!("bad arguments for {name}: {e}")),
        };
        let wait = args.get("timeout").and_then(|t| t.as_u64()).unwrap_or(50);
        match self
            .client
            .call_timeout(&req, std::time::Duration::from_secs(wait.min(600) + 15))
        {
            Ok(Response::Text { text }) => tool_text(text),
            Ok(Response::Error { message }) => tool_error(message),
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!("{name} failed ({e:#}). Is crossthreadsd running?")),
        }
    }

    fn call_activity(&self, args: &Value) -> Value {
        let bucket = args
            .get("bucket")
            .and_then(|b| b.as_str())
            .map(str::to_string);
        match self.client.call(&Request::Activity {
            bucket,
            filters: filters_from(args),
        }) {
            Ok(Response::Activity { bucket, periods }) => {
                if periods.is_empty() {
                    return tool_text("No dated sessions to chart yet.".to_string());
                }
                let total: i64 = periods.iter().map(|p| p.total).sum();
                let mut out = format!(
                    "Activity by {bucket} ({} period(s), {total} sessions):\n",
                    periods.len()
                );
                for p in &periods {
                    let tools = p
                        .by_tool
                        .iter()
                        .take(4)
                        .map(|(t, n)| format!("{t} {n}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!(
                        "\n{}  ·  {} session(s)  ·  {tools}",
                        p.period, p.total
                    ));
                }
                tool_text(out)
            }
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!(
                "activity failed ({e:#}). Is crossthreadsd running?"
            )),
        }
    }

    fn call_weekly(&self, args: &Value) -> Value {
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        match self.client.call(&Request::WeeklyReview { force }) {
            Ok(Response::WeeklyReview { markdown, .. }) => tool_text(markdown),
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!(
                "weekly_review failed ({e:#}). Is crossthreadsd running?"
            )),
        }
    }

    fn call_optimize(&self, args: &Value) -> Value {
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        match self.client.call(&Request::Optimize { force }) {
            Ok(Response::Optimize { markdown, .. }) => tool_text(markdown),
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!(
                "optimize failed ({e:#}). Is crossthreadsd running?"
            )),
        }
    }

    fn call_fluency(&self, args: &Value) -> Value {
        let window_days = args.get("window_days").and_then(|v| v.as_i64());
        match self.client.call(&Request::Fluency { window_days }) {
            Ok(Response::Fluency { report }) => tool_text(report.markdown),
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!("fluency failed ({e:#}). Is crossthreadsd running?")),
        }
    }

    fn call_trends(&self, args: &Value) -> Value {
        let days = args.get("days").and_then(|v| v.as_i64());
        match self.client.call(&Request::Trends { days }) {
            Ok(Response::Trends { rows }) => {
                if rows.is_empty() {
                    return tool_text("Not enough history to compute trends yet.".to_string());
                }
                let mut out = String::from("Trends (last 30 days vs previous):\n");
                for r in rows {
                    let fmt = |v: f64| {
                        if r.is_pct {
                            format!("{:.0}%", v * 100.0)
                        } else {
                            format!("{v:.1}")
                        }
                    };
                    let dir = if r.delta.abs() < 1e-9 {
                        "—"
                    } else if r.improved {
                        "better"
                    } else {
                        "worse"
                    };
                    out.push_str(&format!(
                        "\n- {}: {} (was {}) {dir}",
                        r.label,
                        fmt(r.current),
                        fmt(r.previous)
                    ));
                }
                tool_text(out)
            }
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!("trends failed ({e:#}). Is crossthreadsd running?")),
        }
    }

    fn call_metrics(&self, args: &Value) -> Value {
        match self.client.call(&Request::Metrics {
            filters: filters_from(args),
        }) {
            Ok(Response::Metrics { metrics }) => {
                if metrics.sessions == 0 {
                    return tool_text("No sessions indexed yet.".to_string());
                }
                // Reuse the same compact summary the insights are grounded in.
                tool_text(ct_daemon::behavior::summarize(&metrics))
            }
            Ok(other) => tool_error(format!("unexpected response: {other:?}")),
            Err(e) => tool_error(format!("metrics failed ({e:#}). Is crossthreadsd running?")),
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
/// The working directory a tool call is about: the `cwd` argument, else
/// this server's own (agents start their MCP servers in the project).
fn cwd_from(args: &Value) -> Option<String> {
    args.get("cwd")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        })
}

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
