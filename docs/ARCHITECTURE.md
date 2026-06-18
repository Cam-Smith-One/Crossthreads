# Crossthreads — Reference Architecture

| | |
|---|---|
| **Status** | Draft v0.1 |
| **Last updated** | 2026-06-18 |
| **Companion to** | [PRD.md](PRD.md) · [REQUIREMENTS.md](REQUIREMENTS.md) |

This is a *reference* architecture to anchor estimation and the MVP build. Concrete stack choices are flagged as decisions/open questions, not commitments.

---

## 1. System overview

```
                         ┌──────────────────────────────────────────────┐
                         │                 Crossthreads                  │
                         │                                                │
 ┌─────────────┐  watch  │  ┌────────────┐   ┌──────────────┐            │
 │ Claude Code │────────▶│  │ Connectors │──▶│ Normalizer + │            │
 │  (JSONL)    │         │  │ (per-tool) │   │  Chunker     │            │
 ├─────────────┤         │  └────────────┘   └──────┬───────┘            │
 │   Cursor    │────────▶│        ▲                  │                    │
 │ (state.vscdb)│        │        │ versioned        ▼                    │
 ├─────────────┤         │  ┌─────┴──────┐   ┌───────────────────────┐   │
 │ Codex/Aider │────────▶│  │ Auto-detect│   │  Index layer          │   │
 │  ...        │         │  │  + dedup   │   │  ┌─────────┐ ┌───────┐ │   │
 └─────────────┘         │  └────────────┘   │  │SQLite+  │ │Vector │ │   │
                         │                   │  │  FTS    │ │ store │ │   │
                         │                   │  └─────────┘ └───────┘ │   │
                         │                   └──────────┬────────────┘   │
                         │                              ▼                 │
                         │                   ┌───────────────────────┐   │
                         │                   │  Query engine         │   │
                         │                   │  hybrid + rerank +    │   │
                         │                   │  NL rewrite           │   │
                         │                   └──────────┬────────────┘   │
                         │       ┌──────────────────────┼─────────────┐  │
                         │       ▼            ▼          ▼             ▼  │
                         │   ┌──────┐    ┌────────┐  ┌──────────┐ ┌────────┐
                         │   │ TUI  │    │ CLI/   │  │ Desktop  │ │  MCP / │
                         │   │      │    │ JSON   │  │ (Tauri)  │ │ Agent  │
                         │   └──────┘    └────────┘  └──────────┘ │  API   │
                         │                                         └────────┘
                         └──────────────────────────────────────────────┘
```

## 2. Components

### 2.1 Connectors (Ingestion)
- One module per source tool, each implementing a common `Connector` interface: `detect()`, `discover_sessions()`, `parse(session) -> RawConversation`, plus a declared `format_version`.
- **Auto-detect** probes known per-OS paths; **file watchers** trigger incremental parsing on change.
- **Resilience:** a format fingerprint guards against silent breakage on tool updates; parse failures are isolated and logged (FR-ING-07/08).

### 2.2 Normalizer + Chunker
- Maps `RawConversation` → common schema (`Conversation`, `Message`) with provenance back to source file/offset.
- **Chunking** by message turn / topic segment / code-change boundary, preserving chunk→message→conversation lineage for retrieval and "open original."
- **Dedup** via content hashing before write.

### 2.3 Index layer
- **SQLite + FTS** = system of record for metadata + full text (BM25-class lexical).
- **Vector store** (LanceDB/Chroma-class) for embeddings; embeddings produced by a **local model** (ONNX all-MiniLM-class or Ollama), no network required.
- **Optional Postgres** backend for scale/teams (P2).

### 2.4 Query engine
- **Hybrid retrieval:** run lexical + vector, fuse (e.g. reciprocal-rank fusion), then **rerank** top candidates.
- **NL queries:** optional small local LLM rewrites the query / extracts filters; degrades cleanly to hybrid search when no LLM is configured.
- Applies **filters** (tool, project, date, model) and returns ranked results with snippets/highlights/provenance.

### 2.5 Surfaces
- **TUI** — primary MVP surface, keyboard-first.
- **CLI / JSON** — scriptable; structured output for all core ops.
- **Agent API / MCP** — `search()` / `recall_decision()` for agents.
- **Desktop (Tauri)** — fast-follow GUI reusing the same core.

### 2.6 Actions
- Open original (deep-link/reveal), export markdown/HTML, copy-as-prompt / inject, resume/handoff.

## 3. Tech stack (proposed)

| Layer | Proposal | Rationale | Status |
|---|---|---|---|
| Core / indexing | **Rust** | Performance + single-binary distribution; aligns with CASS-style tooling | Proposed |
| ML-heavy parts | Rust ONNX runtime, or a Python sidecar if it accelerates delivery | Pragmatic; Python has the richer ML ecosystem | Open Q |
| Lexical store | SQLite + FTS5 | Ubiquitous, embeddable, BM25 | Proposed |
| Vector store | LanceDB (embedded) | Local-first, no server | Proposed |
| Embeddings | Bundled ONNX (all-MiniLM-class) w/ optional Ollama | Local, private, swappable | Proposed |
| Desktop | Tauri + React/Next + shadcn/ui | Polished UI, small footprint | Fast-follow |
| File watching | `notify`-class watcher | Near real-time incremental | Proposed |

> **Build-on-CASS vs. clean build** (PRD Open Q1) is the biggest fork in the road and should be resolved during the Research & Prototype phase.

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

See [ROADMAP.md](ROADMAP.md). In short: prototype connectors + SQLite index → hybrid search + TUI + agent API (MVP) → desktop/MCP/analytics (fast-follow) → memory layers, outcome tracking, sync (P2).
