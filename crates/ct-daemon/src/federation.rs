//! Cross-device query federation (ADR-010, design in CROSS_DEVICE_SEARCH.md).
//!
//! Each device keeps only its own index. When federation is configured, a
//! `Search` runs locally *and* is fanned out to peer daemons over a private
//! tunnel (Tailscale/WireGuard); each peer answers a `PeerSearch` from its own
//! index, and the querying device merges everything into one ranked list. Only
//! the query leaves and only matching results come back — no index sync, no
//! central server.
//!
//! This module holds the configuration types and the rank-based merge; the
//! daemon ([`crate::Daemon`]) owns the fan-out and the `PeerSearch` handler.

use std::collections::HashMap;
use std::time::Duration;

use ct_store::SearchHit;

/// A reachable peer device: a human name (shown as the result's device chip)
/// and the address its daemon listens on (a tailnet `host:port` or MagicDNS
/// name, e.g. `linux-desktop:47100`).
#[derive(Debug, Clone)]
pub struct Peer {
    pub name: String,
    pub addr: String,
}

/// Federation settings for this daemon. Present only when the user opts in
/// (via `--peer` / `--device-name` / `CROSSTHREADS_PEERS`); absent means the
/// daemon is purely local and refuses `PeerSearch`.
#[derive(Debug, Clone)]
pub struct Federation {
    /// This device's name, stamped on the hits it serves to peers.
    pub device: String,
    /// Shared secret. When set, incoming `PeerSearch` must present a match;
    /// when `None`, the tailnet itself is the access boundary.
    pub token: Option<String>,
    /// Devices to fan a local search out to. May be empty (this daemon is only
    /// a searchable peer, not a querier).
    pub peers: Vec<Peer>,
    /// Per-peer connect/read budget; a slow or offline peer is skipped after
    /// this, never hung on.
    pub timeout: Duration,
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
}
