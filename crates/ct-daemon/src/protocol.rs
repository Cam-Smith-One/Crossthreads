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
        /// Cross-device routing (ADR-010): names of the devices to search.
        /// `None` (or absent) ⇒ this device + all reachable peers. When set,
        /// only the named devices are queried (this host included if named).
        #[serde(default)]
        devices: Option<Vec<String>>,
    },
    /// List the devices this daemon knows about — this host plus approved peers
    /// — with current liveness, for the device picker and Settings → Devices.
    Devices,
    /// One-shot scan of the tailnet for other reachable Crossthreads daemons
    /// (the "Discover my devices" button). Never runs in the background.
    DiscoverDevices,
    /// Approve a peer (typically one returned by `DiscoverDevices`) and persist
    /// it to the federation config so it survives restarts.
    ApprovePeer { name: String, addr: String },
    /// Remove an approved peer and persist the change.
    RemovePeer { name: String },
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
    /// Fetch a full conversation by id. `device` routes the fetch to a peer
    /// when the conversation lives on another machine (ADR-010).
    GetConversation {
        id: String,
        #[serde(default)]
        device: Option<String>,
    },
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
        /// Cross-device routing, same semantics as `Search::devices` (ADR-010).
        #[serde(default)]
        devices: Option<Vec<String>>,
    },
    /// Fetch a full conversation from a peer during cross-device context
    /// assembly: token-gated and local-only, like `PeerSearch`.
    PeerGetConversation {
        #[serde(default)]
        token: Option<String>,
        id: String,
    },
    /// Read this device's federation identity + serve-scope, for the Settings →
    /// Devices panel (ADR-010). Includes a copy-paste pairing code.
    GetFederationConfig,
    /// Update this device's federation identity / serve-scope. Each field is
    /// optional (omitted = leave unchanged). For `token`: an empty string clears
    /// it; any other value sets it; omitting leaves it.
    SetFederationConfig {
        #[serde(default)]
        device: Option<String>,
        #[serde(default)]
        token: Option<String>,
        #[serde(default)]
        exclude_tools: Option<Vec<String>>,
        #[serde(default)]
        exclude_projects: Option<Vec<String>>,
    },
    /// Onboard a second device in one paste: decode a pairing code (from another
    /// device's `GetFederationConfig`), adopt its token if we have none, and
    /// approve the embedded peer.
    PairWithCode { code: String },
    /// Read LLM provider auth status for the Settings → Models panel (secret-free).
    GetLlmConfig,
    /// Store (non-empty) or clear (empty) a provider's bring-your-own API key in
    /// the OS keychain. `provider` is `anthropic` | `openai`.
    SetLlmKey { provider: String, key: String },
    /// Pin the active provider (`anthropic` | `openai`); empty clears the pin.
    SetLlmProvider { provider: String },
    /// Cluster the indexed conversations into themes (for the UI map and MCP).
    Themes {
        #[serde(default)]
        k: Option<usize>,
        /// Label each cluster with a short LLM-generated name (needs a model
        /// login; falls back to the TF·IDF label when unavailable).
        #[serde(default)]
        name: bool,
    },
    /// Synthesize an LLM insight over recent history (open loops, knowledge
    /// cards, a decision log, a "how I work" profile, or a digest). Needs a
    /// model login (Settings → Models). `kind` is one of `open_loops` |
    /// `knowledge_cards` | `decision_log` | `how_i_work` | `digest`.
    Insight {
        kind: String,
        #[serde(default)]
        limit: Option<usize>,
    },
}

/// A device known to this daemon: this host, or an approved peer (ADR-010).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Display name (this host's `device` name, or the peer's configured name).
    pub name: String,
    /// Peer address (`host:port`); `None` for this device.
    pub addr: Option<String>,
    /// Reachable right now — liveness is pinged at request time, never polled.
    pub online: bool,
    /// True for this machine.
    pub local: bool,
}

/// A candidate device found on the tailnet during discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredDevice {
    pub name: String,
    pub addr: String,
    /// Already in the approved-peers list (so the UI can show it as added).
    pub already_approved: bool,
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
        /// Names of selected peers that didn't answer (offline/timeout/rejected),
        /// so the UI can show "N devices unreachable" (ADR-010).
        #[serde(default)]
        unreachable: Vec<String>,
    },
    /// Known devices (this host + approved peers) with liveness. `federation`
    /// is false when cross-device search isn't configured yet, so the UI can
    /// prompt the user to set a device name + token.
    Devices {
        devices: Vec<DeviceInfo>,
        federation: bool,
    },
    /// Candidate devices found on the tailnet by `DiscoverDevices`.
    DiscoveredDevices {
        devices: Vec<DiscoveredDevice>,
    },
    /// This device's federation identity + serve-scope (ADR-010).
    FederationConfig {
        device: String,
        /// A shared token is set (never returned in the clear).
        has_token: bool,
        /// The token is stored in the OS keychain (vs. the plaintext config file).
        token_in_keyring: bool,
        /// This daemon's peer-reachable address, if known (for the pairing code).
        listen_addr: Option<String>,
        exclude_tools: Vec<String>,
        exclude_projects: Vec<String>,
        /// Copy-paste code another device can `PairWithCode` to join. `None` until
        /// both a token and a reachable address are known.
        pairing_code: Option<String>,
    },
    Conversation {
        conversation: Option<StoredConversation>,
    },
    Context {
        markdown: String,
        sources: Vec<String>,
        token_estimate: usize,
    },
    /// LLM provider auth status for Settings → Models (secret-free).
    LlmConfig {
        providers: Vec<ct_llm::ProviderStatus>,
        /// The pinned active provider id, if any (`anthropic` | `openai`).
        active: Option<String>,
    },
    /// Conversation themes (clusters) for the UI map and MCP.
    Themes {
        themes: Vec<ct_store::Theme>,
    },
    /// A synthesized insight as Markdown, plus the source conversation ids it
    /// drew from.
    Insight {
        markdown: String,
        sources: Vec<String>,
    },
    /// Generic acknowledgement for mutating ops (set flags, open source).
    Ok {
        ok: bool,
    },
    Error {
        message: String,
    },
}
