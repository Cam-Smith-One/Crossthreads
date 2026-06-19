# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it reaches
a tagged release.

## [Unreleased]

The MVP backend is functionally complete and verified end-to-end (search,
filters, daemon, MCP, and the web UI). The native desktop window is scaffolded
but not yet released.

### Added
- **Connectors** for Claude Code (JSONL), Cursor (`state.vscdb`), Aider
  (`.aider.chat.history.md`), and Codex (`~/.codex/sessions` rollouts), with a
  regression corpus guarding against format drift.
- **Single SQLite index** (`ct-store`): content-hash dedup, FTS5 lexical search,
  `sqlite-vec`-style vector storage with brute-force cosine, and **RRF hybrid**
  search with a similarity floor. Tool/project/date **filters** and facets.
- **Embeddings** (`ct-embed`): a deterministic offline default and a real
  all-MiniLM-L6-v2 ONNX backend behind the `onnx` feature.
- **Daemon** (`crossthreadsd`): single-writer index, filesystem watcher with
  debounced auto-reindex, a loopback protocol server, and an HTTP/JSON bridge
  that also serves the web UI.
- **CLI** (`crossthreads`): `index`, `search`, `context`, `status`, with
  `--remote` to drive a running daemon.
- **MCP server** (`ct-mcp`): `crossthreads_search`, `crossthreads_recall`,
  `crossthreads_build_context`, and `crossthreads_status` over stdio for agents.
- **Web UI** (`ui/`): React + Vite — search, mode toggle, filters, highlighted
  results, conversation viewer, context block; served by the daemon.
- **Native shell** (`src-tauri/`): a Tauri wrapper around the web UI (scaffold;
  excluded from the CI workspace).
- Project docs (PRD, requirements, architecture, agent API, decisions, roadmap,
  development), CI, and demo/screenshot/check scripts.

### Notes
- Everything is local-first; the only optional network call is the one-time
  embedding-model download.

[Unreleased]: https://github.com/Cam-Smith-One/Crossthreads/commits/main
