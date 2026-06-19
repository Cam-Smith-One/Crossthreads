//! Wire protocol for the daemon: newline-delimited JSON over a loopback TCP
//! socket. The desktop app, CLI, and MCP server all speak this (ADR-005).
//!
//! One request object per line, one response object per line. Kept deliberately
//! small and dependency-light (no HTTP framework) for the Phase 1 MVP.

use serde::{Deserialize, Serialize};

use ct_store::{Filters, SearchHit, StoredConversation};

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
    /// Distinct tools present (for filter UIs).
    Facets,
    /// Force a re-index pass.
    Reindex,
    /// Search the index.
    Search {
        query: String,
        #[serde(default = "default_mode")]
        mode: Mode,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        filters: Filters,
    },
    /// Fetch a full conversation by id.
    GetConversation { id: String },
    /// Build a paste-ready context block from the top matches for a query.
    Context {
        query: String,
        #[serde(default = "default_mode")]
        mode: Mode,
        #[serde(default = "default_context_limit")]
        limit: usize,
        #[serde(default = "default_max_chars")]
        max_chars: usize,
        #[serde(default)]
        filters: Filters,
    },
}

fn default_mode() -> Mode {
    Mode::Hybrid
}
fn default_limit() -> usize {
    10
}
fn default_context_limit() -> usize {
    3
}
fn default_max_chars() -> usize {
    6000
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
    Facets {
        tools: Vec<String>,
    },
    Reindexed {
        inserted: usize,
        duplicate: usize,
        embedded: usize,
    },
    Hits {
        hits: Vec<SearchHit>,
    },
    Conversation {
        conversation: Option<StoredConversation>,
    },
    Context {
        markdown: String,
        sources: Vec<String>,
        token_estimate: usize,
    },
    Error {
        message: String,
    },
}
