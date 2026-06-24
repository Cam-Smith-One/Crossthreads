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
                    from your other devices (ADR-010). Use `auto` to bind to
                    this device's Tailscale IP automatically (only once a
                    federation token is set).
    --http <ADDR>   Also serve the HTTP/JSON API + UI (e.g. 127.0.0.1:47101)
    --ui <DIR>      Static UI directory to serve over HTTP (built frontend)
    --db <PATH>     Index database path (override with CROSSTHREADS_DB)
    --no-watch      Don't start the filesystem watcher

  Cross-device search (federation, ADR-010):
    --device-name <NAME>   This device's name in results (env CROSSTHREADS_DEVICE_NAME)
    --peer <NAME=ADDR>     A peer daemon to also search; repeatable. ADDR is a
                           tailnet host:port, e.g. --peer linux-desktop:47100
                           (env CROSSTHREADS_PEERS = comma-separated NAME=ADDR list)
    --fed-token <TOKEN>    Shared secret peers must present (env CROSSTHREADS_FED_TOKEN).
                           Stored in the OS keychain when available.
    --fed-timeout-ms <MS>  Per-peer timeout (default 1500)
    --serve-exclude-tool <TOOL>      Don't serve this tool's threads to peers; repeatable
    --serve-exclude-project <SUBSTR> Don't serve projects matching this substring; repeatable

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
    let mut exclude_tools: Vec<String> =
        csv(&std::env::var("CROSSTHREADS_SERVE_EXCLUDE_TOOLS").unwrap_or_default());
    let mut exclude_projects: Vec<String> =
        csv(&std::env::var("CROSSTHREADS_SERVE_EXCLUDE_PROJECTS").unwrap_or_default());

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
            "--serve-exclude-tool" => exclude_tools.push(
                it.next()
                    .context("--serve-exclude-tool needs a value")?
                    .clone(),
            ),
            "--serve-exclude-project" => exclude_projects.push(
                it.next()
                    .context("--serve-exclude-project needs a value")?
                    .clone(),
            ),
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
    // A small pool of read-only connections (WAL) so concurrent searches/reads
    // don't serialize on the single writer lock.
    let read_pool = std::thread::available_parallelism()
        .map(|n| n.get().clamp(2, 8))
        .unwrap_or(4);
    let mut daemon = Daemon::new(store, embedder).with_read_pool(&db_path, read_pool);

    // Federation is always available so the Settings → Devices panel can set it
    // up; with no peers/token it just answers locally (the default bind is
    // loopback). Merge CLI/env with the persisted config (CLI/env win for
    // device; peers and scope are the union).
    let fed_config_path = federation_config_path();
    let saved = ct_daemon::federation::PersistedConfig::load(&fed_config_path);
    let device = device_name
        .or(saved.device)
        .unwrap_or_else(default_device_name);

    // Token precedence: CLI/env > OS keychain > plaintext config file.
    let file_token = saved.token.clone();
    let keyring_token = ct_daemon::federation::token_store::get();
    let from_keyring = token.is_none() && keyring_token.is_some();
    let token = token.or(keyring_token).or(file_token.clone());
    // Store the token in the keychain to encrypt at rest — but only when it isn't
    // already there, so we don't re-write (and re-prompt) on every startup.
    let token_in_keyring = if from_keyring {
        true
    } else {
        token
            .as_deref()
            .map(ct_daemon::federation::token_store::set)
            .unwrap_or(false)
    };

    // `--addr auto`: bind federation to this device's tailnet IP so peers can
    // reach it for cross-device search. Gated on a token being set, so we never
    // expose the daemon on the tailnet without authentication; falls back to the
    // loopback default when there's no token or Tailscale isn't connected.
    if addr == "auto" {
        addr = match (&token, ct_daemon::federation::detect_self_tailnet_ip()) {
            (Some(_), Some(ip)) => {
                let a = format!("{ip}:47100");
                eprintln!("crossthreadsd: federation reachable at {a} (token required)");
                a
            }
            (None, Some(_)) => {
                eprintln!(
                    "crossthreadsd: tailnet detected but no token set — staying on \
                     loopback. Set a token in Settings → Devices to enable pairing."
                );
                DEFAULT_ADDR.to_string()
            }
            _ => DEFAULT_ADDR.to_string(),
        };
    }

    let mut all_peers = saved.peers;
    for p in peers {
        match all_peers.iter_mut().find(|q| q.name == p.name) {
            Some(existing) => existing.addr = p.addr,
            None => all_peers.push(p),
        }
    }
    extend_unique(&mut exclude_tools, saved.exclude_tools);
    extend_unique(&mut exclude_projects, saved.exclude_projects);

    eprintln!(
        "crossthreadsd: federation on as '{device}' — {} peer(s){}",
        all_peers.len(),
        if token.is_some() { ", token set" } else { "" }
    );
    // A pairing code is only meaningful when bound to a non-loopback (tailnet)
    // address that a peer could actually reach.
    let peer_addr =
        (!addr.starts_with("127.") && !addr.starts_with("localhost")).then(|| addr.clone());
    let fed = Federation::new(
        device,
        token,
        all_peers,
        Duration::from_millis(fed_timeout_ms),
    )
    .with_config_path(fed_config_path)
    .with_listen_addr(peer_addr)
    .with_scope(exclude_tools, exclude_projects)
    .with_token_in_keyring(token_in_keyring);
    // Migrate a plaintext file token into the keychain: rewrite the file w/o it.
    if token_in_keyring && file_token.is_some() {
        fed.persist();
    }
    daemon = daemon.with_federation(fed);

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

    // Proactive: if the weekly review is a week stale and a model is configured,
    // regenerate it in the background so it's ready when the user opens the app.
    daemon.spawn_weekly_refresh();

    if let Some(http_addr) = http {
        if let Some(dir) = &ui {
            if !dir.join("index.html").is_file() {
                eprintln!(
                    "crossthreadsd: warning: --ui {} has no index.html; \
                     the web app will 404 (pass the built UI directory)",
                    dir.display()
                );
            }
        }
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

/// Split a comma-separated list, trimming and dropping empties.
fn csv(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Append items from `extra` not already in `base`.
fn extend_unique(base: &mut Vec<String>, extra: Vec<String>) {
    for s in extra {
        if !base.contains(&s) {
            base.push(s);
        }
    }
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
