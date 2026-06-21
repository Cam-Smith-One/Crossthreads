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
    /// This device's name, stamped on the hits it serves to peers.
    pub device: String,
    /// Shared secret. When set, incoming `PeerSearch` must present a match;
    /// when `None`, the tailnet itself is the access boundary.
    pub token: Option<String>,
    /// Per-peer connect/read budget; a slow or offline peer is skipped after
    /// this, never hung on.
    pub timeout: Duration,
    /// Approved peers to fan a local search out to. Behind a lock so the
    /// Settings → Devices panel can add/remove them at runtime.
    peers: Mutex<Vec<Peer>>,
    /// Where the approved-peer list is persisted, if anywhere.
    config_path: Option<PathBuf>,
}

impl Federation {
    /// Build federation config with a fixed peer set and no persistence.
    pub fn new(device: String, token: Option<String>, peers: Vec<Peer>, timeout: Duration) -> Self {
        Self {
            device,
            token,
            timeout,
            peers: Mutex::new(peers),
            config_path: None,
        }
    }

    /// Persist approve/remove changes to `path` (rewritten as JSON).
    pub fn with_config_path(mut self, path: PathBuf) -> Self {
        self.config_path = Some(path);
        self
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

    /// Write the current device/token/peers to the config file (best effort).
    fn save(&self) {
        let Some(path) = &self.config_path else {
            return;
        };
        let cfg = PersistedConfig {
            device: Some(self.device.clone()),
            token: self.token.clone(),
            peers: self.peers(),
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

/// On-disk federation config (`federation.json`). Merged with CLI/env on start.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PersistedConfig {
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub peers: Vec<Peer>,
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

/// Run `tailscale status --json` and extract `(name, tailnet-ip)` per peer.
fn tailscale_peers() -> Vec<(String, String)> {
    let Ok(out) = Command::new("tailscale")
        .arg("status")
        .arg("--json")
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    serde_json::from_slice::<Value>(&out.stdout)
        .map(|v| parse_tailscale_peers(&v))
        .unwrap_or_default()
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
}
