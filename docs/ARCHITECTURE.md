# Crossthreads — Reference Architecture

| | |
|---|---|
| **Status** | v0.2 — §2.8 covers what was added since 0.16 (threads, live state, rooms, devices, phone) |
| **Last updated** | 2026-11-16 |
| **Companion to** | [PRD.md](PRD.md) · [REQUIREMENTS.md](REQUIREMENTS.md) |

This is a *reference* architecture to anchor estimation and the MVP build. Core stack choices are now **decided** (see §3); a few items remain flagged as open.

---

## 1. System overview

Crossthreads runs as a **persistent local daemon** (`crossthreadsd`) that owns ingestion, indexing, and query. The desktop app, CLI, and MCP server are **thin clients** of the daemon over a local IPC/HTTP API on loopback.

```
   source tools                 ┌──────────── crossthreadsd (daemon) ────────────┐
 ┌─────────────┐  fs watch      │  ┌────────────┐   ┌──────────────┐             │
 │ Claude Code │───────────────▶│  │ Connectors │──▶│ Normalizer + │             │
 │  (JSONL)    │                │  │ (per-tool) │   │  Chunker     │             │
 ├─────────────┤                │  └────────────┘   └──────┬───────┘             │
 │   Cursor    │───────────────▶│        ▲                  │                     │
 │ (state.vscdb)│               │        │ versioned        ▼                     │
 ├─────────────┤                │  ┌─────┴──────┐   ┌────────────────────────┐    │
 │   Aider     │───────────────▶│  │ Auto-detect│   │  Index (single SQLite) │    │
 │  ...        │                │  │  + dedup   │   │  FTS5  +  sqlite-vec    │    │
 └─────────────┘                │  └────────────┘   └───────────┬────────────┘    │
                                │                               ▼                  │
                                │   ┌──────────────┐   ┌────────────────────────┐ │
                                │   │ ONNX runtime │──▶│  Query engine          │ │
                                │   │ (ort/candle) │   │  hybrid + fuse(RRF) +  │ │
                                │   └──────────────┘   │  optional rerank       │ │
                                │                      └───────────┬────────────┘ │
                                │              local IPC / HTTP (loopback)        │
                                └───────────────────────────┬─────────────────────┘
                                       ▼            ▼        ▼            ▼
                                   ┌────────┐   ┌────────┐ ┌──────────┐ ┌────────┐
                                   │Desktop │   │ CLI /  │ │  MCP     │ │  TUI   │
                                   │(Tauri) │   │ JSON   │ │  server  │ │ (P1.x) │
                                   └────────┘   └────────┘ └──────────┘ └────────┘
                                    primary                  agents
```

## 2. Components

### 2.0 Daemon (`crossthreadsd`)
- Long-lived background process; the **only** writer to the index. Started on login (or on first client launch) and supervised by the desktop app.
- Owns file-watchers, the index, the ONNX runtime, and the query engine; serves all clients over loopback IPC/HTTP (read paths) and accepts control commands (re-index, purge, status).
- A single-writer daemon removes the concurrent-write problem a multi-process in-process design would have (desktop + CLI + MCP all touching one DB).

### 2.1 Connectors (Ingestion)
- One module per source tool, each implementing a common `Connector` interface: `detect()`, `discover_sessions()`, `parse(session) -> RawConversation`, plus a declared `format_version`.
- **Clean build:** connectors are our own code; we port *extraction patterns* (not code) from CASS/public extractors.
- **Auto-detect** probes known per-OS paths; **file watchers** (run inside the daemon) trigger incremental parsing on change.
- **Resilience:** a format fingerprint guards against silent breakage on tool updates; parse failures are isolated and logged (FR-ING-07/08).

### 2.2 Normalizer + Chunker
- Maps `RawConversation` → common schema (`Conversation`, `Message`) with provenance back to source file/offset.
- **Chunking** by message turn / topic segment / code-change boundary, preserving chunk→message→conversation lineage for retrieval and "open original."
- **Dedup** via content hashing before write.

### 2.3 Index layer
- **One SQLite database** is the system of record: metadata + **FTS5** (BM25-class lexical) + **`sqlite-vec`** for embeddings — a single file to back up, sync, or purge.
- Embeddings produced **in-process by the ONNX runtime** (`ort`/`candle`, all-MiniLM-class); no network required.
- **Optional Postgres** backend for scale/teams (P2).

### 2.4 Query engine
- **Hybrid retrieval:** run lexical (FTS5) + vector (`sqlite-vec`), fuse with **reciprocal-rank fusion (RRF)** at MVP; **cross-encoder rerank** is a post-MVP enhancement (one extra ONNX model).
- **NL queries:** optional small local LLM (Ollama, opt-in) rewrites the query / extracts filters; degrades cleanly to hybrid search when no LLM is configured. `recall` synthesis is **retrieval-only at MVP** unless an LLM is configured.
- Applies **filters** (tool, project, date, model) and returns ranked results with snippets/highlights/provenance.

### 2.5 Surfaces (daemon clients)
- **Desktop (Tauri)** — primary MVP surface; React + Vite + shadcn/ui, talking to the daemon.
- **CLI / JSON** — scriptable; structured output for all core ops.
- **Agent API / MCP** — `search()` / `recall_decision()` for agents.
- **TUI** — keyboard surface over the same daemon (P1.x).

### 2.6 Actions
- Open original (deep-link/reveal), export markdown/HTML, copy-as-prompt / inject, resume/handoff.

### 2.8 Since 0.16: threads, live state, rooms, devices, phone

The product grew from a session indexer into a thread layer ([SPEC.md](SPEC.md)).
Everything below lives in the same daemon and the same SQLite file.

| Piece | Where | How it works |
|---|---|---|
| **Repository key** | `ct-core::repo` | The normalised `origin` remote, else `root:<first commit>`, else `path:`. Read from `.git/config` (worktrees through `commondir`), credentials stripped. The same on every machine and worktree. |
| **Threads** | `ct-store::threads`, `ct-index::link_threads` | Durable tables (`threads`, `thread_sessions`, `links`, `link_rejections`) keyed by the stable session key, so a rebuild keeps them. Detection is deterministic: copied content links sessions; a cross-tool session within 30 minutes is only suggested. |
| **Live state** | `ct-daemon::live` | In memory. Hooks (`crossthreads hook <tool> <event>`) report running / needs you / idle / ended; without hooks, a transcript written in the last 45 s is "running (inferred)". |
| **Rooms** | `ct-store::rooms`, `ct-daemon::rooms` | Messages, read cursors and claims in SQLite; waiting uses a condition variable, never the store lock. Keyed by repository. A closed exchange is indexed as kind `exchange` and links the sessions `conversed`. Stop and PreToolUse hooks wake agents and guard claims. |
| **Federation** | `ct-daemon::federation` | Approved peers over Tailscale with a shared token (OS keychain). Search and the board fan out on demand and merge; transcripts are fetched on open. Other machines may call only the token-checked peer ops. |
| **Rooms across devices** | `ct-daemon::rooms` | Each device keeps its own copy. New messages are pushed to peers with the *Can message* grant; every 10 s each device pulls what it missed. Messages carry `(device, origin_seq)` and are stored once. |
| **Phone** | `ct-daemon::phone`, `http.rs` | A second HTTP listener on the tailnet address only. Pairing trades a one-time code for a random token in an `HttpOnly`, `SameSite=Strict` cookie (hash stored in `phone.json`); a fixed allowlist of reading ops, posting per-phone. |

Who may call what is listed in [AGENT_API.md §7.8](AGENT_API.md#78-who-may-call-what).

## 3. Tech stack

| Layer | Choice | Rationale | Status |
|---|---|---|---|
| Core / indexing | **Rust** | Performance + single-binary distribution | Decided |
| Engine strategy | **Clean build, port patterns** | Own engine; study CASS/public-extractor patterns, no code/runtime dependency | Decided |
| Process model | **Background daemon (`crossthreadsd`)** | Single-writer index; desktop/CLI/MCP are thin clients | Decided |
| ML runtime | **Pure-Rust ONNX (`ort`/`candle`)** | Single binary, clean signing/distribution; no Python sidecar | Decided |
| Lexical store | SQLite + FTS5 | Ubiquitous, embeddable, BM25 | Decided |
| Vector store | **`sqlite-vec` (same DB)** | One store/file; no second backend to manage | Decided |
| Embeddings | **Bundled ONNX (all-MiniLM-class), Ollama optional** | Zero-dependency local default; Ollama opt-in upgrade. Weights downloaded on first run (checksum-verified) to keep installer small | Decided |
| Reranking | RRF fusion at MVP; cross-encoder later | Avoid a second model in the MVP bundle | Proposed |
| Desktop (primary surface) | **Tauri + React + Vite + shadcn/ui** | Polished GUI as the adoption wedge; SPA (no SSR) talking to the daemon | Decided |
| Frontend↔core | Tauri commands → daemon IPC/HTTP | One API for all clients | Proposed |
| Workspace layout | `ct-core` (lib) · `ct-daemon` (bin) · `ct-cli` (bin) · `ct-desktop` (Tauri) · `ct-mcp` (bin) | Clean-build structure over one core | Proposed |
| TUI | `ratatui`-class | Keyboard surface over same daemon | Fast-follow |
| File watching | `notify`-class watcher (in daemon) | Near real-time incremental | Decided |
| License | **Apache-2.0** | Permissive + patent grant | Decided |
| 3rd MVP connector | **Aider** | Well-documented history format; lower-risk parse | Decided |

> **Remaining open:** cross-encoder reranker timing, exact embedding model + chunk sizing, and the frontend↔daemon transport detail (Tauri command shim vs. direct loopback HTTP). All low-risk, settled early in P1.

## 4. Data model (sketch)

```
Conversation
  id            (stable, content-derived)
  tool          (claude-code | cursor | codex | aider | ...)
  project       (workspace/repo path)
  model         (e.g. claude-opus-4-8)
  started_at    / ended_at
  source_path   / source_fingerprint
  git_context   (branch, commit) — optional
  content_hash

Message
  id
  conversation_id
  role          (user | assistant | tool | system)
  content
  timestamp
  code_snippets[]   (language, text, linked_file?)
  tool_calls[]      (name, args, result_ref)
  metadata

Chunk
  id
  message_id / conversation_id   (lineage)
  text
  embedding_ref
  chunk_type    (turn | topic | code_change)
```

## 5. Privacy & security posture

- **Local-only default**, **no telemetry** (NFR-PRIV-01/02). Any network call (cloud embeddings, sync) is opt-in and visible.
- **Purge** command removes all indexed data and derived artifacts (NFR-PRIV-03).
- **Sync (P2)** is end-to-end encrypted with user-controlled keys (NFR-SEC-01).

## 6. Key cross-cutting concerns

- **Connector versioning & regression corpus:** keep sample sessions per tool/version as fixtures; CI parses them to catch format drift (mitigates the top risk in the PRD).
- **Schema migrations:** versioned; prefer in-place migration over full rebuild; always recoverable from source files.
- **Performance budget:** background/throttled indexing; warm search < 500 ms (NFR-PERF-01).
- **Extensibility:** connectors and embedding models behind stable interfaces (NFR-MAINT-01) → enables the plugin system (P2).

## 7. Phased technical sequencing

See [ROADMAP.md](ROADMAP.md). In short: prototype connectors + daemon + single-DB index in a Tauri shell → hybrid search + desktop app + CLI/agent API (MVP) → TUI/MCP/analytics (fast-follow) → memory layers, outcome tracking, sync (P2).
