//! `ct_daemon` — the Crossthreads background daemon (ADR-005).
//!
//! Owns the single [`Store`] writer, the embedder, file-watchers, and a small
//! loopback request server. The desktop app, CLI, and MCP server connect as
//! thin [`Client`]s over the [`protocol`].

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use ct_embed::Embedder;
use ct_store::Store;

pub mod federation;
pub mod fluency;
pub mod glance;
mod http;
pub mod insights;
pub mod optimize;
pub mod protocol;
pub mod resume;
pub mod schedule;
pub mod weekly;
pub use ct_store::{behavior, Filters, SearchHit, StoredConversation};
pub use federation::{Federation, Peer};
pub use protocol::{DeviceInfo, Mode, Request, Response, DEFAULT_ADDR};

/// How long to wait for the filesystem to settle before re-indexing.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(750);

/// Default port a peer daemon listens on, used when discovering tailnet devices.
const FED_PORT: u16 = 47100;

/// The running daemon: shared handles to the store and embedder.
#[derive(Clone)]
pub struct Daemon {
    /// The single writer connection (single-writer invariant, ADR-005).
    store: Arc<Mutex<Store>>,
    /// Read-only connections sharing the writer's vector cache, for concurrent
    /// searches/reads (WAL). Empty ⇒ reads fall back to the writer.
    readers: Arc<Vec<Mutex<Store>>>,
    /// Round-robin cursor over `readers`.
    next_reader: Arc<AtomicUsize>,
    embedder: Arc<dyn Embedder>,
    /// Cross-device federation config (ADR-010); `None` = local-only daemon.
    federation: Option<Arc<Federation>>,
}

impl Daemon {
    /// Build a daemon around an open store and embedder. Reads use the writer
    /// until a read pool is attached with [`Daemon::with_read_pool`].
    pub fn new(store: Store, embedder: Box<dyn Embedder>) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            readers: Arc::new(Vec::new()),
            next_reader: Arc::new(AtomicUsize::new(0)),
            embedder: Arc::from(embedder),
            federation: None,
        }
    }

    /// Attach `n` read-only connections to the index at `path` (WAL), so searches
    /// and other reads run concurrently instead of serializing on the writer
    /// lock. They share the writer's in-memory vector cache. Best-effort: a
    /// connection that fails to open is skipped.
    pub fn with_read_pool(mut self, path: impl AsRef<Path>, n: usize) -> Self {
        let cache = self
            .store
            .lock()
            .expect("store mutex poisoned")
            .cache_handle();
        let mut readers = Vec::new();
        for _ in 0..n {
            match Store::open_read(path.as_ref(), cache.clone()) {
                Ok(s) => readers.push(Mutex::new(s)),
                Err(e) => eprintln!("warn: read-pool connection failed: {e:#}"),
            }
        }
        if !readers.is_empty() {
            eprintln!(
                "crossthreadsd: read pool of {} connection(s)",
                readers.len()
            );
            self.readers = Arc::new(readers);
        }
        self
    }

    /// Lock a read connection from the pool (round-robin), or the writer when no
    /// pool is attached. Use for read-only operations.
    fn read_lock(&self) -> MutexGuard<'_, Store> {
        if self.readers.is_empty() {
            self.store.lock().expect("store mutex poisoned")
        } else {
            let i = self.next_reader.fetch_add(1, Ordering::Relaxed) % self.readers.len();
            self.readers[i].lock().expect("store mutex poisoned")
        }
    }

    /// Lock the single writer connection. Use for any operation that writes.
    fn write_lock(&self) -> MutexGuard<'_, Store> {
        self.store.lock().expect("store mutex poisoned")
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
            let mut store = self.write_lock();
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
                let store = self.write_lock();
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
                let mut store = self.write_lock();
                store.store_embeddings(self.embedder.id(), &rows)?;
            }
            report.embedded += rows.len();
        }

        Ok(report)
    }

    /// Per-(tool, kind) record counts, for a coverage summary.
    pub fn counts_by_tool(&self) -> Result<Vec<(String, String, i64)>> {
        let store = self.read_lock();
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
            Request::GetLlmConfig => llm_config(),
            Request::SetLlmKey { provider, key } => set_llm_key(&provider, &key),
            Request::SetLlmProvider { provider } => set_llm_provider(&provider),
            Request::Themes { k, name } => self.themes(k.unwrap_or(8), name).unwrap_or_else(err),
            Request::Insight { kind, limit } => self.insight(&kind, limit).unwrap_or_else(err),
            Request::Ask {
                question,
                limit,
                filters,
                devices,
            } => self
                .ask(&question, limit, &filters, devices.as_deref())
                .unwrap_or_else(err),
            Request::Activity { bucket, filters } => self
                .activity(bucket.as_deref().unwrap_or("day"), &filters)
                .unwrap_or_else(err),
            Request::Graph { limit } => self.graph(limit.unwrap_or(40)).unwrap_or_else(err),
            Request::Metrics { filters } => self.metrics(&filters).unwrap_or_else(err),
            Request::WeeklyReview { force } => self.weekly_review(force).unwrap_or_else(err),
            Request::Trends { days } => self.trends(days).unwrap_or_else(err),
            Request::Optimize { force } => self.optimize(force).unwrap_or_else(err),
            Request::Fluency { window_days } => self.fluency(window_days).unwrap_or_else(err),
            Request::Glance => self.glance().unwrap_or_else(err),
            Request::SetFlags {
                id,
                bookmarked,
                pinned,
            } => self.set_flags(&id, bookmarked, pinned).unwrap_or_else(err),
            Request::Saved => self.saved().unwrap_or_else(err),
            Request::OpenSource { id } => self.open_source(&id).unwrap_or_else(err),
            Request::Resume { id } => self.resume(&id).unwrap_or_else(err),
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
        let store = self.read_lock();
        Ok(Response::Facets {
            tools: store.facets_tools()?,
            tags: store.facets_tags()?,
        })
    }

    fn themes(&self, k: usize, name: bool) -> Result<Response> {
        let mut themes = {
            let store = self.read_lock();
            store.themes(k, 8)?
        };
        // Optional LLM naming: replace each cluster's keyword label with a short
        // generated name. Best-effort — leave the keyword label if the model is
        // unavailable or a call fails. Done after dropping the store lock.
        if name && ct_llm::available() {
            for theme in &mut themes {
                if let Some(named) = insights::name_theme(theme) {
                    theme.label = named;
                }
            }
        }
        Ok(Response::Themes { themes })
    }

    /// Synthesize an LLM insight over recent history. Gathers conversation text
    /// under the read lock, drops it, then calls the model (no lock held).
    fn insight(&self, kind: &str, limit: Option<usize>) -> Result<Response> {
        // Behavioral kinds ("how you work") are grounded in the metrics layer:
        // gather under the lock, drop it, then call the model.
        if let Some(b) = insights::Behavioral::parse(kind) {
            let input = {
                let store = self.read_lock();
                insights::gather_behavioral(&store, b)?
            };
            let (markdown, sources) = insights::synthesize_behavioral(input)?;
            return Ok(Response::Insight { markdown, sources });
        }
        let kind = insights::Kind::parse(kind).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown insight kind `{kind}` — expected one of: open_loops, \
                 knowledge_cards, decision_log, how_i_work, digest, recurrence, \
                 work_dna, gaps, prompt_coach, process_miner"
            )
        })?;
        let n = limit.unwrap_or_else(|| kind.default_limit());
        let convos = {
            let store = self.read_lock();
            store.recent_conversations(n, 1500)?
        };
        let (markdown, sources) = insights::synthesize(&convos, kind)?;
        Ok(Response::Insight { markdown, sources })
    }

    fn status(&self) -> Result<Response> {
        let store = self.read_lock();
        // Roll the per-(tool, kind) counts up into (tool, threads, skills) for
        // the "what's indexed" chips, busiest tool first.
        let mut by_tool: std::collections::BTreeMap<String, (i64, i64)> = Default::default();
        for (tool, kind, n) in store.counts_by_tool()? {
            let e = by_tool.entry(tool).or_default();
            if kind == "skill" {
                e.1 += n;
            } else {
                e.0 += n;
            }
        }
        let mut tools: Vec<(String, i64, i64)> =
            by_tool.into_iter().map(|(t, (a, b))| (t, a, b)).collect();
        tools.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)).then_with(|| a.0.cmp(&b.0)));
        Ok(Response::Status {
            conversations: store.conversation_count()?,
            embeddings: store.embedding_count()?,
            embedder: self.embedder.id().to_string(),
            tools,
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
                let store = self.read_lock();
                store.search_filtered(query, limit, f)?
            }
            Mode::Semantic => {
                let q = self.embedder.embed_one(query)?;
                let store = self.read_lock();
                store.search_semantic_filtered(&q, limit, f)?
            }
            Mode::Hybrid => {
                let q = self.embedder.embed_one(query)?;
                let store = self.read_lock();
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
        let store = self.write_lock();
        let ok = store.set_flags(id, bookmarked, pinned)?;
        Ok(Response::Ok { ok })
    }

    fn saved(&self) -> Result<Response> {
        let store = self.read_lock();
        Ok(Response::Hits {
            hits: store.saved()?,
            unreachable: Vec::new(),
        })
    }

    fn forget(&self, id: &str) -> Result<Response> {
        let store = self.write_lock();
        let ok = store.forget(id)?;
        Ok(Response::Ok { ok })
    }

    fn set_note(&self, id: &str, note: &str) -> Result<Response> {
        let store = self.write_lock();
        Ok(Response::Ok {
            ok: store.set_note(id, note)?,
        })
    }

    fn set_tags(&self, id: &str, tags: &[String]) -> Result<Response> {
        let store = self.write_lock();
        Ok(Response::Ok {
            ok: store.set_tags(id, tags)?,
        })
    }

    /// Reveal a conversation's source file in the OS file manager (FR-ACT-01).
    /// The daemon runs locally, so it can hand the path to the platform opener;
    /// this works for the web UI and the native shell alike.
    fn open_source(&self, id: &str) -> Result<Response> {
        let path = {
            let store = self.read_lock();
            store.get_conversation(id)?.map(|c| c.source_path)
        };
        let Some(path) = path.filter(|p| !p.is_empty()) else {
            return Ok(Response::Ok { ok: false });
        };
        let ok = open_in_file_manager(&path);
        Ok(Response::Ok { ok })
    }

    fn resume(&self, id: &str) -> Result<Response> {
        let conv = {
            let store = self.read_lock();
            store.get_conversation(id)?
        };
        let resume = conv
            .filter(|c| !c.source_path.is_empty())
            .map(|c| crate::resume::resume_for(&c.tool, &c.source_path, c.project.as_deref()));
        Ok(Response::Resume { resume })
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
        let block = self.context_block(query, mode, limit, max_chars, f, devices)?;
        Ok(Response::Context {
            markdown: block.markdown,
            sources: block.sources,
            token_estimate: block.token_estimate,
        })
    }

    /// Retrieve the top matches for `query` (federated) and render them into a
    /// paste-ready context block. Shared by `context` and `ask`.
    fn context_block(
        &self,
        query: &str,
        mode: Mode,
        limit: usize,
        max_chars: usize,
        f: &Filters,
        devices: Option<&[String]>,
    ) -> Result<ct_store::ContextBlock> {
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
        let store = self.read_lock();
        Ok(store.render_conversations(&convos, max_chars))
    }

    /// Answer a natural-language question from the user's own history: retrieve
    /// the most relevant past sessions (across tools and devices), then ask the
    /// model to synthesize a cited answer. Degrades to the retrieved context
    /// block when no model is configured (retrieval-only, like `recall`).
    fn ask(
        &self,
        question: &str,
        limit: usize,
        f: &Filters,
        devices: Option<&[String]>,
    ) -> Result<Response> {
        // Budget scales with how many sessions we draw on, so a wider retrieval
        // isn't immediately truncated away.
        let max_chars = (limit * 1400).clamp(9000, 24000);
        let block = self.context_block(question, Mode::Hybrid, limit, max_chars, f, devices)?;
        if block.sources.is_empty() {
            return Ok(Response::Answer {
                markdown: format!("No past sessions found relevant to \u{201c}{question}\u{201d}."),
                sources: Vec::new(),
                llm_used: false,
            });
        }
        if !ct_llm::available() {
            // No model — hand back the retrieved context so the caller (often an
            // agent) can synthesize, mirroring the `recall` contract.
            return Ok(Response::Answer {
                markdown: block.markdown,
                sources: block.sources,
                llm_used: false,
            });
        }
        let system = "You answer a developer's question using ONLY the provided excerpts from \
                      their own past AI coding sessions. Cite the session title in parentheses \
                      after each claim. If the excerpts don't contain the answer, say so plainly \
                      rather than guessing. Be concise; use Markdown.";
        let user = format!(
            "Question: {question}\n\n--- RELEVANT PAST SESSIONS ---\n{}",
            block.markdown
        );
        let markdown = ct_llm::complete_with(system, &user, 1024)?;
        Ok(Response::Answer {
            markdown,
            sources: block.sources,
            llm_used: true,
        })
    }

    /// Session activity over time (local index), for the temporal view.
    fn activity(&self, bucket: &str, f: &Filters) -> Result<Response> {
        let bucket = if bucket.eq_ignore_ascii_case("week") {
            "week"
        } else {
            "day"
        };
        let store = self.read_lock();
        Ok(Response::Activity {
            bucket: bucket.to_string(),
            periods: store.activity(bucket, f)?,
        })
    }

    /// The knowledge graph over the local index, for the graph view and agents.
    fn graph(&self, limit: usize) -> Result<Response> {
        let store = self.read_lock();
        Ok(Response::Graph {
            graph: store.graph(limit)?,
        })
    }

    /// Deterministic behavioral metrics ("how you work"), for the profile view
    /// and agents. No model involved.
    fn metrics(&self, f: &Filters) -> Result<Response> {
        let store = self.read_lock();
        let (metrics, _per) = store.work_metrics(f)?;
        Ok(Response::Metrics { metrics })
    }

    /// Behavioral trends (current window vs previous). Deterministic.
    fn trends(&self, days: Option<i64>) -> Result<Response> {
        let days = days.unwrap_or(optimize::WINDOW_DAYS).clamp(1, 365);
        let store = self.read_lock();
        let rows = optimize::trends(&store, chrono::Utc::now(), days)?;
        Ok(Response::Trends { rows })
    }

    /// The "optimize how you work" loop. Persists a prescription, so it takes
    /// the writer lock; all deterministic (no model), so it's brief.
    fn optimize(&self, force: bool) -> Result<Response> {
        let store = self.write_lock();
        let report = optimize::run(&store, chrono::Utc::now(), force)?;
        Ok(Response::Optimize {
            markdown: report.markdown,
            has_active: report.has_active,
        })
    }

    /// The 4D AI-fluency view (Delegation/Description/Discernment/Diligence),
    /// scored from the deterministic metrics with a reflective question. Read-only.
    fn fluency(&self, window_days: Option<i64>) -> Result<Response> {
        let days = window_days.unwrap_or(fluency::WINDOW_DAYS);
        let store = self.read_lock();
        let report = fluency::report(&store, chrono::Utc::now(), days)?;
        Ok(Response::Fluency { report })
    }

    /// The menu-bar glance (usage + insight). Deterministic; read-only.
    fn glance(&self) -> Result<Response> {
        let store = self.read_lock();
        let glance = glance::build_now(&store)?;
        Ok(Response::Glance { glance })
    }

    /// The proactive weekly review: return the cached one when it's fresh, else
    /// regenerate. Gathering and persistence take the lock briefly; the model
    /// call in between runs with no lock held.
    fn weekly_review(&self, force: bool) -> Result<Response> {
        if !force {
            let cached = {
                let store = self.read_lock();
                (!weekly::is_stale(&store))
                    .then(|| weekly::cached(&store))
                    .transpose()?
                    .flatten()
            };
            if let Some(r) = cached {
                return Ok(Response::WeeklyReview {
                    markdown: r.markdown,
                    generated_at: r.generated_at,
                    period_start: r.period_start,
                    llm_used: r.llm_used,
                    fresh: false,
                });
            }
        }
        let review = self.regenerate_weekly()?;
        Ok(Response::WeeklyReview {
            markdown: review.markdown,
            generated_at: review.generated_at,
            period_start: review.period_start,
            llm_used: review.llm_used,
            fresh: true,
        })
    }

    /// Gather (read lock) → synthesize (model, no lock) → persist (write lock).
    fn regenerate_weekly(&self) -> Result<weekly::WeeklyReview> {
        let gathered = {
            let store = self.read_lock();
            weekly::gather(&store)?
        };
        let review = weekly::synthesize(gathered);
        {
            let store = self.write_lock();
            weekly::persist(&store, &review)?;
        }
        Ok(review)
    }

    /// Best-effort: if the cached weekly review is a week stale and a model is
    /// configured, regenerate it in the background so it's waiting when the user
    /// next opens the app (the "proactive" path). Spawns and returns immediately.
    pub fn spawn_weekly_refresh(&self) {
        let stale = {
            let store = self.read_lock();
            weekly::is_stale(&store)
        };
        if !stale || !ct_llm::available() {
            return;
        }
        let this = self.clone();
        let _ = thread::Builder::new()
            .name("ct-weekly".into())
            .spawn(move || {
                if let Err(e) = this.regenerate_weekly() {
                    eprintln!("weekly review refresh failed: {e:#}");
                }
            });
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
            let store = self.read_lock();
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
            let store = self.read_lock();
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

/// LLM provider auth status for Settings → Models (secret-free).
fn llm_config() -> Response {
    let (providers, active) = ct_llm::status();
    Response::LlmConfig { providers, active }
}

/// Store (non-empty) or clear (empty) a provider's BYO key, then return status.
fn set_llm_key(provider: &str, key: &str) -> Response {
    match ct_llm::Provider::parse(provider) {
        Some(p) => {
            ct_llm::store::set_key(p, key);
            llm_config()
        }
        None => Response::Error {
            message: format!("unknown provider: {provider}"),
        },
    }
}

/// Pin the active provider (empty clears), then return status.
fn set_llm_provider(provider: &str) -> Response {
    let pinned = if provider.trim().is_empty() {
        None
    } else {
        match ct_llm::Provider::parse(provider) {
            Some(p) => Some(p),
            None => {
                return Response::Error {
                    message: format!("unknown provider: {provider}"),
                }
            }
        }
    };
    ct_llm::store::set_active_provider(pinned);
    llm_config()
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
