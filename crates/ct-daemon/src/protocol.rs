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
    /// Search forwarded from a peer daemon during cross-device federation
    /// (ADR-010). Unlike `Search`, it runs **local-only** (never re-fans-out, so
    /// queries can't loop/amplify) and is gated by the shared `token`. The
    /// querying device merges the returned hits into its own results.
    PeerSearch {
        /// Shared federation secret; required if the receiving daemon configured
        /// one. Carried per-request so it never has to be logged.
        #[serde(default)]
        token: Option<String>,
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
    /// Set/clear the bookmark and/or pin flag on a conversation.
    SetFlags {
        id: String,
        #[serde(default)]
        bookmarked: Option<bool>,
        #[serde(default)]
        pinned: Option<bool>,
    },
    /// List saved (bookmarked or pinned) conversations.
    Saved,
    /// Open a conversation's source file in the OS default app / reveal it.
    OpenSource { id: String },
    /// Forget a conversation: delete it from the index and tombstone it so it is
    /// not re-indexed.
    Forget { id: String },
    /// Attach a free-text note to a conversation.
    SetNote { id: String, note: String },
    /// Set the tags on a conversation.
    SetTags { id: String, tags: Vec<String> },
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
        #[serde(default)]
        tags: Vec<String>,
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
    /// Generic acknowledgement for mutating ops (set flags, open source).
    Ok {
        ok: bool,
    },
    Error {
        message: String,
    },
}
