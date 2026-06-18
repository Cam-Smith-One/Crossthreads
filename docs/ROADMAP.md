# Crossthreads — Roadmap

| | |
|---|---|
| **Status** | Draft v0.1 |
| **Last updated** | 2026-06-18 |
| **Companion to** | [PRD.md](PRD.md) · [REQUIREMENTS.md](REQUIREMENTS.md) · [ARCHITECTURE.md](ARCHITECTURE.md) |

Phases map to requirement IDs in [REQUIREMENTS.md](REQUIREMENTS.md). Timeboxes are indicative for a solo/small-team effort.

---

## Phase 0 — Research & Prototype (1–2 weeks)
**Goal:** de-risk parsing and the build-on-CASS decision.

- Study CASS source deeply; decide fork/interoperate/clean-build (PRD Open Q1).
- Validate Cursor `state.vscdb` schema parsing and Claude Code JSONL parsing on real data.
- Build a minimal connector + SQLite indexer spike.
- Pick embedding model + vector store; benchmark local indexing speed.

**Exit:** can parse 2 tools end-to-end into SQLite and run a keyword search.

## Phase 1 — MVP (4–8 weeks)
**Goal:** ship the OSS core that is meaningfully better than built-ins.

Covers all **Must / P1** requirements:
- Connectors: Claude Code + Cursor + one of (Codex/Aider), incremental + dedup (FR-ING-01..08).
- Normalized schema + chunking (FR-SCH-01..04).
- SQLite/FTS + local vector store + local embeddings (FR-STO-01..03).
- Hybrid search + filters + NL queries + ranked results (FR-SRCH-01..04).
- Actions: open / export / inject (FR-ACT-01..03).
- TUI + CLI/JSON + agent API (FR-UI-01..03).
- Local-only, no telemetry, purge (NFR-PRIV-01..03); perf + robustness targets.

**Exit:** MVP acceptance gate in REQUIREMENTS §11 passes. **Validate** with the originating X/Grok thread community and heavy users.

## Phase 1.x — Fast-Follow (post-launch, weeks)
**Goal:** polish + reach.

- Desktop app (Tauri + shadcn/ui) (FR-UI-05).
- MCP server (FR-UI-04).
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
| **M0 Prototype** | 0 | 2 connectors → SQLite → keyword search |
| **M1 MVP / OSS launch** | 1 | Hybrid search, 3 tools, TUI + agent API, local-first |
| **M2 Desktop + MCP** | 1.x | Polished GUI, agent-native via MCP, more connectors |
| **M3 Memory + Sync** | 2 | Episodic→semantic→procedural, outcomes, team sync |
