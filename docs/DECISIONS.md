# Crossthreads — Decision Log (ADRs)

Lightweight architecture decision records. Newest first. Each entry: the decision, why, and the consequences it commits us to.

| # | Decision | Status | Date |
|---|---|---|---|
| [ADR-010](#adr-010-cross-device-search--query-federation-over-a-private-tunnel) | Cross-device search = query federation over a private tunnel | Proposed | 2026-06-21 |
| [ADR-009](#adr-009-frontenddaemon-transport--httpjson-bridge) | Frontend↔daemon transport = HTTP/JSON bridge | Accepted | 2026-06-19 |
| [ADR-008](#adr-008-third-mvp-connector--aider) | 3rd MVP connector = Aider | Accepted | 2026-06-19 |
| [ADR-007](#adr-007-vector-store--sqlite-vec-in-the-same-db) | Vector store = `sqlite-vec` (one DB) | Accepted | 2026-06-19 |
| [ADR-006](#adr-006-ml-runtime--pure-rust-onnx) | ML runtime = pure-Rust ONNX | Accepted | 2026-06-19 |
| [ADR-005](#adr-005-process-model--background-daemon) | Process model = background daemon | Accepted | 2026-06-19 |
| [ADR-004](#adr-004-license--apache-20) | License = Apache-2.0 | Accepted | 2026-06-18 |
| [ADR-003](#adr-003-default-embeddings--bundled-onnx-ollama-optional) | Embeddings = bundled ONNX, Ollama optional | Accepted | 2026-06-18 |
| [ADR-002](#adr-002-primary-surface--desktop-first-tauri) | Primary surface = desktop-first (Tauri) | Accepted | 2026-06-18 |
| [ADR-001](#adr-001-engine-strategy--clean-build) | Engine strategy = clean build | Accepted | 2026-06-18 |

---

## ADR-001 — Engine strategy = clean build
**Decision.** Build our own Rust indexing/search engine. Study CASS and public extractors for connector/extraction *patterns*, but take no code or runtime dependency on them.

**Why.** Full control over the engine, schema, and on-disk format; no license entanglement; no coupling to another project's release cadence.

**Consequences.**
- We own connector maintenance and format-drift handling alone → mitigated by versioned connectors, a regression corpus (CI parses fixtures per tool/version), and a P2 plugin system to crowdsource connectors.
- Higher upfront cost than forking/interoperating — accepted as the price of control.

## ADR-002 — Primary surface = desktop-first (Tauri)
**Decision.** Lead the MVP with a Tauri desktop app as the primary surface. CLI + agent API ship alongside; TUI is demoted to fast-follow (P1.x).

**Why.** A polished GUI is the adoption wedge and supports later paid tiers; the underlying core is exercised by all surfaces equally.

**Consequences.**
- Need a frontend stack (React + Vite) and desktop concerns earlier: file-access permissions UX, code signing/notarization (deferred to launch), optional auto-update (off by default to honor no-telemetry).
- The engine must be headless and client-agnostic from day one (see ADR-005).

**Update (impl).** The frontend (`ui/`) is built and runs as a web app served by
the daemon's HTTP bridge. The native shell (`src-tauri/`) is scaffolded and
wraps the same UI via an `rpc` Tauri command (ADR-009), but is **excluded from
the Rust workspace** because Tauri needs platform webview libs + a display and
can't build headlessly — so it's intentionally unverified in CI and built on a
real dev machine.

## ADR-003 — Default embeddings = bundled ONNX, Ollama optional
**Decision.** Ship a zero-dependency local embedding model (all-MiniLM-class, ONNX). Ollama is an opt-in upgrade for richer models / `recall` synthesis.

**Why.** First-run activation must not require installing anything. Power users can opt up.

**Consequences.**
- Model weights are downloaded on first run (checksum-verified) to keep the installer small.
- All embedding/search works fully offline; any Ollama/cloud path is explicit opt-in (privacy posture intact).

## ADR-004 — License = Apache-2.0
**Decision.** Release the OSS core under Apache-2.0.

**Why.** Permissive plus an explicit patent grant — better for enterprise/team adoption and outside contributors than MIT, with negligible downside.

**Consequences.** `LICENSE` (Apache-2.0) at repo root; add SPDX headers as code lands; `NOTICE` file if/when we redistribute third-party assets.

## ADR-005 — Process model = background daemon
**Decision.** Ingestion, indexing, file-watching, the ONNX runtime, and the query engine live in a long-lived daemon (`crossthreadsd`). The desktop app, CLI, and MCP server are thin clients over loopback IPC/HTTP. The daemon is the **single writer** to the index.

**Why.** Desktop + CLI + MCP can all be live at once; a single-writer service avoids concurrent-write corruption and gives always-on file-watching independent of whether the GUI is open.

**Consequences.**
- Need lifecycle management: start-on-login/first-launch, supervision by the desktop app, health/status, graceful shutdown.
- Need a stable local API contract shared by all clients (aligns with [AGENT_API.md](AGENT_API.md)).
- Slightly more moving parts than an in-process MVP — accepted for correctness and the multi-surface story.

## ADR-006 — ML runtime = pure-Rust ONNX
**Decision.** Run embeddings (and any future reranker) via a pure-Rust ONNX runtime (`ort`/`candle`) in-process in the daemon. No Python sidecar.

**Why.** Preserves single-binary distribution and clean macOS signing/notarization; avoids shipping/managing a Python environment inside a desktop app.

**Consequences.**
- Constrained to models exportable to ONNX (fine for the planned encoder models).
- If a future feature truly needs the Python ML ecosystem, revisit as an opt-in component — not in the default install.

## ADR-007 — Vector store = `sqlite-vec` in the same DB
**Decision.** Store embeddings in the same SQLite database as metadata + FTS5, via `sqlite-vec`. No separate vector store (e.g. LanceDB) at MVP.

**Why.** One file to back up, sync, and purge; one transaction boundary; simplest operational model. Adequate for the target corpus (tens of thousands of messages).

**Consequences.**
- If vectors outgrow `sqlite-vec`'s performance envelope, revisit LanceDB/Postgres+pgvector (P2 scale path) behind the same query interface.

**Update (impl).** The Phase 0 implementation stores embeddings as raw f32 BLOBs
in the same SQLite DB and ranks with a **brute-force cosine scan** rather than
the `sqlite-vec` extension. This keeps the "one store" property and avoids
loadable-extension setup during the spike; it's correct and adequate for the
target corpus (tens of thousands of messages). `sqlite-vec` ANN remains the
drop-in optimization behind the same `search_semantic`/`search_hybrid` API once
linear scan becomes the bottleneck.

## ADR-008 — Third MVP connector = Aider
**Decision.** The MVP ships three connectors: Claude Code, Cursor, and **Aider**.

**Why.** Aider's chat history format is well-documented and a lower-risk parse than alternatives, getting us to three tools faster.

**Consequences.**
- Codex and others move to the fast-follow connector set (FR-ING-09).
- Revisit if Aider isn't in the maintainer's daily flow — swap target is cheap pre-implementation.

**Update (impl).** Implemented. Parses `.aider.chat.history.md` (sessions split
on the `# aider chat started at` marker; `#### ` user lines, unprefixed
assistant prose, `> ` notes skipped with a best-effort `Model:` sniff). Aider
has no central history registry, so discovery scans bounded roots
(`CROSSTHREADS_AIDER_ROOTS` overrides).

## ADR-009 — Frontend↔daemon transport = HTTP/JSON bridge
**Decision.** The UI talks to the daemon over an HTTP/JSON bridge (`POST
/api/rpc` with a `Request` body), not Tauri command IPC. The daemon also serves
the built static UI for all other paths.

**Why.** One transport serves both the native Tauri shell and a plain browser,
so the frontend is developable and verifiable without the native webview
toolchain. It reuses the existing protocol `Request`/`Response` types, so there's
one API for CLI, MCP, and UI. Resolves the open "transport" question.

**Consequences.**
- A lightweight HTTP server (`tiny_http`, no async runtime) lives in `ct-daemon`
  alongside the raw TCP protocol; the two run on separate ports.
- The Tauri wrapper becomes thin (point a webview at the local HTTP server, or
  call the same JSON API from Rust commands).
- CORS/auth stay trivial because it's loopback-only; revisit if ever exposed.

## ADR-010 — Cross-device search = query federation over a private tunnel
**Decision.** To search history that lives on your other machines, **federate
the query** rather than replicate indexes: each device keeps only its own index;
a search runs locally and is forwarded to peer daemons, which search their own
index and return matches that the querying device merges. Transport and device
identity are delegated to a **private mesh tunnel (Tailscale/WireGuard)** the
user runs on each device; the daemon binds a peer listener to the tailnet
interface and authenticates peers with a shared token. Full design in
[CROSS_DEVICE_SEARCH.md](CROSS_DEVICE_SEARCH.md).

**Why.** Federation reuses the existing `Request`/`Response` protocol, RRF merge,
and content-hash dedup, and keeps each device the single source of truth (offline
peers degrade gracefully, no sync conflicts). It preserves the local-first /
no-account / no-telemetry posture — no central server and no bulk copy of history
anywhere new. Tailscale solves encryption, stable addressing, and NAT traversal
so we don't build (and own the security of) a mesh.

**Consequences.**
- The daemon gains a federation layer: bind to the tailnet interface (in addition
  to loopback), a static `peers` list + shared token, parallel fan-out with a
  bounded per-peer timeout, and a `local_only` flag on forwarded `Search`
  requests to prevent query loops.
- Results carry a `device` tag; cross-device duplicates collapse by
  `content_hash`. No index/schema change — it's purely a query-time layer.
- New trust boundary: peers return matching content to the querying device over
  the user's own encrypted tailnet — documented explicitly, with snippet-first
  fetch and per-peer scope filters as opt-in tighteners.
- Tailscale is the recommended default, not a hard dependency: a raw `host:port`
  peer list still works on a LAN (user owns NAT/firewall).
- Status: **Proposed** — not yet implemented; phased MVP→P2 in the design doc.

---

## Still open
- Resume deep-link depth per tool (research; [AGENT_API.md](AGENT_API.md) §6).
- Schema versioning & migration strategy.
- Team-sync trust model (P2). _(Single-user cross-device search is now specified
  — see [ADR-010](#adr-010-cross-device-search--query-federation-over-a-private-tunnel)
  / [CROSS_DEVICE_SEARCH.md](CROSS_DEVICE_SEARCH.md); multi-user team sharing is a
  separate, later question.)_
- Early-P1 details: cross-encoder reranker timing, exact embedding model + chunk sizing, frontend↔daemon transport.
