# Design — Cross-device search (federated, local-first)

_Status: Proposed · 2026-06-21 · see [ADR-010](DECISIONS.md#adr-010-cross-device-search--query-federation-over-a-private-tunnel)_

## Problem

Each device keeps its own Crossthreads index. Today that index is an island:
if you solved something in Cursor on your Linux desktop, you can't find it from
Codex on your Mac. As people spread work across machines, "everything is
isolated to the device in front of me" becomes the main friction.

**Goal:** from one device, search the history that lives on your *other*
devices, and get one merged result list — without giving up the local-first,
no-telemetry, no-account posture.

> I'm in Codex on my Mac. I search for the thread where I fixed the OAuth retry,
> and it surfaces a match from my Linux desktop running Cursor.

## Approach: federate the query, don't sync the data

Two ways to make device A's search see device B's history:

1. **Federation (chosen).** Every device keeps only its own index. A search on
   A runs locally *and* is forwarded to peer daemons; each peer searches its own
   index and returns matches; A merges them. Only the query leaves, and only
   matching results come back.
2. **Replication (rejected).** Sync every device's index to every other device
   so each holds a full copy. Requires conflict resolution, storage
   duplication, divergent-state handling, and puts every device's raw content on
   every machine.

Federation is both **simpler** (reuses the existing request/response protocol,
RRF merge, and content-hash dedup) and **more robust** (each device stays the
sole source of truth for its own data; an offline peer is just skipped — the
same graceful-degradation pattern the connectors already use). It also keeps
the privacy story clean: there is no central server and no bulk copy of your
history sitting anywhere new.

## Transport & security: a private mesh tunnel (Tailscale/WireGuard)

The hard parts of "securely reach my other PCs" are encryption, device
identity, and NAT traversal (devices on different networks). Rather than build a
mesh, **delegate transport to [Tailscale](https://tailscale.com) (WireGuard)**,
which the user installs on each device:

- **Encryption** — WireGuard end-to-end; no crypto for us to get wrong.
- **Identity + addressing** — stable MagicDNS names (`mac-mini`,
  `linux-desktop`) instead of brittle IPs.
- **NAT traversal** — works across home/office networks, not just one LAN.
- **Access control** — only your own devices (your tailnet) can connect; ACLs
  optionally narrow it to the daemon port.

The daemon binds its **peer listener to the tailnet interface only** (never
`0.0.0.0`) and dials peers by MagicDNS name. We add an **app-level shared
token** on top as defence-in-depth, so a peer request is rejected unless it
carries the token even if something else is listening on the tailnet.

LAN-only / no-Tailscale users can still list raw `host:port` peers; they just
own NAT/firewall themselves. Tailscale is the recommended default, not a hard
dependency.

## What changes in the daemon

The daemon already exposes a `Request`/`Response` protocol over loopback TCP
(ADR-005/009). Federation adds a thin layer around `Search`:

1. **Config** (`crossthreads.toml` / env):
   ```toml
   [federation]
   listen = "100.x.y.z:47100"   # tailnet interface (bind here, in addition to loopback)
   token  = "…shared secret…"   # required on every peer request
   peers  = ["mac-mini:47100", "linux-desktop:47100"]
   timeout_ms = 1500
   ```
2. **Fan-out.** On a `Search`, the daemon runs the local query and, in
   parallel, sends the *same* request to each peer — but with a new
   `local_only: true` flag set so peers **do not** re-fan-out (prevents query
   loops/amplification). Each peer call has a bounded timeout; a peer that is
   offline, slow, or rejects the token is skipped, and the search still returns
   local + reachable-peer results.
3. **Merge.** Reuse the existing hybrid/RRF scoring to interleave local and
   remote hits, then **dedup by `content_hash`** — a nice bonus: the same repo
   cloned on two machines, or a thread that exists identically on both, collapses
   to a single result. Each surviving result is tagged with its origin
   `device` (the peer's configured name; local = this host).
4. **Cross-device fetch.** `GetConversation` and `build_context` already return
   content in the response, so a remote hit can be previewed and injected as-is.
   `Search` responses carry enough (snippet + ids + `device`) to render the list;
   the full transcript is fetched on demand from the owning peer.

No schema/index changes are required — federation is purely a daemon-to-daemon
query layer over the indexes that already exist.

## UX

- Results show a device chip: `📁 linux-desktop · Cursor · 2026-05-20`.
- "Open original" becomes **"Reveal on `<device>`"** for remote hits (the file
  lives on that machine); local hits keep the existing reveal-in-file-manager
  action. `build_context`/recall work uniformly across devices.
- A status indicator lists reachable vs unreachable peers so a missing device is
  visible, not silent.
- MCP: the agent tools (`crossthreads_search` / `recall` / `build_context`)
  transparently span devices once federation is enabled — no new tools.

## Trust boundary (be explicit)

To return useful results, a peer sends matching snippets/content back in its
response. That content travels **only** between your own devices over your own
encrypted tailnet — no third party, no telemetry — but the docs must state
plainly: enabling federation means you trust your tailnet and your peer
daemons. Mitigations available as options:

- **Snippet-first:** peers return metadata + a short snippet by default; the
  full transcript is fetched only when you open a result.
- **Scope filters:** allow a peer to restrict what it will serve (e.g. exclude a
  tool or a project) so a shared/work machine can expose less.
- The shared token + tailnet ACLs gate *who* can query at all.

## Failure modes & robustness

| Situation | Behaviour |
|---|---|
| Peer offline | Skipped after `timeout_ms`; local + other peers still return. |
| Peer slow | Bounded per-peer timeout; partial results, never a hang. |
| Bad/missing token | Peer rejects; querying device logs and skips it. |
| Query loop | Prevented — peer requests carry `local_only`, so no re-fan-out. |
| Duplicate result across devices | Collapsed by `content_hash`; keeps highest-ranked, lists the others' devices. |
| Tailnet down | Federation degrades to local-only search; nothing breaks. |

## Phasing

- **MVP (P-fed.0):** loopback + tailnet bind, static `peers` list, shared token,
  parallel fan-out with timeout, RRF merge + dedup, `device` tag in results,
  `local_only` loop guard. CLI/`--peers` flag to try it before any UI.
- **P-fed.1:** UI device chips + "reveal on device", reachable-peers status,
  snippet-first fetch.
- **P-fed.2:** scope filters per peer; optional Tailscale-API auto-discovery of
  peers; encrypted at-rest token storage.

## Alternatives considered

- **Central sync server / hosted index.** Rejected — breaks local-first and
  no-account, and creates exactly the data-aggregation point the project avoids.
- **CRDT index replication.** Rejected for now — high complexity (conflict
  resolution, storage blow-up, full content on every device) for a problem
  federation solves without copying data. Could revisit if true offline access
  to *other* devices' history (search a device that's currently off) becomes a
  hard requirement.
- **Build our own libp2p/QUIC mesh + hole-punching.** Rejected — re-implements
  what Tailscale/WireGuard already does well; months of work and a security
  surface we'd own. Keep a raw `host:port` peer option for LAN, but don't build
  the mesh.

## Open questions

- Default for offline peers: skip silently vs. surface "N devices unreachable"
  prominently (leaning: surface, so results never look complete when they aren't).
- Token bootstrap UX — copy/paste a token between devices vs. a short pairing
  code; keep it out of telemetry/logs either way.
- Should `recall` synthesis run on the querying device over merged hits, or ask
  each peer to pre-summarize? (Leaning: merge raw hits locally, synthesize once.)
