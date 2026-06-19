//! `crossthreadsd` — the Crossthreads background daemon (ADR-005).
//!
//! Opens the single index, runs an initial index pass, starts file-watching,
//! and serves search/status/reindex on a loopback socket. The desktop app,
//! CLI, and MCP server connect as thin clients.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use ct_daemon::{Daemon, DEFAULT_ADDR};

const USAGE: &str = "\
crossthreadsd — Crossthreads background daemon

USAGE:
    crossthreadsd [OPTIONS]

OPTIONS:
    --addr <ADDR>   Loopback listen address (default: 127.0.0.1:47100;
                    override with CROSSTHREADS_ADDR)
    --http <ADDR>   Also serve the HTTP/JSON API + UI (e.g. 127.0.0.1:47101)
    --ui <DIR>      Static UI directory to serve over HTTP (built frontend)
    --db <PATH>     Index database path (override with CROSSTHREADS_DB)
    --no-watch      Don't start the filesystem watcher
    --help          Show this help
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("crossthreadsd: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut addr = std::env::var("CROSSTHREADS_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
    let mut http: Option<String> = std::env::var("CROSSTHREADS_HTTP").ok();
    let mut ui: Option<PathBuf> = std::env::var_os("CROSSTHREADS_UI").map(PathBuf::from);
    let mut db: Option<PathBuf> = None;
    let mut watch = true;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--addr" => addr = it.next().context("--addr needs a value")?.clone(),
            "--http" => http = Some(it.next().context("--http needs a value")?.clone()),
            "--ui" => ui = Some(PathBuf::from(it.next().context("--ui needs a value")?)),
            "--db" => db = Some(PathBuf::from(it.next().context("--db needs a value")?)),
            "--no-watch" => watch = false,
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => anyhow::bail!("unknown option: {other}"),
        }
    }

    let db_path = resolve_db(db)?;
    let store = ct_store::Store::open(&db_path)?;
    let embedder = ct_embed::default_embedder();
    let daemon = Daemon::new(store, embedder);

    // Index in the BACKGROUND so the UI/API are reachable immediately. A large
    // first index (hundreds of sessions + embeddings) can take a while; the
    // server should come up right away and fill in as indexing progresses.
    eprintln!(
        "crossthreadsd: index {} (initial index running…)",
        db_path.display()
    );
    {
        let d = daemon.clone();
        std::thread::Builder::new()
            .name("ct-initial-index".into())
            .spawn(move || match d.reindex() {
                Ok(r) => eprintln!(
                    "crossthreadsd: initial index complete — {} new, {} present, {} embedded",
                    r.inserted, r.duplicate, r.embedded
                ),
                Err(e) => eprintln!("crossthreadsd: initial index failed: {e:#}"),
            })
            .context("starting initial index")?;
    }

    if watch {
        daemon.spawn_watcher().context("starting watcher")?;
        eprintln!("crossthreadsd: watching for session changes");
    }

    if let Some(http_addr) = http {
        let ui_label = if ui.is_some() { " + UI" } else { "" };
        eprintln!("crossthreadsd: HTTP API{ui_label} on {http_addr}");
        let d = daemon.clone();
        std::thread::Builder::new()
            .name("ct-http".into())
            .spawn(move || {
                if let Err(e) = d.serve_http(&http_addr, ui) {
                    eprintln!("crossthreadsd: http server stopped: {e:#}");
                }
            })
            .context("starting http server")?;
    }

    let listener = TcpListener::bind(&addr).with_context(|| format!("binding {addr}"))?;
    eprintln!("crossthreadsd: listening on {addr}");
    daemon.serve(listener)
}

fn resolve_db(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let path = explicit
        .or_else(|| std::env::var_os("CROSSTHREADS_DB").map(PathBuf::from))
        .unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("crossthreads")
                .join("index.db")
        });
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating index dir {}", parent.display()))?;
    }
    Ok(path)
}
