# Crossthreads

[![CI](https://github.com/Cam-Smith-One/Crossthreads/actions/workflows/ci.yml/badge.svg)](https://github.com/Cam-Smith-One/Crossthreads/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Local-first](https://img.shields.io/badge/local--first-no%20telemetry-2ea44f.svg)](#principles)

**One place to search, recall, and resume every AI coding conversation — across Claude Code, Codex, Cursor, Aider, and more.**

Crossthreads is a local-first session indexer and memory layer for AI coding tools. It auto-discovers conversation history wherever your agents store it, normalizes everything into a common schema, and gives you fast hybrid search (keyword + semantic) over the whole corpus — plus actionable outputs like "resume this thread," "export context," and "inject into a new agent prompt." A background daemon keeps the index live, and an MCP server lets your agents query it natively.

Built for developers who switch between coding agents daily and are tired of fragmented, un-searchable session memory.

## Why

The built-in history/search in each tool is siloed and weak. When you ask "where's the thread where we implemented the auth retry logic?", you have no way to answer it across tools. Crossthreads makes that a one-line query.

## Quickstart

Prereqs: **Rust** (stable); **Node** for the web UI. SQLite and the ONNX runtime are bundled; the embedding model downloads once on first use.

```sh
# CLI: index your sessions and search them
cargo run --release -p ct-cli -- index
cargo run --release -p ct-cli -- search "oauth refresh retry" --mode hybrid

# Full app: daemon + web UI (auto-indexes, watches, serves the UI)
cd ui && npm install && npm run build && cd ..
cargo run --release --features onnx -p ct-daemon -- --http 127.0.0.1:47101 --ui ui/dist
#   → open http://127.0.0.1:47101

# Or bring up a sample 4-tool corpus + UI in one command:
CT_ONNX=1 scripts/demo.sh
```

Add `--features onnx` for real semantic search (all-MiniLM via ONNX); without it, a deterministic offline embedder is used. For agents, point an MCP client at the `ct-mcp` binary — see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Status

🛠️ **MVP backend complete.** Sessions from **Claude Code** (JSONL), **Cursor** (`state.vscdb` SQLite), **Aider** (`.aider.chat.history.md`), and **Codex** (`~/.codex/sessions` rollouts) are parsed, normalized, **persisted into one SQLite index** (deduped by content hash), and searchable with **hybrid retrieval** — FTS5 keyword (BM25) **+ semantic embeddings (all-MiniLM via ONNX)** fused with Reciprocal Rank Fusion, with tool/project/date **filters**. A background daemon (`crossthreadsd`) owns the index, **watches for new sessions and re-indexes automatically**, and serves search/status/context over a loopback socket **and** an HTTP bridge. An **MCP server** lets agents search/recall/inject context natively, a **React+Vite UI** provides search + filters + a conversation viewer, and a **Tauri shell** wraps that UI (builds on a desktop machine).

```
crates/
  ct-core         # normalized schema + Connector trait + content hashing
  ct-connectors   # parsers: Claude Code, Cursor, Aider, Codex (+ regression corpus)
  ct-embed        # Embedder trait: hash (default) + ONNX/all-MiniLM (`onnx`)
  ct-store        # one SQLite index: FTS5 + vectors + RRF hybrid + filters (ADR-007)
  ct-index        # indexing orchestration shared by CLI + daemon
  ct-daemon       # `crossthreadsd` single-writer daemon + loopback/HTTP API + watcher
  ct-cli          # `crossthreads` CLI (index / search / context / status; --remote)
  ct-mcp          # MCP server: agents query your history natively (FR-UI-04)
ui/               # React + Vite frontend (web + Tauri), talks to the daemon
src-tauri/        # native desktop shell (Tauri; excluded from the CI workspace)
```

## Documentation

Full docs live in [`docs/`](docs/README.md):

| | |
|---|---|
| [DEVELOPMENT](docs/DEVELOPMENT.md) | Build, test, run the daemon/UI/MCP; the `onnx` feature; crate layout |
| [ARCHITECTURE](docs/ARCHITECTURE.md) | Components, data model, daemon process model, privacy posture |
| [AGENT_API](docs/AGENT_API.md) | CLI/JSON, MCP, and HTTP interface (search / recall / build_context) |
| [PRD](docs/PRD.md) · [REQUIREMENTS](docs/REQUIREMENTS.md) · [ROADMAP](docs/ROADMAP.md) | Product intent, requirements, status |
| [DECISIONS](docs/DECISIONS.md) | Architecture decision records (ADRs) |

## Testing & development

```sh
scripts/check.sh   # everything CI runs: fmt + clippy + tests + UI build
scripts/demo.sh    # bring up the full stack against a sample corpus
```

## Contributing

Contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) and our
[Code of Conduct](CODE_OF_CONDUCT.md). Security issues: please report privately
per [SECURITY.md](SECURITY.md). Licensed under [Apache-2.0](LICENSE).

> **Status:** pre-release. The MVP backend (connectors, search, daemon, MCP, web
> UI) is implemented and tested; the native desktop app is scaffolded. Not yet
> publicly announced.

## Principles

1. **Local-first & private by default** — no telemetry, your data never leaves your machine unless you opt in.
2. **Better than grep** — hybrid lexical + semantic retrieval with reranking and NL query understanding.
3. **Actionable, not just searchable** — every result can be opened, exported, resumed, or injected.
4. **Resilient connectors** — tool formats change; detection is versioned and community-extensible.
5. **Agent-friendly** — agents can query Crossthreads via CLI/JSON and MCP tools.
