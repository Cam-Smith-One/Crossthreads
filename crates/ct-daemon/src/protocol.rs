//! Wire protocol for the daemon: newline-delimited JSON over a loopback TCP
//! socket. The desktop app, CLI, and MCP server all speak this (ADR-005).
//!
//! One request object per line, one response object per line. Kept deliberately
//! small and dependency-light (no HTTP framework) for the Phase 1 MVP.

use serde::{Deserialize, Serialize};

use ct_store::{
    Filters, SearchHit, StoredConversation, ThreadDetail, ThreadFilters, ThreadSummary,
};

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
    /// This daemon's version, so a client (the desktop app) can tell an
    /// older daemon from its own. Daemons before 0.22.1 don't know this op and
    /// answer with an error, which also means "older". Answers `Version`.
    Version,
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
    /// Grant or withdraw "can message" for an approved peer: whether room
    /// messages are exchanged with it (SPEC FR-ROOM-08). Answers `Devices`.
    SetPeerRooms { name: String, rooms: bool },
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
    /// How to resume a conversation in its native tool (a runnable command where
    /// one exists, else reveal the transcript).
    Resume { id: String },
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
    /// cards, a decision log, or recurring patterns).
    /// Needs a model login (Settings → Models). `kind` is one of `open_loops` |
    /// `knowledge_cards` | `decision_log` | `recurrence`.
    Insight {
        kind: String,
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Answer a natural-language question from the user's own history: retrieve
    /// the most relevant past sessions (across tools/devices) and synthesize a
    /// cited answer. Degrades to retrieved context when no model is configured.
    Ask {
        question: String,
        #[serde(default = "default_ask_limit")]
        limit: usize,
        #[serde(default)]
        filters: Filters,
        #[serde(default)]
        devices: Option<Vec<String>>,
    },
    /// Activity over time, bucketed by day or week, for the temporal view.
    Activity {
        /// `day` (default) or `week`.
        #[serde(default)]
        bucket: Option<String>,
        #[serde(default)]
        filters: Filters,
    },
    /// Deterministic behavioral metrics ("how you work") over the indexed
    /// sessions — turns, corrections, friction, tempo, specificity. No model.
    Metrics {
        #[serde(default)]
        filters: Filters,
    },
    /// The proactive weekly review. Returns the cached one if it's fresh;
    /// `force` regenerates. Generation uses the model when configured.
    WeeklyReview {
        #[serde(default)]
        force: bool,
    },
    /// Behavioral trends: the last 30 days vs the previous 30, per metric.
    Trends {
        #[serde(default)]
        days: Option<i64>,
    },
    /// The "optimize how you work" loop: grade the active prescription when ripe,
    /// then prescribe the current biggest lever. `force` grades immediately.
    Optimize {
        #[serde(default)]
        force: bool,
    },
    /// The 4D AI-fluency view: Delegation / Description / Discernment / Diligence
    /// scored from the behavioral metrics, each with a movement delta, plus a
    /// single reflective question. Deterministic; no model.
    Fluency {
        #[serde(default)]
        window_days: Option<i64>,
    },
    /// The menu-bar glance: per-tool usage (today vs. your baseline, this week)
    /// plus the insight read (fluency score, unresolved threads, optimize focus)
    /// for the desktop tray popover. Deterministic; no model.
    Glance,
    /// List threads: pieces of work, each one or more sessions linked across
    /// tools (SPEC §6.1). Most recently active first.
    ///
    /// With federation configured the list spans devices: `devices` routes it
    /// like `Search::devices` (`None` = this device and every reachable peer),
    /// and each thread says which device it is on.
    Threads {
        #[serde(default)]
        filters: ThreadFilters,
        #[serde(default)]
        devices: Option<Vec<String>>,
    },
    /// One thread with its sessions, links and pending suggestions. `id` is a
    /// thread id or the conversation id of any session in it. `device` fetches
    /// it from the peer it lives on.
    Thread {
        id: String,
        #[serde(default)]
        device: Option<String>,
    },
    /// A paste-ready brief for continuing a thread elsewhere: the goal, every
    /// session, and where the latest one left off. Answers `Context`.
    ThreadHandoff {
        id: String,
        #[serde(default)]
        device: Option<String>,
        #[serde(default = "default_handoff_chars")]
        max_chars: usize,
    },
    /// A thread list forwarded from a peer: token-gated and local-only, like
    /// `PeerSearch`. Threads with a session outside the serve-scope are left out.
    PeerThreads {
        #[serde(default)]
        token: Option<String>,
        #[serde(default)]
        filters: ThreadFilters,
    },
    /// One thread fetched by a peer: token-gated, local-only, serve-scoped.
    PeerThread {
        #[serde(default)]
        token: Option<String>,
        id: String,
    },
    /// Room messages pushed by peer `from` as they are posted there.
    /// Token-gated, and accepted only when this device has granted `from`
    /// "can message". Answers `Ok`.
    PeerRoomPost {
        #[serde(default)]
        token: Option<String>,
        from: String,
        messages: Vec<ct_store::rooms::RoomWire>,
    },
    /// A peer catching up: this device's own room messages after id `after`
    /// (the last 24 hours at most). Same gate as `PeerRoomPost`. Answers
    /// `RoomWire`.
    PeerRoomSync {
        #[serde(default)]
        token: Option<String>,
        from: String,
        #[serde(default)]
        after: i64,
    },
    // ---- local key (see `local_auth`) --------------------------------------
    /// Present this account's local key. A local client sends this first;
    /// without it only `ping` and the peer protocol answer. Answers `Ok`.
    Hello { key: String },
    /// A one-time, 60-second ticket and link to sign the web app in. Answers
    /// `LoginTicket`.
    LoginTicket,
    // ---- phone access (SPEC FR-BRD-04) -------------------------------------
    /// Who is asking: a paired phone (with its name and grants) or this
    /// device. Answers `Session`.
    Session,
    /// What's installed and turned on here, for the "Get started" checklist.
    /// Answers `Setup`.
    SetupStatus,
    /// Phone access settings and paired phones. Answers `Phone`.
    PhoneStatus,
    /// Turn the phone listener on or off. Answers `Phone`.
    PhoneSetEnabled { enabled: bool },
    /// Open a one-time pairing code. Answers `PhonePairing`.
    PhonePair,
    /// Forget a paired phone. Answers `Phone`.
    PhoneRevoke { id: String },
    /// Let a paired phone post to rooms, or not. Answers `Phone`.
    PhoneSetPost { id: String, can_post: bool },
    // ---- rooms (SPEC §6.4) ------------------------------------------------
    /// Post a message to a room and, unless `wait` is false, block until a
    /// reply arrives (up to `timeout` seconds). Answers `Text`.
    RoomSend {
        me: String,
        text: String,
        #[serde(default)]
        room: String,
        #[serde(default)]
        to: Option<String>,
        #[serde(default = "default_true")]
        wait: bool,
        #[serde(default = "default_room_wait")]
        timeout: u64,
    },
    /// Return an agent's unread messages, blocking up to `timeout` seconds.
    RoomWait {
        me: String,
        #[serde(default)]
        room: String,
        #[serde(default = "default_room_wait")]
        timeout: u64,
    },
    /// Who is in a room, turns used, claims, and the last `limit` messages.
    RoomHistory {
        #[serde(default)]
        me: Option<String>,
        #[serde(default)]
        room: String,
        #[serde(default = "default_room_history")]
        limit: usize,
    },
    /// Reserve files or folders (absolute paths) for `minutes`.
    RoomClaim {
        me: String,
        paths: Vec<String>,
        #[serde(default)]
        room: String,
        #[serde(default = "default_claim_minutes")]
        minutes: i64,
    },
    /// Release an agent's claims on `paths`, or all of them.
    RoomRelease {
        me: String,
        #[serde(default)]
        paths: Option<Vec<String>>,
    },
    /// A message from the human, typed into the board.
    RoomPost {
        room: String,
        text: String,
        #[serde(default)]
        to: Option<String>,
        #[serde(default)]
        author: Option<String>,
    },
    /// Every room with its agents, turns and claims, for the board.
    Rooms,
    /// Messages in a room after id `after`, oldest first, for the board.
    RoomMessages {
        room: String,
        #[serde(default)]
        after: i64,
        #[serde(default = "default_room_history")]
        limit: usize,
    },
    /// From the Stop hook: if this agent is mid-exchange, hold it up to `wait`
    /// seconds for the next message and hand it back.
    RoomStop {
        me: String,
        #[serde(default)]
        room: String,
        #[serde(default)]
        wait: u64,
        #[serde(default)]
        transcript_path: Option<String>,
    },
    /// From the PreToolUse hook: the live claims covering any of `paths`.
    RoomGuard { paths: Vec<String> },
    /// A session event from a tool's hook (`crossthreads hook …`), used to
    /// track what each session is doing right now (SPEC §6.2).
    HookEvent {
        #[serde(flatten)]
        event: crate::live::HookEvent,
    },
    /// Put two sessions (by conversation id) in one thread. Also how a
    /// suggested link is accepted. `kind` defaults to `manual`.
    LinkSessions {
        a: String,
        b: String,
        #[serde(default)]
        kind: Option<String>,
    },
    /// Take a session out of its thread, back to a thread of one.
    UnlinkSession { id: String },
    /// Dismiss a suggested link between two sessions, for good.
    RejectLink { a: String, b: String },
    /// Rename a thread and/or set its status (`active` | `parked` | `done`).
    /// An empty title restores the derived one.
    SetThread {
        id: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        status: Option<String>,
    },
}

impl Request {
    /// Requests one daemon sends another. These check the federation token
    /// themselves, so they are the only ones served to other machines.
    pub fn is_peer_request(&self) -> bool {
        matches!(
            self,
            Request::Ping
                | Request::PeerSearch { .. }
                | Request::PeerGetConversation { .. }
                | Request::PeerThreads { .. }
                | Request::PeerThread { .. }
                | Request::PeerRoomPost { .. }
                | Request::PeerRoomSync { .. }
        )
    }
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
    /// Room messages are exchanged with this peer ("can message").
    #[serde(default)]
    pub rooms: bool,
}

/// A candidate device found on the tailnet during discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredDevice {
    pub name: String,
    pub addr: String,
    /// Already in the approved-peers list (so the UI can show it as added).
    pub already_approved: bool,
}

fn default_handoff_chars() -> usize {
    8000
}
fn default_true() -> bool {
    true
}
fn default_room_wait() -> u64 {
    crate::rooms::DEFAULT_WAIT
}
fn default_room_history() -> usize {
    30
}
fn default_claim_minutes() -> i64 {
    20
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
fn default_ask_limit() -> usize {
    12
}
fn default_max_chars() -> usize {
    6000
}

/// A response from the daemon to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// The daemon's version (see `Request::Version`).
    Version {
        version: String,
    },
    Pong,
    Status {
        conversations: i64,
        embeddings: i64,
        embedder: String,
        /// Detected tools with their indexed counts: `(tool, threads, skills)`,
        /// busiest first — for the "what's indexed" chips in the UI.
        #[serde(default)]
        tools: Vec<(String, i64, i64)>,
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
    /// How to resume a conversation, or `None` if the id is unknown.
    Resume {
        resume: Option<crate::resume::Resume>,
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
    /// An answer to an `Ask`: synthesized (or retrieved) Markdown plus the source
    /// conversation ids, and whether a model was used (vs. retrieval-only).
    Answer {
        markdown: String,
        sources: Vec<String>,
        llm_used: bool,
    },
    /// Activity counts over time for the temporal view.
    Activity {
        bucket: String,
        periods: Vec<ct_store::ActivityBucket>,
    },
    /// Behavioral metrics ("how you work").
    Metrics {
        metrics: ct_store::WorkMetrics,
    },
    /// The weekly review (Markdown + provenance). `fresh` is true when this call
    /// generated it (vs. returned a cached one).
    WeeklyReview {
        markdown: String,
        generated_at: String,
        period_start: String,
        llm_used: bool,
        fresh: bool,
    },
    /// Behavioral trends, one row per metric.
    Trends {
        rows: Vec<crate::optimize::TrendRow>,
    },
    /// The optimize report (Markdown) + whether a focus is active.
    Optimize {
        markdown: String,
        has_active: bool,
    },
    /// The 4D AI-fluency report: scored dimensions, deltas, and a reflection.
    Fluency {
        report: crate::fluency::FluencyReport,
    },
    /// The menu-bar glance bundle (usage + insight).
    Glance {
        glance: crate::glance::Glance,
    },
    /// Threads, most recently active first. `unreachable` names selected
    /// peers that didn't answer, as for `Hits`.
    Threads {
        threads: Vec<ThreadSummary>,
        #[serde(default)]
        unreachable: Vec<String>,
    },
    /// One thread in full, or `None` if the id is unknown.
    Thread {
        thread: Option<ThreadDetail>,
    },
    /// The thread a link or edit landed in, or `None` if nothing matched.
    ThreadId {
        id: Option<String>,
    },
    /// A plain-text result for an agent (room tools).
    Text {
        text: String,
    },
    /// Every room, for the board.
    Rooms {
        rooms: Vec<crate::rooms::RoomView>,
    },
    RoomMessages {
        messages: Vec<ct_store::RoomMessage>,
    },
    /// Who is asking (see `Request::Session`).
    Session {
        /// True when the request came from a paired phone.
        phone: bool,
        #[serde(default)]
        name: Option<String>,
        /// The phone may post to rooms.
        #[serde(default)]
        can_post: bool,
    },
    /// A sign-in ticket for the local web app, and the link that uses it.
    LoginTicket {
        ticket: String,
        url: String,
    },
    /// What's installed and turned on (see `Request::SetupStatus`).
    Setup {
        setup: crate::setup::SetupStatus,
    },
    /// Phone access state.
    Phone {
        enabled: bool,
        /// The listener is running.
        listening: bool,
        /// Where a phone opens the board, when listening.
        #[serde(default)]
        url: Option<String>,
        /// Why it isn't listening, when it should be.
        #[serde(default)]
        error: Option<String>,
        phones: Vec<crate::phone::PhoneInfo>,
    },
    /// A fresh pairing code and the link (and QR code) that carries it.
    PhonePairing {
        code: String,
        url: String,
        /// The link as a QR code, SVG.
        #[serde(default)]
        qr_svg: Option<String>,
        expires_secs: u64,
    },
    /// Room messages for a peer catching up.
    RoomWire {
        messages: Vec<ct_store::rooms::RoomWire>,
    },
    /// The id of a message the human posted.
    RoomPosted {
        id: i64,
    },
    /// Messages for an agent that was about to stop, or `None` to let it stop.
    RoomStop {
        /// The room's display name.
        room: Option<String>,
        messages: Option<String>,
    },
    /// Claims that cover the paths an agent is about to edit.
    RoomGuard {
        claims: Vec<ct_store::Claim>,
    },
    /// Generic acknowledgement for mutating ops (set flags, open source).
    Ok {
        ok: bool,
    },
    Error {
        message: String,
    },
}
