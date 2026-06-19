# Crossthreads

**One place to search, recall, and resume every AI coding conversation — across Claude Code, Codex, Cursor, Aider, Gemini CLI, and more.**

Crossthreads is a local-first session indexer and memory layer for AI coding tools. It auto-discovers conversation history wherever your agents store it, normalizes everything into a common schema, and gives you fast hybrid search (keyword + semantic + natural language) over the whole corpus — plus actionable outputs like "resume this thread," "export context," and "inject into a new agent prompt."

Built for developers who switch between coding agents daily and are tired of fragmented, un-searchable session memory.

## Why

The built-in history/search in each tool is siloed and weak. When you ask "where's the thread where we implemented the auth retry logic?", you have no way to answer it across tools. Crossthreads makes that a one-line query.

## Status

🛠️ **Phase 0 (prototype) — daemon + hybrid search working.** Sessions from **Claude Code** (JSONL) and **Cursor** (`state.vscdb` SQLite) are parsed, normalized, **persisted into one SQLite index** (deduped by content hash), and searchable with **hybrid retrieval** — FTS5 keyword (BM25) **+ semantic embeddings (all-MiniLM via ONNX)** fused with Reciprocal Rank Fusion. A background daemon (`crossthreadsd`) owns the index, **watches for new sessions and re-indexes automatically**, and serves search/status over a loopback socket. Aider and the desktop app are next.

```
crates/
  ct-core         # normalized schema + Connector trait + content hashing
  ct-connectors   # source-tool parsers (Claude Code + Cursor)
  ct-embed        # Embedder trait: hash (default) + ONNX/all-MiniLM (`onnx`)
  ct-store        # one SQLite index: FTS5 + vectors + RRF hybrid search (ADR-007)
  ct-index        # indexing orchestration shared by CLI + daemon
  ct-daemon       # `crossthreadsd` single-writer daemon + loopback API + watcher
  ct-cli          # `crossthreads` CLI (index / search / status; --remote)
  ct-mcp          # MCP server: agents query your history natively (FR-UI-04)
```

Try it (see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for the `onnx` semantic build):

```sh
cargo run -p ct-cli -- index                                    # discover, parse, store, embed
cargo run -p ct-cli -- search "auth keeps failing" --mode hybrid

# or run the always-on daemon and query it over the socket:
cargo run -p ct-daemon --bin crossthreadsd &
cargo run -p ct-cli -- status --remote
cargo run -p ct-cli -- search "auth keeps failing" --remote
```

### Documentation

- [`docs/PRD.md`](docs/PRD.md) — Product Requirements Document (vision, users, scope, GTM)
- [`docs/REQUIREMENTS.md`](docs/REQUIREMENTS.md) — Detailed functional & non-functional requirements
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — Reference architecture & tech decisions
- [`docs/AGENT_API.md`](docs/AGENT_API.md) — Agent-facing API & interface spec (CLI/JSON, MCP, HTTP)
- [`docs/DECISIONS.md`](docs/DECISIONS.md) — Decision log (ADRs)
- [`docs/ROADMAP.md`](docs/ROADMAP.md) — Phased delivery plan

Licensed under [Apache-2.0](LICENSE).

## Principles

1. **Local-first & private by default** — no telemetry, your data never leaves your machine unless you opt in.
2. **Better than grep** — hybrid lexical + semantic retrieval with reranking and NL query understanding.
3. **Actionable, not just searchable** — every result can be opened, exported, resumed, or injected.
4. **Resilient connectors** — tool formats change; detection is versioned and community-extensible.
5. **Agent-friendly** — agents can query Crossthreads via CLI/JSON and MCP tools.
