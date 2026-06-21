//! Cross-device query federation (ADR-010, design in CROSS_DEVICE_SEARCH.md).
//!
//! Each device keeps only its own index. When federation is configured, a
//! `Search` runs locally *and* is fanned out to peer daemons over a private
//! tunnel (Tailscale/WireGuard); each peer answers a `PeerSearch` from its own
//! index, and the querying device merges everything into one ranked list. Only
//! the query leaves and only matching results come back — no index sync, no
//! central server.
//!
//! This module holds the configuration types, the persisted approved-peers
//! list, the rank-based merge, and on-demand tailnet discovery. The daemon
//! ([`crate::Daemon`]) owns the fan-out and the `PeerSearch` handler.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use ct_store::SearchHit;

use crate::protocol::DiscoveredDevice;

/// A reachable peer device: a human name (shown as the result's device chip)
/// and the address its daemon listens on (a tailnet `host:port` or MagicDNS
/// name, e.g. `linux-desktop:47100`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub name: String,
    pub addr: String,
}

/// Federation settings for this daemon. Present only when the user opts in
/// (via `--device-name` / `--peer` / a saved config); absent means the daemon
/// is purely local and refuses `PeerSearch`.
#[derive(Debug)]
pub struct Federation {
    /// Per-peer connect/read budget; a slow or offline peer is skipped after
    /// this, never hung on.
    pub timeout: Duration,
    /// This daemon's own peer-reachable address (the `--addr` it binds), used to
    /// build a pairing code. `None` when only loopback-bound.
    pub listen_addr: Option<String>,
    /// Mutable identity + serve-scope, so the Settings → Devices panel can edit
    /// the device name, token, and what this device exposes at runtime.
    inner: Mutex<Identity>,
    /// Approved peers to fan a local search out to. Behind a lock so the panel
    /// can add/remove them at runtime.
    peers: Mutex<Vec<Peer>>,
    /// Where the approved-peer list + scope are persisted, if anywhere.
    config_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct Identity {
    /// This device's name, stamped on the hits it serves to peers.
    device: String,
    /// Shared secret. When set, incoming `PeerSearch` must present a match;
    /// when `None`, the tailnet itself is the access boundary.
    token: Option<String>,
    /// The token lives in the OS keychain (vs. the plaintext config file).
    token_in_keyring: bool,
    /// Tools this device will not serve to peers (serve-scope).
    exclude_tools: Vec<String>,
    /// Project substrings this device will not serve to peers.
    exclude_projects: Vec<String>,
}

impl Federation {
    /// Build federation config with a fixed peer set and no persistence.
    pub fn new(device: String, token: Option<String>, peers: Vec<Peer>, timeout: Duration) -> Self {
        Self {
            timeout,
            listen_addr: None,
            inner: Mutex::new(Identity {
                device,
                token,
                token_in_keyring: false,
                exclude_tools: Vec::new(),
                exclude_projects: Vec::new(),
            }),
            peers: Mutex::new(peers),
            config_path: None,
        }
    }

    /// Persist approve/remove/scope changes to `path` (rewritten as JSON).
    pub fn with_config_path(mut self, path: PathBuf) -> Self {
        self.config_path = Some(path);
        self
    }

    /// Record this daemon's peer-reachable bind address (for pairing codes).
    pub fn with_listen_addr(mut self, addr: Option<String>) -> Self {
        self.listen_addr = addr;
        self
    }

    /// Set the serve-scope (tools/projects this device won't expose to peers).
    pub fn with_scope(self, exclude_tools: Vec<String>, exclude_projects: Vec<String>) -> Self {
        {
            let mut id = self.inner.lock().expect("identity mutex poisoned");
            id.exclude_tools = exclude_tools;
            id.exclude_projects = exclude_projects;
        }
        self
    }

    /// Mark the token as living in the OS keychain rather than the config file.
    pub fn with_token_in_keyring(self, yes: bool) -> Self {
        self.inner
            .lock()
            .expect("identity mutex poisoned")
            .token_in_keyring = yes;
        self
    }

    /// This device's name.
    pub fn device(&self) -> String {
        self.inner
            .lock()
            .expect("identity mutex poisoned")
            .device
            .clone()
    }

    /// The shared token, if set.
    pub fn token(&self) -> Option<String> {
        self.inner
            .lock()
            .expect("identity mutex poisoned")
            .token
            .clone()
    }

    /// Whether this device would serve a conversation in `tool`/`project` to a
    /// peer — the serve-scope from ADR-010's trust boundary.
    pub fn serves(&self, tool: &str, project: Option<&str>) -> bool {
        let id = self.inner.lock().expect("identity mutex poisoned");
        if id.exclude_tools.iter().any(|t| t == tool) {
            return false;
        }
        if let Some(p) = project {
            if id
                .exclude_projects
                .iter()
                .any(|x| !x.is_empty() && p.contains(x.as_str()))
            {
                return false;
            }
        }
        true
    }

    /// Snapshot for the Settings panel: (device, has_token, token_in_keyring,
    /// exclude_tools, exclude_projects).
    pub fn config_snapshot(&self) -> (String, bool, bool, Vec<String>, Vec<String>) {
        let id = self.inner.lock().expect("identity mutex poisoned");
        (
            id.device.clone(),
            id.token.is_some(),
            id.token_in_keyring,
            id.exclude_tools.clone(),
            id.exclude_projects.clone(),
        )
    }

    /// A copy-paste code another device can `PairWithCode` to join us: it carries
    /// the shared token and our peer address. `None` until both are known.
    pub fn pairing_code(&self) -> Option<String> {
        let addr = self.listen_addr.clone()?;
        let id = self.inner.lock().expect("identity mutex poisoned");
        let token = id.token.clone()?;
        let payload = PairingPayload {
            token,
            peer: Peer {
                name: id.device.clone(),
                addr,
            },
        };
        Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).ok()?))
    }

    /// Apply a `SetFederationConfig` update (any subset of fields) and persist.
    /// For `token`: `Some("")` clears, `Some(x)` sets, `None` leaves unchanged.
    pub fn set_config(
        &self,
        device: Option<String>,
        token: Option<String>,
        exclude_tools: Option<Vec<String>>,
        exclude_projects: Option<Vec<String>>,
    ) {
        {
            let mut id = self.inner.lock().expect("identity mutex poisoned");
            if let Some(d) = device {
                if !d.trim().is_empty() {
                    id.device = d;
                }
            }
            if let Some(t) = token {
                if t.is_empty() {
                    token_store::delete();
                    id.token = None;
                    id.token_in_keyring = false;
                } else {
                    id.token_in_keyring = token_store::set(&t);
                    id.token = Some(t);
                }
            }
            if let Some(et) = exclude_tools {
                id.exclude_tools = et;
            }
            if let Some(ep) = exclude_projects {
                id.exclude_projects = ep;
            }
        }
        self.save();
    }

    /// Adopt a shared token only if we don't already have one (pairing). Returns
    /// whether it was set. Persisted.
    pub fn adopt_token_if_absent(&self, token: &str) -> bool {
        {
            let mut id = self.inner.lock().expect("identity mutex poisoned");
            if id.token.is_some() || token.is_empty() {
                return false;
            }
            id.token_in_keyring = token_store::set(token);
            id.token = Some(token.to_string());
        }
        self.save();
        true
    }

    /// Snapshot of the approved peers.
    pub fn peers(&self) -> Vec<Peer> {
        self.peers.lock().expect("peers mutex poisoned").clone()
    }

    /// Add (or update the address of) an approved peer and persist. Returns
    /// `true` if it was newly added.
    pub fn add_peer(&self, peer: Peer) -> bool {
        let added = {
            let mut peers = self.peers.lock().expect("peers mutex poisoned");
            match peers.iter_mut().find(|p| p.name == peer.name) {
                Some(existing) => {
                    existing.addr = peer.addr;
                    false
                }
                None => {
                    peers.push(peer);
                    true
                }
            }
        };
        self.save();
        added
    }

    /// Remove an approved peer by name and persist. Returns `true` if present.
    pub fn remove_peer(&self, name: &str) -> bool {
        let removed = {
            let mut peers = self.peers.lock().expect("peers mutex poisoned");
            let before = peers.len();
            peers.retain(|p| p.name != name);
            peers.len() != before
        };
        self.save();
        removed
    }

    /// Rewrite the config file now (e.g. to migrate a plaintext token into the
    /// keychain on startup).
    pub fn persist(&self) {
        self.save();
    }

    /// Write the current device/scope/peers to the config file (best effort).
    /// The token is written **only** when it isn't in the keychain.
    fn save(&self) {
        let Some(path) = &self.config_path else {
            return;
        };
        let peers = self.peers();
        let id = self.inner.lock().expect("identity mutex poisoned");
        let cfg = PersistedConfig {
            device: Some(id.device.clone()),
            token: if id.token_in_keyring {
                None
            } else {
                id.token.clone()
            },
            peers,
            exclude_tools: id.exclude_tools.clone(),
            exclude_projects: id.exclude_projects.clone(),
        };
        if let Ok(json) = serde_json::to_vec_pretty(&cfg) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(path, json) {
                eprintln!(
                    "warn: could not save federation config {}: {e}",
                    path.display()
                );
            }
        }
    }
}

/// Pairing-code payload: the shared token + the issuing device as a peer.
#[derive(Debug, Serialize, Deserialize)]
struct PairingPayload {
    token: String,
    peer: Peer,
}

/// Decode a pairing code into `(token, peer)`. `None` if it isn't a valid code.
pub fn decode_pairing(code: &str) -> Option<(String, Peer)> {
    let bytes = URL_SAFE_NO_PAD.decode(code.trim()).ok()?;
    let p: PairingPayload = serde_json::from_slice(&bytes).ok()?;
    Some((p.token, p.peer))
}

/// OS keychain storage for the federation token (ADR-010). Best-effort: every
/// call degrades to `false`/`None` when no keychain backend is available (e.g.
/// headless CI), so callers fall back to the plaintext config file.
pub mod token_store {
    const SERVICE: &str = "crossthreads";
    const USER: &str = "federation-token";

    pub fn get() -> Option<String> {
        keyring::Entry::new(SERVICE, USER).ok()?.get_password().ok()
    }

    pub fn set(token: &str) -> bool {
        keyring::Entry::new(SERVICE, USER)
            .and_then(|e| e.set_password(token))
            .is_ok()
    }

    pub fn delete() {
        if let Ok(e) = keyring::Entry::new(SERVICE, USER) {
            let _ = e.delete_credential();
        }
    }
}

/// On-disk federation config (`federation.json`). Merged with CLI/env on start.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PersistedConfig {
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub peers: Vec<Peer>,
    #[serde(default)]
    pub exclude_tools: Vec<String>,
    #[serde(default)]
    pub exclude_projects: Vec<String>,
}

impl PersistedConfig {
    /// Load the config from `path`, or a default if it's missing/unreadable.
    pub fn load(path: &PathBuf) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
}

/// Reciprocal-rank-fusion constant. The standard damping value; larger means
/// rank position matters less. Matches the spirit of the store's hybrid RRF.
const RRF_K: f64 = 60.0;

/// Merge several ranked hit lists (local + one per peer) into one list.
///
/// Each hit scores `1 / (RRF_K + rank)` within its own list; a conversation
/// that appears on several devices sums its contributions and rises. Duplicates
/// collapse by `conversation_id` — which is the content hash (see
/// `ct_core::hash`), so the same thread mirrored on two machines becomes one
/// result. The first occurrence wins the kept copy, and lists are passed
/// local-first so a local copy is preferred over a remote duplicate.
pub fn rrf_merge(lists: Vec<Vec<SearchHit>>, limit: usize) -> Vec<SearchHit> {
    let mut scores: HashMap<String, f64> = HashMap::new();
    let mut kept: HashMap<String, SearchHit> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for list in lists {
        for (rank, hit) in list.into_iter().enumerate() {
            let key = hit.conversation_id.clone();
            *scores.entry(key.clone()).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
            kept.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                hit
            });
        }
    }

    // Sort by fused score (desc); ties keep first-seen order for determinism.
    order.sort_by(|a, b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    order
        .into_iter()
        .take(limit)
        .filter_map(|k| kept.remove(&k))
        .collect()
}

/// Liveness probe: is a Crossthreads daemon answering at `addr`?
pub fn probe(addr: &str, timeout: Duration) -> bool {
    matches!(
        crate::Client::new(addr).call_timeout(&crate::Request::Ping, timeout),
        Ok(crate::Response::Pong)
    )
}

/// One-shot tailnet scan: ask Tailscale for peer devices, probe each on `port`,
/// and return those running a reachable Crossthreads daemon. Graceful when
/// Tailscale isn't installed/running (returns an empty list).
pub fn discover(approved: &[Peer], port: u16, timeout: Duration) -> Vec<DiscoveredDevice> {
    tailscale_peers()
        .into_iter()
        .filter_map(|(name, ip)| {
            let addr = format!("{ip}:{port}");
            if !probe(&addr, timeout) {
                return None;
            }
            let already_approved = approved.iter().any(|p| p.name == name || p.addr == addr);
            Some(DiscoveredDevice {
                name,
                addr,
                already_approved,
            })
        })
        .collect()
}

/// Locate the Tailscale CLI. The macOS app (App Store or the standalone GUI)
/// doesn't put `tailscale` on PATH — its binary lives inside the app bundle — so
/// when a bare `tailscale` isn't runnable, fall back to well-known locations.
fn tailscale_bin() -> String {
    let on_path = Command::new("tailscale")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if on_path {
        return "tailscale".into();
    }
    first_existing(TAILSCALE_FALLBACKS).unwrap_or_else(|| "tailscale".into())
}

const TAILSCALE_FALLBACKS: &[&str] = &[
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    "/opt/homebrew/bin/tailscale",
    "/usr/local/bin/tailscale",
];

/// First path in `candidates` that exists on disk. Pure, so it's testable.
fn first_existing(candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(|p| p.to_string())
}

/// Run `tailscale status --json` (locating the CLI via [`tailscale_bin`]) and
/// parse it. `None` when Tailscale isn't installed or connected.
fn tailscale_status_json() -> Option<Value> {
    let out = Command::new(tailscale_bin())
        .arg("status")
        .arg("--json")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

/// Run `tailscale status --json` and extract `(name, tailnet-ip)` per peer.
fn tailscale_peers() -> Vec<(String, String)> {
    tailscale_status_json()
        .map(|v| parse_tailscale_peers(&v))
        .unwrap_or_default()
}

/// Best-effort: this device's own tailnet IPv4 address, read from the `Self`
/// node of `tailscale status --json`. `None` when Tailscale isn't installed or
/// connected — callers then fall back to a loopback bind.
pub fn detect_self_tailnet_ip() -> Option<String> {
    self_tailnet_ip(&tailscale_status_json()?)
}

/// Pull the `Self` node's first IPv4 tailnet address. Pure, so it's testable.
pub fn self_tailnet_ip(v: &Value) -> Option<String> {
    v.get("Self")?
        .get("TailscaleIPs")?
        .as_array()?
        .iter()
        .filter_map(|s| s.as_str())
        .find(|s| !s.contains(':')) // IPv4 only (skip the IPv6 tailnet address)
        .map(str::to_string)
}

/// Pull `(short-name, first-tailnet-ip)` for each peer out of `tailscale status
/// --json` output. Pure, so it's unit-testable against a fixture.
pub fn parse_tailscale_peers(v: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(peers) = v.get("Peer").and_then(|p| p.as_object()) else {
        return out;
    };
    for peer in peers.values() {
        let ip = peer
            .get("TailscaleIPs")
            .and_then(|a| a.as_array())
            .and_then(|a| a.first())
            .and_then(|s| s.as_str());
        // Prefer the MagicDNS short name; fall back to the host name.
        let name = peer
            .get("DNSName")
            .and_then(|s| s.as_str())
            .map(short_name)
            .or_else(|| {
                peer.get("HostName")
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            });
        if let (Some(ip), Some(name)) = (ip, name) {
            if !name.is_empty() {
                out.push((name, ip.to_string()));
            }
        }
    }
    out
}

/// `linux-desktop.tail1234.ts.net.` → `linux-desktop`.
fn short_name(dns: &str) -> String {
    dns.trim_end_matches('.')
        .split('.')
        .next()
        .unwrap_or(dns)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str, device: Option<&str>) -> SearchHit {
        SearchHit {
            conversation_id: id.into(),
            tool: "claude-code".into(),
            kind: "thread".into(),
            project: None,
            title: None,
            started_at: None,
            snippet: String::new(),
            score: 0.0,
            source_path: String::new(),
            bookmarked: false,
            pinned: false,
            tags: vec![],
            device: device.map(String::from),
        }
    }

    #[test]
    fn merges_and_dedups_across_devices() {
        // "shared" appears on both devices; "local"/"remote" on one each.
        let local = vec![hit("local", Some("mac")), hit("shared", Some("mac"))];
        let remote = vec![hit("shared", Some("linux")), hit("remote", Some("linux"))];

        let merged = rrf_merge(vec![local, remote], 10);

        // Three distinct conversations, the duplicate collapsed.
        let ids: Vec<&str> = merged.iter().map(|h| h.conversation_id.as_str()).collect();
        assert_eq!(merged.len(), 3);
        assert!(ids.contains(&"local") && ids.contains(&"remote") && ids.contains(&"shared"));

        // "shared" scored on both lists, so it ranks first.
        assert_eq!(merged[0].conversation_id, "shared");
        // The kept copy of the duplicate is the local-first one.
        assert_eq!(merged[0].device.as_deref(), Some("mac"));
    }

    #[test]
    fn respects_limit() {
        let a = vec![hit("a", None), hit("b", None), hit("c", None)];
        assert_eq!(rrf_merge(vec![a], 2).len(), 2);
    }

    #[test]
    fn add_remove_peer_roundtrips_to_disk() {
        let tmp = std::env::temp_dir().join(format!("ct-fed-{}.json", std::process::id()));
        let fed = Federation::new(
            "mac".into(),
            Some("t".into()),
            vec![],
            Duration::from_millis(1),
        )
        .with_config_path(tmp.clone());

        assert!(fed.add_peer(Peer {
            name: "linux".into(),
            addr: "100.1.2.3:47100".into(),
        }));
        // Re-adding the same name updates rather than duplicates.
        assert!(!fed.add_peer(Peer {
            name: "linux".into(),
            addr: "100.1.2.4:47100".into(),
        }));
        assert_eq!(fed.peers().len(), 1);
        assert_eq!(fed.peers()[0].addr, "100.1.2.4:47100");

        // It persisted, and reloads.
        let reloaded = PersistedConfig::load(&tmp);
        assert_eq!(reloaded.peers.len(), 1);
        assert_eq!(reloaded.device.as_deref(), Some("mac"));

        assert!(fed.remove_peer("linux"));
        assert!(fed.peers().is_empty());
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn pairing_code_round_trips() {
        let fed = Federation::new(
            "mac".into(),
            Some("sec".into()),
            vec![],
            Duration::from_millis(1),
        )
        .with_listen_addr(Some("100.1.2.3:47100".into()));
        let code = fed.pairing_code().expect("code with token + addr");
        let (token, peer) = decode_pairing(&code).expect("decodes");
        assert_eq!(token, "sec");
        assert_eq!(peer.name, "mac");
        assert_eq!(peer.addr, "100.1.2.3:47100");

        // No code without a reachable address, and garbage doesn't decode.
        let no_addr = Federation::new(
            "mac".into(),
            Some("s".into()),
            vec![],
            Duration::from_millis(1),
        );
        assert!(no_addr.pairing_code().is_none());
        assert!(decode_pairing("not-a-code").is_none());
    }

    #[test]
    fn serve_scope_excludes_tools_and_projects() {
        let fed = Federation::new("mac".into(), None, vec![], Duration::from_millis(1))
            .with_scope(vec!["cursor".into()], vec!["secret".into()]);
        assert!(fed.serves("claude-code", Some("/work/api")));
        assert!(!fed.serves("cursor", Some("/work/api"))); // excluded tool
        assert!(!fed.serves("claude-code", Some("/home/secret-proj"))); // excluded project substr
    }

    #[test]
    fn parses_tailscale_status_json() {
        let v = serde_json::json!({
            "Peer": {
                "key1": {
                    "DNSName": "linux-desktop.tail9876.ts.net.",
                    "HostName": "linux-desktop",
                    "TailscaleIPs": ["100.10.20.30", "fd7a::1"],
                    "Online": true
                },
                "key2": {
                    "HostName": "work-laptop",
                    "TailscaleIPs": ["100.40.50.60"]
                }
            }
        });
        let mut peers = parse_tailscale_peers(&v);
        peers.sort();
        assert_eq!(
            peers,
            vec![
                ("linux-desktop".to_string(), "100.10.20.30".to_string()),
                ("work-laptop".to_string(), "100.40.50.60".to_string()),
            ]
        );
    }

    #[test]
    fn reads_self_tailnet_ipv4() {
        let v = serde_json::json!({
            "Self": { "TailscaleIPs": ["100.99.1.2", "fd7a::99"] }
        });
        assert_eq!(self_tailnet_ip(&v), Some("100.99.1.2".to_string()));
        // No Self / no IPs → None (callers fall back to loopback).
        assert_eq!(self_tailnet_ip(&serde_json::json!({})), None);
        assert_eq!(
            self_tailnet_ip(&serde_json::json!({ "Self": { "TailscaleIPs": ["fd7a::1"] } })),
            None
        );
    }

    #[test]
    fn first_existing_picks_present_path() {
        assert_eq!(first_existing(&["/definitely/not/here/xyz"]), None);
        // temp_dir() exists on every platform, so it stands in for the bundle path.
        let tmp = std::env::temp_dir();
        let tmp = tmp.to_str().unwrap();
        assert_eq!(
            first_existing(&["/definitely/not/here", tmp]),
            Some(tmp.to_string())
        );
    }
}
