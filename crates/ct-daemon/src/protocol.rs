//! Wire protocol for the daemon: newline-delimited JSON over a loopback TCP
//! socket. The desktop app, CLI, and MCP server all speak this (ADR-005).
//!
//! One request object per line, one response object per line. Kept deliberately
//! small and dependency-light (no HTTP framework) for the Phase 1 MVP.

use serde::{Deserialize, Serialize};

use ct_store::SearchHit;

/// Default loopback address. Override with `CROSSTHREADS_ADDR`.
pub const DEFAULT_ADDR: &str = "127.0.0.1:47100";

/// Search ranking mode, mirrored from the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Lexical,
    Semantic,
    Hybrid,
}

/// A request from a client to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Liveness check.
    Ping,
    /// Index health and counts.
    Status,
    /// Force a re-index pass.
    Reindex,
    /// Search the index.
    Search {
        query: String,
        #[serde(default = "default_mode")]
        mode: Mode,
        #[serde(default = "default_limit")]
        limit: usize,
    },
}

fn default_mode() -> Mode {
    Mode::Hybrid
}
fn default_limit() -> usize {
    10
}

/// A response from the daemon to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong,
    Status {
        conversations: i64,
        embeddings: i64,
        embedder: String,
    },
    Reindexed {
        inserted: usize,
        duplicate: usize,
        embedded: usize,
    },
    Hits {
        hits: Vec<SearchHit>,
    },
    Error {
        message: String,
    },
}
