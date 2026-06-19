# Crossthreads — Roadmap

| | |
|---|---|
| **Status** | Draft v0.1 |
| **Last updated** | 2026-06-18 |
| **Companion to** | [PRD.md](PRD.md) · [REQUIREMENTS.md](REQUIREMENTS.md) · [ARCHITECTURE.md](ARCHITECTURE.md) |

Phases map to requirement IDs in [REQUIREMENTS.md](REQUIREMENTS.md). Timeboxes are indicative for a solo/small-team effort.

---

## Phase 0 — Research & Prototype (1–2 weeks)
**Goal:** de-risk parsing, the Tauri shell, and the ML-runtime placement.

- ✅ Study CASS/public-extractor source for connector *patterns* (no dependency — clean build is decided).
- ✅ Connectors: Claude Code (JSONL), Cursor (`state.vscdb`), and Aider (`.aider.chat.history.md`) — the full MVP set (ADR-008).
- ✅ Build a minimal connector + SQLite indexer behind a thin Rust core API (`ct-core` / `ct-connectors` / `ct-store` / `ct-cli`).
- ✅ Persist into a single SQLite index with content-hash dedup + FTS5 keyword search.
- ✅ Semantic + **hybrid (RRF)** search: vectors in the same DB, real ONNX embeddings (`ct-embed --features onnx`, all-MiniLM) with a deterministic offline default.
- ✅ Resolve **ML-runtime placement** — pure-Rust ONNX via `fastembed`/`ort`, validated end-to-end.
- ✅ Background daemon (`crossthreadsd`): single-writer index, file-watch auto-reindex, loopback search/status API; CLI `--remote` client (ADR-005).
- 🟡 Primary surface: React + Vite UI (`ui/`) served by the daemon over an HTTP/JSON bridge — search, hybrid mode, and context block, verified headlessly. The native Tauri wrapper around the same frontend is pending an environment with platform webview libs + a display.

**Exit:** ✅ parse 2 tools end-to-end into SQLite and run keyword **and hybrid** search (`crossthreads index` + `crossthreads search --mode hybrid`). ⬜ display results in the Tauri shell.

## Phase 1 — MVP (4–8 weeks)
**Goal:** ship the OSS core that is meaningfully better than built-ins.

Covers all **Must / P1** requirements:
- Connectors: Claude Code + Cursor + one of (Codex/Aider), incremental + dedup (FR-ING-01..08).
- Normalized schema + chunking (FR-SCH-01..04).
- SQLite/FTS + local vector store + local embeddings (FR-STO-01..03).
- Hybrid search + filters + NL queries + ranked results (FR-SRCH-01..04).
- Actions: open / export / inject (FR-ACT-01..03).
- **Desktop app (Tauri, primary surface)** + CLI/JSON + agent API (FR-UI-05, FR-UI-02/03).
- Local-only, no telemetry, purge (NFR-PRIV-01..03); perf + robustness targets.

**Exit:** MVP acceptance gate in REQUIREMENTS §11 passes. **Validate** with the originating X/Grok thread community and heavy users.

## Phase 1.x — Fast-Follow (post-launch, weeks)
**Goal:** polish + reach.

- TUI surface over the same core (FR-UI-01).
- ✅ MCP server (FR-UI-04) — `ct-mcp` exposes search/status to agents over stdio→daemon (landed early).
- Timeline + analytics (FR-UI-06).
- Resume/handoff action, HTML export (FR-ACT-04/05).
- More connectors: Gemini CLI, Windsurf, Claude.ai web import (FR-ING-09/10).
- Saved searches, retention/pruning, schema migrations, optional cloud embeddings (FR-SRCH-07, FR-STO-04/06, FR-SCH-05).

## Phase 2 — Differentiation & Memory (post-MVP)
**Goal:** the moat — memory depth, outcomes, collaboration.

- Memory layers: semantic + procedural (FR-MEM-02/03).
- Outcome tracking — link sessions to git/PRs/tests, "what worked" (FR-MEM-04).
- Proactive memory & suggestions (FR-MEM-05); knowledge graph / visualization (FR-MEM-06).
- Multi-machine sync (encrypted) (FR-SYNC-01); team sharing, audit/SSO (FR-SYNC-02/03).
- Export ecosystem (Obsidian/Notion/`AGENTS.md`/`CLAUDE.md`) (FR-ACT-06).
- Connector plugin system (FR-ING-11); optional Postgres backend (FR-STO-05).

## Monetization track (parallel, post-MVP)
- Freemium desktop (advanced memory/analytics paid).
- Team SaaS tier (hosted/synced index, sharing, audit logs, SSO).
- Distribution: GitHub releases, Homebrew/Scoop, Product Hunt, Hacker News, dev forums.

## Milestone summary

| Milestone | Phase | Outcome |
|---|---|---|
| **M0 Prototype** | 0 | 2 connectors → SQLite → keyword search in a Tauri shell |
| **M1 MVP / OSS launch** | 1 | Hybrid search, 3 tools, desktop app + agent API, local-first |
| **M2 TUI + MCP** | 1.x | Terminal surface, agent-native via MCP, more connectors |
| **M3 Memory + Sync** | 2 | Episodic→semantic→procedural, outcomes, team sync |
