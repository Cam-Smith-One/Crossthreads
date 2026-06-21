//! `ct_daemon` — the Crossthreads background daemon (ADR-005).
//!
//! Owns the single [`Store`] writer, the embedder, file-watchers, and a small
//! loopback request server. The desktop app, CLI, and MCP server connect as
//! thin [`Client`]s over the [`protocol`].

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use ct_embed::Embedder;
use ct_store::Store;

pub mod federation;
mod http;
pub mod protocol;
pub use ct_store::{Filters, SearchHit, StoredConversation};
pub use federation::{Federation, Peer};
pub use protocol::{DeviceInfo, Mode, Request, Response, DEFAULT_ADDR};

/// How long to wait for the filesystem to settle before re-indexing.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(750);

/// Default port a peer daemon listens on, used when discovering tailnet devices.
const FED_PORT: u16 = 47100;

/// The running daemon: shared handles to the store and embedder.
#[derive(Clone)]
pub struct Daemon {
    store: Arc<Mutex<Store>>,
    embedder: Arc<dyn Embedder>,
    /// Cross-device federation config (ADR-010); `None` = local-only daemon.
    federation: Option<Arc<Federation>>,
}

impl Daemon {
    /// Build a daemon around an open store and embedder.
    pub fn new(store: Store, embedder: Box<dyn Embedder>) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            embedder: Arc::from(embedder),
            federation: None,
        }
    }

    /// Enable cross-device federation: stamp served hits with our device name,
    /// fan local searches out to `peers`, and answer authenticated `PeerSearch`.
    pub fn with_federation(mut self, fed: Federation) -> Self {
        self.federation = Some(Arc::new(fed));
        self
    }

    /// Run one indexing pass. The store lock is taken only briefly (per upsert
    /// batch and per embed batch) and released during the slow embedding step,
    /// so search stays responsive even during a large initial index.
    pub fn reindex(&self) -> Result<ct_index::IndexReport> {
        let (conversations, unparseable) = ct_index::collect(None);
        let mut report = ct_index::IndexReport {
            parsed: conversations.len(),
            unparseable,
            ..Default::default()
        };

        // Persist (fast) under a single short-lived lock.
        {
            let mut store = self.store.lock().expect("store mutex poisoned");
            for convo in &conversations {
                match store.upsert_conversation(convo)? {
                    ct_store::Upsert::Inserted => report.inserted += 1,
                    ct_store::Upsert::Duplicate => report.duplicate += 1,
                    ct_store::Upsert::Forgotten => {} // tombstoned, skip
                }
            }
        }

        // Embed in small batches, holding the lock only to read pending rows and
        // to write vectors back — never during the (slow) embedding compute.
        loop {
            let batch = {
                let store = self.store.lock().expect("store mutex poisoned");
                store.pending_embeddings(128)?
            };
            if batch.is_empty() {
                break;
            }
            let texts: Vec<String> = batch.iter().map(|(_, c)| c.clone()).collect();
            let vecs = self.embedder.embed(&texts)?;
            let rows: Vec<(i64, Vec<f32>)> =
                batch.iter().map(|(rowid, _)| *rowid).zip(vecs).collect();
            {
                let mut store = self.store.lock().expect("store mutex poisoned");
                store.store_embeddings(self.embedder.id(), &rows)?;
            }
            report.embedded += rows.len();
        }

        Ok(report)
    }

    /// Per-(tool, kind) record counts, for a coverage summary.
    pub fn counts_by_tool(&self) -> Result<Vec<(String, String, i64)>> {
        let store = self.store.lock().expect("store mutex poisoned");
        store.counts_by_tool()
    }

    /// Spawn the file-watcher: re-index (debounced) whenever a connector root
    /// changes (FR-ING-05). Returns a handle; the watcher lives for the
    /// thread's lifetime. Watching nothing (no connectors present) is a no-op.
    pub fn spawn_watcher(&self) -> Result<thread::JoinHandle<()>> {
        let roots: Vec<PathBuf> = ct_index::watch_roots();
        let this = self.clone();
        let handle = thread::Builder::new()
            .name("ct-watcher".into())
            .spawn(move || {
                if let Err(e) = this.watch_loop(roots) {
                    eprintln!("watcher stopped: {e:#}");
                }
            })?;
        Ok(handle)
    }

    fn watch_loop(&self, roots: Vec<PathBuf>) -> Result<()> {
        use notify::{RecursiveMode, Watcher};

        if roots.is_empty() {
            return Ok(()); // nothing to watch
        }

        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })?;
        for root in &roots {
            // Best-effort: a missing/locked root shouldn't kill the watcher.
            if let Err(e) = watcher.watch(root, RecursiveMode::Recursive) {
                eprintln!("warn: cannot watch {}: {e}", root.display());
            }
        }

        // Debounce: block for an event, then drain the burst before re-indexing.
        loop {
            match rx.recv() {
                Ok(_) => {
                    while rx.recv_timeout(WATCH_DEBOUNCE).is_ok() {}
                    match self.reindex() {
                        Ok(r) if r.inserted > 0 || r.embedded > 0 => {
                            eprintln!("reindex: {} new, {} embedded", r.inserted, r.embedded);
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("reindex failed: {e:#}"),
                    }
                }
                Err(_) => return Ok(()), // all senders dropped
            }
        }
    }

    /// Serve requests on an already-bound listener until it closes. Each
    /// connection is handled on its own thread; the store mutex serializes
    /// access (single-writer invariant holds).
    pub fn serve(&self, listener: TcpListener) -> Result<()> {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    // Bound half-open/idle connections so they can't pin a thread
                    // forever (clients send a request immediately, then we reply).
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
                    let this = self.clone();
                    thread::spawn(move || {
                        if let Err(e) = this.handle_conn(stream) {
                            eprintln!("connection error: {e:#}");
                        }
                    });
                }
                Err(e) => eprintln!("accept error: {e}"),
            }
        }
        Ok(())
    }

    fn handle_conn(&self, stream: TcpStream) -> Result<()> {
        let peer = stream.peer_addr().ok();
        let reader = BufReader::new(stream.try_clone()?);
        let mut writer = stream;
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Request>(&line) {
                Ok(req) => self.handle(req),
                Err(e) => Response::Error {
                    message: format!("bad request: {e}"),
                },
            };
            let mut bytes = serde_json::to_vec(&response)?;
            bytes.push(b'\n');
            writer.write_all(&bytes)?;
            writer.flush()?;
        }
        let _ = peer;
        Ok(())
    }

    /// Dispatch one request to a response.
    pub fn handle(&self, req: Request) -> Response {
        match req {
            Request::Ping => Response::Pong,
            Request::Status => self.status().unwrap_or_else(err),
            Request::Facets => self.facets().unwrap_or_else(err),
            Request::Reindex => self
                .reindex()
                .map(|r| Response::Reindexed {
                    inserted: r.inserted,
                    duplicate: r.duplicate,
                    embedded: r.embedded,
                })
                .unwrap_or_else(err),
            Request::Search {
                query,
                mode,
                limit,
                filters,
                devices,
            } => self
                .search(&query, mode, limit, &filters, devices.as_deref())
                .unwrap_or_else(err),
            Request::Devices => self.devices_list(),
            Request::DiscoverDevices => self.discover_devices(),
            Request::ApprovePeer { name, addr } => self.approve_peer(name, addr),
            Request::RemovePeer { name } => self.remove_peer(&name),
            Request::PeerSearch {
                token,
                query,
                mode,
                limit,
                filters,
            } => self.peer_search(token.as_deref(), &query, mode, limit, &filters),
            Request::PeerGetConversation { token, id } => {
                self.peer_get_conversation(token.as_deref(), &id)
            }
            Request::GetConversation { id, device } => self
                .get_conversation(&id, device.as_deref())
                .unwrap_or_else(err),
            Request::GetFederationConfig => self.federation_config(),
            Request::SetFederationConfig {
                device,
                token,
                exclude_tools,
                exclude_projects,
            } => self.set_federation_config(device, token, exclude_tools, exclude_projects),
            Request::PairWithCode { code } => self.pair_with_code(&code),
            Request::SetFlags {
                id,
                bookmarked,
                pinned,
            } => self.set_flags(&id, bookmarked, pinned).unwrap_or_else(err),
            Request::Saved => self.saved().unwrap_or_else(err),
            Request::OpenSource { id } => self.open_source(&id).unwrap_or_else(err),
            Request::Forget { id } => self.forget(&id).unwrap_or_else(err),
            Request::SetNote { id, note } => self.set_note(&id, &note).unwrap_or_else(err),
            Request::SetTags { id, tags } => self.set_tags(&id, &tags).unwrap_or_else(err),
            Request::Context {
                query,
                mode,
                limit,
                max_chars,
                filters,
                devices,
            } => self
                .context(&query, mode, limit, max_chars, &filters, devices.as_deref())
                .unwrap_or_else(err),
        }
    }

    fn facets(&self) -> Result<Response> {
        let store = self.store.lock().expect("store mutex poisoned");
        Ok(Response::Facets {
            tools: store.facets_tools()?,
            tags: store.facets_tags()?,
        })
    }

    fn status(&self) -> Result<Response> {
        let store = self.store.lock().expect("store mutex poisoned");
        Ok(Response::Status {
            conversations: store.conversation_count()?,
            embeddings: store.embedding_count()?,
            embedder: self.embedder.id().to_string(),
        })
    }

    /// Run a search against the local index only. Embedding happens outside the
    /// store lock; we then lock just for the query.
    fn local_search(
        &self,
        query: &str,
        mode: Mode,
        limit: usize,
        f: &Filters,
    ) -> Result<Vec<SearchHit>> {
        Ok(match mode {
            Mode::Lexical => {
                let store = self.store.lock().expect("store mutex poisoned");
                store.search_filtered(query, limit, f)?
            }
            Mode::Semantic => {
                let q = self.embedder.embed_one(query)?;
                let store = self.store.lock().expect("store mutex poisoned");
                store.search_semantic_filtered(&q, limit, f)?
            }
            Mode::Hybrid => {
                let q = self.embedder.embed_one(query)?;
                let store = self.store.lock().expect("store mutex poisoned");
                store.search_hybrid_filtered(query, &q, limit, f)?
            }
        })
    }

    /// Top-level search response. Wraps [`Self::federated_hits`].
    fn search(
        &self,
        query: &str,
        mode: Mode,
        limit: usize,
        f: &Filters,
        devices: Option<&[String]>,
    ) -> Result<Response> {
        let (hits, unreachable) = self.federated_hits(query, mode, limit, f, devices)?;
        Ok(Response::Hits { hits, unreachable })
    }

    /// Local-only unless federation is configured, in which case it fans out to
    /// peer daemons in parallel and merges the results (ADR-010). An offline/
    /// slow/unauthorized peer is skipped, never fatal — its name is returned in
    /// the second tuple element so the UI can surface "N devices unreachable".
    ///
    /// `devices` selects which devices to search (routing, not filtering): `None`
    /// = this host + all peers; otherwise only the named devices are queried.
    fn federated_hits(
        &self,
        query: &str,
        mode: Mode,
        limit: usize,
        f: &Filters,
        devices: Option<&[String]>,
    ) -> Result<(Vec<SearchHit>, Vec<String>)> {
        let Some(fed) = &self.federation else {
            // No federation: device selection is moot, return local results.
            return Ok((self.local_search(query, mode, limit, f)?, Vec::new()));
        };

        // Which peers to query, and whether to include local results.
        let device = fed.device();
        let want = |name: &str| devices.map_or(true, |d| d.iter().any(|n| n == name));
        let peers: Vec<Peer> = fed.peers().into_iter().filter(|p| want(&p.name)).collect();

        let mut lists: Vec<Vec<SearchHit>> = Vec::new();
        if want(&device) {
            let mut local = self.local_search(query, mode, limit, f)?;
            for h in &mut local {
                h.device = Some(device.clone());
            }
            lists.push(local);
        }
        let mut unreachable = Vec::new();
        if !peers.is_empty() {
            let (peer_lists, un) =
                self.fan_out(fed.token(), fed.timeout, peers, query, mode, limit, f);
            lists.extend(peer_lists);
            unreachable = un;
        }
        Ok((federation::rrf_merge(lists, limit), unreachable))
    }

    /// Query the given peers in parallel; return the hit lists that came back in
    /// time, plus the names of peers that didn't (offline/timeout/rejected) so
    /// the caller can surface them. Each peer's hits are tagged with its name.
    #[allow(clippy::too_many_arguments)]
    fn fan_out(
        &self,
        token: Option<String>,
        timeout: std::time::Duration,
        peers: Vec<Peer>,
        query: &str,
        mode: Mode,
        limit: usize,
        f: &Filters,
    ) -> (Vec<Vec<SearchHit>>, Vec<String>) {
        let handles: Vec<_> = peers
            .into_iter()
            .map(|peer| {
                let req = Request::PeerSearch {
                    token: token.clone(),
                    query: query.to_string(),
                    mode,
                    limit,
                    filters: f.clone(),
                };
                // Ok(hits) on success; Err(name) when the peer didn't contribute.
                thread::spawn(move || {
                    let client = Client::new(peer.addr.clone());
                    match client.call_timeout(&req, timeout) {
                        Ok(Response::Hits { mut hits, .. }) => {
                            for h in &mut hits {
                                h.device = Some(peer.name.clone());
                            }
                            Ok(hits)
                        }
                        Ok(Response::Error { message }) => {
                            eprintln!("warn: peer {} rejected search: {message}", peer.name);
                            Err(peer.name)
                        }
                        Ok(_) => Err(peer.name),
                        Err(e) => {
                            eprintln!("warn: peer {} unreachable: {e:#}", peer.name);
                            Err(peer.name)
                        }
                    }
                })
            })
            .collect();

        let mut lists = Vec::new();
        let mut unreachable = Vec::new();
        for h in handles {
            match h.join() {
                Ok(Ok(hits)) => lists.push(hits),
                Ok(Err(name)) => unreachable.push(name),
                Err(_) => {} // worker panicked; treat as no contribution
            }
        }
        (lists, unreachable)
    }

    /// Answer a search forwarded from a peer: authenticate, run **local-only**
    /// (no re-fan-out, so queries can't loop), and stamp our device name.
    fn peer_search(
        &self,
        token: Option<&str>,
        query: &str,
        mode: Mode,
        limit: usize,
        f: &Filters,
    ) -> Response {
        let Some(fed) = &self.federation else {
            return Response::Error {
                message: "federation not enabled on this device".into(),
            };
        };
        // A configured token must match (compared in constant time).
        if let Some(expected) = fed.token() {
            if !token.is_some_and(|t| token_eq(t, &expected)) {
                return Response::Error {
                    message: "unauthorized peer (bad or missing token)".into(),
                };
            }
        }
        match self.local_search(query, mode, limit, f) {
            Ok(mut hits) => {
                // Serve-scope: don't expose excluded tools/projects to peers.
                hits.retain(|h| fed.serves(&h.tool, h.project.as_deref()));
                let device = fed.device();
                for h in &mut hits {
                    h.device = Some(device.clone());
                }
                Response::Hits {
                    hits,
                    unreachable: Vec::new(),
                }
            }
            Err(e) => err(e),
        }
    }

    /// List this host + approved peers, pinging peers for current liveness
    /// (ADR-010). Powers the device picker and Settings → Devices.
    fn devices_list(&self) -> Response {
        let Some(fed) = &self.federation else {
            return Response::Devices {
                devices: vec![DeviceInfo {
                    name: "this device".into(),
                    addr: None,
                    online: true,
                    local: true,
                }],
                federation: false,
            };
        };
        let mut devices = vec![DeviceInfo {
            name: fed.device(),
            addr: None,
            online: true,
            local: true,
        }];
        // Probe peers in parallel so one slow peer doesn't stall the list.
        let timeout = fed.timeout;
        let handles: Vec<_> = fed
            .peers()
            .into_iter()
            .map(|p| {
                thread::spawn(move || DeviceInfo {
                    online: federation::probe(&p.addr, timeout),
                    name: p.name,
                    addr: Some(p.addr),
                    local: false,
                })
            })
            .collect();
        for h in handles {
            if let Ok(d) = h.join() {
                devices.push(d);
            }
        }
        Response::Devices {
            devices,
            federation: true,
        }
    }

    /// One-shot tailnet scan for other reachable Crossthreads daemons.
    fn discover_devices(&self) -> Response {
        let (approved, timeout) = match &self.federation {
            Some(fed) => (fed.peers(), fed.timeout),
            None => (Vec::new(), std::time::Duration::from_millis(1500)),
        };
        Response::DiscoveredDevices {
            devices: federation::discover(&approved, FED_PORT, timeout),
        }
    }

    /// Approve (persist) a peer, then return the refreshed device list.
    fn approve_peer(&self, name: String, addr: String) -> Response {
        let Some(fed) = &self.federation else {
            return Response::Error {
                message: "enable cross-device search first (set a device name and shared token)"
                    .into(),
            };
        };
        fed.add_peer(Peer { name, addr });
        self.devices_list()
    }

    /// Remove (persist) a peer, then return the refreshed device list.
    fn remove_peer(&self, name: &str) -> Response {
        match &self.federation {
            Some(fed) => {
                fed.remove_peer(name);
                self.devices_list()
            }
            None => Response::Ok { ok: false },
        }
    }

    /// Fetch a conversation for a local client, routing to the owning peer when
    /// `device` names another machine (ADR-010) — so the UI can open a remote
    /// search hit's transcript.
    fn get_conversation(&self, id: &str, device: Option<&str>) -> Result<Response> {
        Ok(Response::Conversation {
            conversation: self.fetch_conversation(id, device),
        })
    }

    fn set_flags(
        &self,
        id: &str,
        bookmarked: Option<bool>,
        pinned: Option<bool>,
    ) -> Result<Response> {
        let store = self.store.lock().expect("store mutex poisoned");
        let ok = store.set_flags(id, bookmarked, pinned)?;
        Ok(Response::Ok { ok })
    }

    fn saved(&self) -> Result<Response> {
        let store = self.store.lock().expect("store mutex poisoned");
        Ok(Response::Hits {
            hits: store.saved()?,
            unreachable: Vec::new(),
        })
    }

    fn forget(&self, id: &str) -> Result<Response> {
        let store = self.store.lock().expect("store mutex poisoned");
        let ok = store.forget(id)?;
        Ok(Response::Ok { ok })
    }

    fn set_note(&self, id: &str, note: &str) -> Result<Response> {
        let store = self.store.lock().expect("store mutex poisoned");
        Ok(Response::Ok {
            ok: store.set_note(id, note)?,
        })
    }

    fn set_tags(&self, id: &str, tags: &[String]) -> Result<Response> {
        let store = self.store.lock().expect("store mutex poisoned");
        Ok(Response::Ok {
            ok: store.set_tags(id, tags)?,
        })
    }

    /// Reveal a conversation's source file in the OS file manager (FR-ACT-01).
    /// The daemon runs locally, so it can hand the path to the platform opener;
    /// this works for the web UI and the native shell alike.
    fn open_source(&self, id: &str) -> Result<Response> {
        let path = {
            let store = self.store.lock().expect("store mutex poisoned");
            store.get_conversation(id)?.map(|c| c.source_path)
        };
        let Some(path) = path.filter(|p| !p.is_empty()) else {
            return Ok(Response::Ok { ok: false });
        };
        let ok = open_in_file_manager(&path);
        Ok(Response::Ok { ok })
    }

    /// Search for the top matches (across devices) and render them into a
    /// paste-ready context block (AGENT_API §5). Remote transcripts are fetched
    /// from their owning peer so the block spans devices (ADR-010).
    fn context(
        &self,
        query: &str,
        mode: Mode,
        limit: usize,
        max_chars: usize,
        f: &Filters,
        devices: Option<&[String]>,
    ) -> Result<Response> {
        let (hits, _unreachable) = self.federated_hits(query, mode, limit, f, devices)?;
        // Fetch transcripts concurrently — remote ones are network round-trips, so
        // doing them serially would cost up to one timeout *per* remote hit.
        let handles: Vec<_> = hits
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, h)| {
                let this = self.clone();
                thread::spawn(move || (i, this.fetch_conversation_for(&h)))
            })
            .collect();
        let mut indexed: Vec<(usize, StoredConversation)> = handles
            .into_iter()
            .filter_map(|j| j.join().ok())
            .filter_map(|(i, c)| c.map(|c| (i, c)))
            .collect();
        indexed.sort_by_key(|(i, _)| *i); // keep ranked order
        let convos: Vec<StoredConversation> = indexed.into_iter().map(|(_, c)| c).collect();
        let block = {
            let store = self.store.lock().expect("store mutex poisoned");
            store.render_conversations(&convos, max_chars)
        };
        Ok(Response::Context {
            markdown: block.markdown,
            sources: block.sources,
            token_estimate: block.token_estimate,
        })
    }

    fn fetch_conversation_for(&self, hit: &SearchHit) -> Option<StoredConversation> {
        self.fetch_conversation(&hit.conversation_id, hit.device.as_deref())
    }

    /// Fetch a conversation from whichever device owns it: locally if it's ours
    /// (or untagged), otherwise via a token-gated `PeerGetConversation` to the
    /// named peer.
    fn fetch_conversation(&self, id: &str, device: Option<&str>) -> Option<StoredConversation> {
        let local_name = self.federation.as_ref().map(|f| f.device());
        let is_local = match (device, local_name.as_deref()) {
            (None, _) | (Some(_), None) => true,
            (Some(d), Some(me)) => d == me,
        };
        if is_local {
            let store = self.store.lock().expect("store mutex poisoned");
            return store.get_conversation(id).ok().flatten();
        }
        let fed = self.federation.as_ref()?;
        let peer = fed
            .peers()
            .into_iter()
            .find(|p| Some(p.name.as_str()) == device)?;
        let req = Request::PeerGetConversation {
            token: fed.token(),
            id: id.to_string(),
        };
        match Client::new(peer.addr).call_timeout(&req, fed.timeout) {
            Ok(Response::Conversation { conversation }) => conversation,
            _ => None,
        }
    }

    /// Answer a peer's request for one conversation: token-gated, local fetch,
    /// honouring the serve-scope (an excluded thread isn't served either).
    fn peer_get_conversation(&self, token: Option<&str>, id: &str) -> Response {
        let Some(fed) = &self.federation else {
            return Response::Error {
                message: "federation not enabled on this device".into(),
            };
        };
        if let Some(expected) = fed.token() {
            if !token.is_some_and(|t| token_eq(t, &expected)) {
                return Response::Error {
                    message: "unauthorized peer (bad or missing token)".into(),
                };
            }
        }
        let convo = {
            let store = self.store.lock().expect("store mutex poisoned");
            store.get_conversation(id).ok().flatten()
        };
        let convo = convo.filter(|c| fed.serves(&c.tool, c.project.as_deref()));
        Response::Conversation {
            conversation: convo,
        }
    }

    /// Read this device's federation identity + serve-scope (Settings panel).
    fn federation_config(&self) -> Response {
        let Some(fed) = &self.federation else {
            return Response::FederationConfig {
                device: "this device".into(),
                has_token: false,
                token_in_keyring: false,
                listen_addr: None,
                exclude_tools: Vec::new(),
                exclude_projects: Vec::new(),
                pairing_code: None,
            };
        };
        let (device, has_token, token_in_keyring, exclude_tools, exclude_projects) =
            fed.config_snapshot();
        Response::FederationConfig {
            device,
            has_token,
            token_in_keyring,
            listen_addr: fed.listen_addr.clone(),
            exclude_tools,
            exclude_projects,
            pairing_code: fed.pairing_code(),
        }
    }

    /// Update identity / serve-scope and return the refreshed config.
    fn set_federation_config(
        &self,
        device: Option<String>,
        token: Option<String>,
        exclude_tools: Option<Vec<String>>,
        exclude_projects: Option<Vec<String>>,
    ) -> Response {
        let Some(fed) = &self.federation else {
            return Response::Error {
                message: "cross-device search isn't enabled on this device".into(),
            };
        };
        fed.set_config(device, token, exclude_tools, exclude_projects);
        self.federation_config()
    }

    /// One-paste onboarding: decode a pairing code, adopt its token if we have
    /// none, approve the embedded peer, and return the refreshed device list.
    fn pair_with_code(&self, code: &str) -> Response {
        let Some(fed) = &self.federation else {
            return Response::Error {
                message: "enable cross-device search first (set a device name)".into(),
            };
        };
        let Some((token, peer)) = federation::decode_pairing(code) else {
            return Response::Error {
                message: "that doesn't look like a valid pairing code".into(),
            };
        };
        fed.adopt_token_if_absent(&token);
        fed.add_peer(peer);
        self.devices_list()
    }
}

fn err(e: anyhow::Error) -> Response {
    Response::Error {
        message: format!("{e:#}"),
    }
}

/// Constant-time string compare for the shared federation token, so verifying it
/// doesn't leak its contents through response timing. (Length is not secret.)
fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Reveal `path` in the platform file manager (best effort). Returns whether the
/// opener launched. Uses `open -R` on macOS, `explorer /select,` on Windows, and
/// `xdg-open` on the containing dir elsewhere.
fn open_in_file_manager(path: &str) -> bool {
    use std::process::Command;
    let spawned = if cfg!(target_os = "macos") {
        Command::new("open").arg("-R").arg(path).spawn()
    } else if cfg!(target_os = "windows") {
        Command::new("explorer")
            .arg(format!("/select,{path}"))
            .spawn()
    } else {
        // Linux/other: reveal the containing directory.
        let dir = std::path::Path::new(path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from(path));
        Command::new("xdg-open").arg(dir).spawn()
    };
    spawned.is_ok()
}

/// A thin client for talking to a running daemon.
pub struct Client {
    addr: String,
}

impl Client {
    pub fn new(addr: impl Into<String>) -> Self {
        Self { addr: addr.into() }
    }

    /// Default client: `CROSSTHREADS_ADDR` or [`DEFAULT_ADDR`].
    pub fn from_env() -> Self {
        Self::new(std::env::var("CROSSTHREADS_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.into()))
    }

    /// Send one request and read one response.
    pub fn call(&self, req: &Request) -> Result<Response> {
        let stream = TcpStream::connect(&self.addr)
            .with_context(|| format!("connecting to daemon at {}", self.addr))?;
        self.exchange(stream, req)
    }

    /// Like [`call`], but bounded by `timeout` on connect, read, and write — for
    /// fanning a search out to peer daemons that may be slow or offline
    /// (ADR-010). Resolves the address (so MagicDNS / hostnames work) and
    /// connects to the first candidate.
    pub fn call_timeout(&self, req: &Request, timeout: Duration) -> Result<Response> {
        let sock = self
            .addr
            .to_socket_addrs()
            .with_context(|| format!("resolving peer {}", self.addr))?
            .next()
            .with_context(|| format!("no address for peer {}", self.addr))?;
        let stream = TcpStream::connect_timeout(&sock, timeout)
            .with_context(|| format!("connecting to peer {}", self.addr))?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        self.exchange(stream, req)
    }

    /// Write one request line and read one response line on `stream`.
    fn exchange(&self, stream: TcpStream, req: &Request) -> Result<Response> {
        let mut writer = stream.try_clone()?;
        let mut bytes = serde_json::to_vec(req)?;
        bytes.push(b'\n');
        writer.write_all(&bytes)?;
        writer.flush()?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    }

    pub fn search(
        &self,
        query: &str,
        mode: Mode,
        limit: usize,
        filters: Filters,
    ) -> Result<Vec<SearchHit>> {
        self.search_devices(query, mode, limit, filters, None)
    }

    /// Search, optionally restricting which devices are queried (ADR-010).
    /// `devices = None` searches this device + all reachable peers.
    pub fn search_devices(
        &self,
        query: &str,
        mode: Mode,
        limit: usize,
        filters: Filters,
        devices: Option<Vec<String>>,
    ) -> Result<Vec<SearchHit>> {
        match self.call(&Request::Search {
            query: query.into(),
            mode,
            limit,
            filters,
            devices,
        })? {
            Response::Hits { hits, .. } => Ok(hits),
            Response::Error { message } => Err(anyhow::anyhow!(message)),
            other => Err(anyhow::anyhow!("unexpected response: {other:?}")),
        }
    }

    /// Build a paste-ready context block from the top matches for `query`.
    /// Returns the markdown and the conversation ids included.
    pub fn build_context(
        &self,
        query: &str,
        mode: Mode,
        limit: usize,
        max_chars: usize,
        filters: Filters,
    ) -> Result<(String, Vec<String>)> {
        self.build_context_devices(query, mode, limit, max_chars, filters, None)
    }

    /// Build context, optionally restricting which devices contribute (ADR-010).
    #[allow(clippy::too_many_arguments)]
    pub fn build_context_devices(
        &self,
        query: &str,
        mode: Mode,
        limit: usize,
        max_chars: usize,
        filters: Filters,
        devices: Option<Vec<String>>,
    ) -> Result<(String, Vec<String>)> {
        match self.call(&Request::Context {
            query: query.into(),
            mode,
            limit,
            max_chars,
            filters,
            devices,
        })? {
            Response::Context {
                markdown, sources, ..
            } => Ok((markdown, sources)),
            Response::Error { message } => Err(anyhow::anyhow!(message)),
            other => Err(anyhow::anyhow!("unexpected response: {other:?}")),
        }
    }
}
