//! `ct_daemon` — the Crossthreads background daemon (ADR-005).
//!
//! Owns the single [`Store`] writer, the embedder, file-watchers, and a small
//! loopback request server. The desktop app, CLI, and MCP server connect as
//! thin [`Client`]s over the [`protocol`].

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use ct_embed::Embedder;
use ct_store::Store;

mod http;
pub mod protocol;
pub use ct_store::{Filters, SearchHit};
pub use protocol::{Mode, Request, Response, DEFAULT_ADDR};

/// How long to wait for the filesystem to settle before re-indexing.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(750);

/// The running daemon: shared handles to the store and embedder.
#[derive(Clone)]
pub struct Daemon {
    store: Arc<Mutex<Store>>,
    embedder: Arc<dyn Embedder>,
}

impl Daemon {
    /// Build a daemon around an open store and embedder.
    pub fn new(store: Store, embedder: Box<dyn Embedder>) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            embedder: Arc::from(embedder),
        }
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
            } => self
                .search(&query, mode, limit, &filters)
                .unwrap_or_else(err),
            Request::GetConversation { id } => self.get_conversation(&id).unwrap_or_else(err),
            Request::SetFlags {
                id,
                bookmarked,
                pinned,
            } => self.set_flags(&id, bookmarked, pinned).unwrap_or_else(err),
            Request::Saved => self.saved().unwrap_or_else(err),
            Request::OpenSource { id } => self.open_source(&id).unwrap_or_else(err),
            Request::Context {
                query,
                mode,
                limit,
                max_chars,
                filters,
            } => self
                .context(&query, mode, limit, max_chars, &filters)
                .unwrap_or_else(err),
        }
    }

    fn facets(&self) -> Result<Response> {
        let store = self.store.lock().expect("store mutex poisoned");
        Ok(Response::Facets {
            tools: store.facets_tools()?,
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

    fn search(&self, query: &str, mode: Mode, limit: usize, f: &Filters) -> Result<Response> {
        let hits: Vec<SearchHit> = {
            // Embed outside the store lock where possible; keep it simple and
            // correct by embedding first, then locking for the query.
            match mode {
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
            }
        };
        Ok(Response::Hits { hits })
    }

    fn get_conversation(&self, id: &str) -> Result<Response> {
        let store = self.store.lock().expect("store mutex poisoned");
        Ok(Response::Conversation {
            conversation: store.get_conversation(id)?,
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

    /// Search for the top matches and render them into a paste-ready context
    /// block (AGENT_API §5). Embedding happens before the store lock.
    fn context(
        &self,
        query: &str,
        mode: Mode,
        limit: usize,
        max_chars: usize,
        f: &Filters,
    ) -> Result<Response> {
        let qv = match mode {
            Mode::Lexical => None,
            _ => Some(self.embedder.embed_one(query)?),
        };
        let store = self.store.lock().expect("store mutex poisoned");
        let hits = match (mode, &qv) {
            (Mode::Lexical, _) => store.search_filtered(query, limit, f)?,
            (Mode::Semantic, Some(q)) => store.search_semantic_filtered(q, limit, f)?,
            (_, Some(q)) => store.search_hybrid_filtered(query, q, limit, f)?,
            (_, None) => store.search_filtered(query, limit, f)?,
        };
        let ids: Vec<String> = hits.into_iter().map(|h| h.conversation_id).collect();
        let block = store.render_context(&ids, max_chars)?;
        Ok(Response::Context {
            markdown: block.markdown,
            sources: block.sources,
            token_estimate: block.token_estimate,
        })
    }
}

fn err(e: anyhow::Error) -> Response {
    Response::Error {
        message: format!("{e:#}"),
    }
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
        match self.call(&Request::Search {
            query: query.into(),
            mode,
            limit,
            filters,
        })? {
            Response::Hits { hits } => Ok(hits),
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
        match self.call(&Request::Context {
            query: query.into(),
            mode,
            limit,
            max_chars,
            filters,
        })? {
            Response::Context {
                markdown, sources, ..
            } => Ok((markdown, sources)),
            Response::Error { message } => Err(anyhow::anyhow!(message)),
            other => Err(anyhow::anyhow!("unexpected response: {other:?}")),
        }
    }
}
