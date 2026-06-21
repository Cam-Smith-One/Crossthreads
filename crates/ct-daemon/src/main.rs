//! `crossthreadsd` — the Crossthreads background daemon (ADR-005).
//!
//! Opens the single index, runs an initial index pass, starts file-watching,
//! and serves search/status/reindex on a loopback socket. The desktop app,
//! CLI, and MCP server connect as thin clients.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use ct_daemon::{Daemon, Federation, Peer, DEFAULT_ADDR};

const USAGE: &str = "\
crossthreadsd — Crossthreads background daemon

USAGE:
    crossthreadsd [OPTIONS]

OPTIONS:
    --addr <ADDR>   Listen address (default: 127.0.0.1:47100; override with
                    CROSSTHREADS_ADDR). Set to a tailnet IP to be searchable
                    from your other devices (ADR-010).
    --http <ADDR>   Also serve the HTTP/JSON API + UI (e.g. 127.0.0.1:47101)
    --ui <DIR>      Static UI directory to serve over HTTP (built frontend)
    --db <PATH>     Index database path (override with CROSSTHREADS_DB)
    --no-watch      Don't start the filesystem watcher

  Cross-device search (federation, ADR-010):
    --device-name <NAME>   This device's name in results (env CROSSTHREADS_DEVICE_NAME)
    --peer <NAME=ADDR>     A peer daemon to also search; repeatable. ADDR is a
                           tailnet host:port, e.g. --peer linux-desktop:47100
                           (env CROSSTHREADS_PEERS = comma-separated NAME=ADDR list)
    --fed-token <TOKEN>    Shared secret peers must present (env CROSSTHREADS_FED_TOKEN)
    --fed-timeout-ms <MS>  Per-peer timeout (default 1500)

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

    let mut device_name: Option<String> = std::env::var("CROSSTHREADS_DEVICE_NAME").ok();
    let mut token: Option<String> = std::env::var("CROSSTHREADS_FED_TOKEN").ok();
    let mut peers: Vec<Peer> =
        parse_peers(&std::env::var("CROSSTHREADS_PEERS").unwrap_or_default())?;
    let mut fed_timeout_ms: u64 = 1500;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--addr" => addr = it.next().context("--addr needs a value")?.clone(),
            "--http" => http = Some(it.next().context("--http needs a value")?.clone()),
            "--ui" => ui = Some(PathBuf::from(it.next().context("--ui needs a value")?)),
            "--db" => db = Some(PathBuf::from(it.next().context("--db needs a value")?)),
            "--no-watch" => watch = false,
            "--device-name" => {
                device_name = Some(it.next().context("--device-name needs a value")?.clone())
            }
            "--fed-token" => token = Some(it.next().context("--fed-token needs a value")?.clone()),
            "--peer" => peers.push(parse_peer(it.next().context("--peer needs NAME=ADDR")?)?),
            "--fed-timeout-ms" => {
                fed_timeout_ms = it
                    .next()
                    .context("--fed-timeout-ms needs a value")?
                    .parse()
                    .context("--fed-timeout-ms must be a number")?
            }
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
    let mut daemon = Daemon::new(store, embedder);

    // Merge persisted federation config (peers approved via the Devices panel)
    // with CLI/env. CLI/env win for device/token; peers are the union.
    let fed_config_path = federation_config_path();
    let saved = ct_daemon::federation::PersistedConfig::load(&fed_config_path);
    let device_name = device_name.or(saved.device);
    let token = token.or(saved.token);
    let mut all_peers = saved.peers;
    for p in peers {
        match all_peers.iter_mut().find(|q| q.name == p.name) {
            Some(existing) => existing.addr = p.addr,
            None => all_peers.push(p),
        }
    }

    // Federation is opt-in: enabled when the user names this device, has peers,
    // or sets a token. A daemon with no peers can still be a *searchable* peer.
    if device_name.is_some() || !all_peers.is_empty() || token.is_some() {
        let device = device_name.unwrap_or_else(default_device_name);
        eprintln!(
            "crossthreadsd: federation on as '{device}' — {} peer(s){}",
            all_peers.len(),
            if token.is_some() { ", token set" } else { "" }
        );
        daemon = daemon.with_federation(
            Federation::new(
                device,
                token,
                all_peers,
                Duration::from_millis(fed_timeout_ms),
            )
            .with_config_path(fed_config_path),
        );
    }

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
                Ok(r) => {
                    eprintln!(
                        "crossthreadsd: initial index complete — {} new, {} present, {} embedded",
                        r.inserted, r.duplicate, r.embedded
                    );
                    // Per-tool coverage, so it's obvious what indexed.
                    if let Ok(counts) = d.counts_by_tool() {
                        for (tool, kind, n) in counts {
                            eprintln!("  {tool} ({kind}): {n}");
                        }
                    }
                }
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

/// Where the approved-peer list is persisted. Override with
/// `CROSSTHREADS_FED_CONFIG`; defaults to the platform config dir.
fn federation_config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("CROSSTHREADS_FED_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("crossthreads")
        .join("federation.json")
}

/// Parse one `NAME=ADDR` peer spec, e.g. `linux-desktop:47100` → name+addr.
fn parse_peer(spec: &str) -> Result<Peer> {
    let (name, addr) = spec
        .split_once('=')
        .with_context(|| format!("peer must be NAME=ADDR, got '{spec}'"))?;
    let (name, addr) = (name.trim(), addr.trim());
    if name.is_empty() || addr.is_empty() {
        anyhow::bail!("peer must be NAME=ADDR, got '{spec}'");
    }
    Ok(Peer {
        name: name.to_string(),
        addr: addr.to_string(),
    })
}

/// Parse a comma-separated `NAME=ADDR,NAME=ADDR` list (empty string → none).
fn parse_peers(list: &str) -> Result<Vec<Peer>> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(parse_peer)
        .collect()
}

/// Best-effort device name when the user didn't supply one: the OS hostname,
/// falling back to a generic label.
fn default_device_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(hostname_from_os)
        .unwrap_or_else(|| "this-device".to_string())
}

#[cfg(unix)]
fn hostname_from_os() -> Option<String> {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(not(unix))]
fn hostname_from_os() -> Option<String> {
    std::env::var("COMPUTERNAME").ok().filter(|s| !s.is_empty())
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
