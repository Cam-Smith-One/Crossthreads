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

pub mod protocol;
pub use ct_store::SearchHit;
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

    /// Run one indexing pass (single-writer: holds the store lock throughout).
    pub fn reindex(&self) -> Result<ct_index::IndexReport> {
        let mut store = self.store.lock().expect("store mutex poisoned");
        ct_index::index_once(&mut store, &*self.embedder, None)
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
                            eprintln!(
                                "reindex: {} new, {} embedded",
                                r.inserted, r.embedded
                            );
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
            Request::Reindex => self
                .reindex()
                .map(|r| Response::Reindexed {
                    inserted: r.inserted,
                    duplicate: r.duplicate,
                    embedded: r.embedded,
                })
                .unwrap_or_else(err),
            Request::Search { query, mode, limit } => {
                self.search(&query, mode, limit).unwrap_or_else(err)
            }
        }
    }

    fn status(&self) -> Result<Response> {
        let store = self.store.lock().expect("store mutex poisoned");
        Ok(Response::Status {
            conversations: store.conversation_count()?,
            embeddings: store.embedding_count()?,
            embedder: self.embedder.id().to_string(),
        })
    }

    fn search(&self, query: &str, mode: Mode, limit: usize) -> Result<Response> {
        let hits: Vec<SearchHit> = {
            // Embed outside the store lock where possible; keep it simple and
            // correct by embedding first, then locking for the query.
            match mode {
                Mode::Lexical => {
                    let store = self.store.lock().expect("store mutex poisoned");
                    store.search(query, limit)?
                }
                Mode::Semantic => {
                    let q = self.embedder.embed_one(query)?;
                    let store = self.store.lock().expect("store mutex poisoned");
                    store.search_semantic(&q, limit)?
                }
                Mode::Hybrid => {
                    let q = self.embedder.embed_one(query)?;
                    let store = self.store.lock().expect("store mutex poisoned");
                    store.search_hybrid(query, &q, limit)?
                }
            }
        };
        Ok(Response::Hits { hits })
    }
}

fn err(e: anyhow::Error) -> Response {
    Response::Error {
        message: format!("{e:#}"),
    }
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

    pub fn search(&self, query: &str, mode: Mode, limit: usize) -> Result<Vec<SearchHit>> {
        match self.call(&Request::Search {
            query: query.into(),
            mode,
            limit,
        })? {
            Response::Hits { hits } => Ok(hits),
            Response::Error { message } => Err(anyhow::anyhow!(message)),
            other => Err(anyhow::anyhow!("unexpected response: {other:?}")),
        }
    }
}
