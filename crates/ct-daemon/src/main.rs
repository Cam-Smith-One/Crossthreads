//! `crossthreadsd` — the Crossthreads background daemon (ADR-005).
//!
//! The single writer to the index: owns connectors, file-watchers, the ONNX
//! runtime, and the query engine, and serves desktop/CLI/MCP clients over a
//! loopback API.
//!
//! Not yet implemented — scaffold only. Phase 0 exercises the pipeline through
//! the `crossthreads` CLI; the daemon, storage (SQLite + sqlite-vec), and the
//! loopback API land in Phase 1.

fn main() {
    eprintln!("crossthreadsd: not implemented yet (scaffold). See docs/ROADMAP.md.");
}
