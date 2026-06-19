//! `ct-mcp` — Crossthreads MCP server (FR-UI-04).
//!
//! Reads JSON-RPC messages (one per line) from stdin and writes responses to
//! stdout, forwarding tool calls to a running `crossthreadsd`. Configure it as
//! an MCP server in your agent; point it at the daemon with `CROSSTHREADS_ADDR`
//! if not the default.
//!
//! See `docs/AGENT_API.md` §7.

use std::io::{BufRead, Write};
use std::process::ExitCode;

use ct_daemon::Client;
use ct_mcp::Server;
use serde_json::Value;

fn main() -> ExitCode {
    let server = Server::new(Client::from_env());

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("ct-mcp: stdin error: {e}");
                return ExitCode::FAILURE;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                // Malformed input: emit a JSON-RPC parse error with null id.
                let err = serde_json::json!({
                    "jsonrpc": "2.0", "id": null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") }
                });
                let _ = writeln!(out, "{err}");
                let _ = out.flush();
                continue;
            }
        };

        if let Some(response) = server.handle(&msg) {
            if writeln!(out, "{response}")
                .and_then(|_| out.flush())
                .is_err()
            {
                break; // stdout closed
            }
        }
    }

    ExitCode::SUCCESS
}
