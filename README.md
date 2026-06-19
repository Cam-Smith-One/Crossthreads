# Crossthreads

**One place to search, recall, and resume every AI coding conversation — across Claude Code, Codex, Cursor, Aider, Gemini CLI, and more.**

Crossthreads is a local-first session indexer and memory layer for AI coding tools. It auto-discovers conversation history wherever your agents store it, normalizes everything into a common schema, and gives you fast hybrid search (keyword + semantic + natural language) over the whole corpus — plus actionable outputs like "resume this thread," "export context," and "inject into a new agent prompt."

Built for developers who switch between coding agents daily and are tired of fragmented, un-searchable session memory.

## Why

The built-in history/search in each tool is siloed and weak. When you ask "where's the thread where we implemented the auth retry logic?", you have no way to answer it across tools. Crossthreads makes that a one-line query.

## Status

🛠️ **Phase 0 (prototype).** Product docs are complete and the Rust workspace is scaffolded. The `detect → discover → parse → normalize` pipeline runs end-to-end via the CLI for **Claude Code** (JSONL) and **Cursor** (`state.vscdb` SQLite — legacy chat + both composer layouts). Aider, the daemon, and storage are next.

```
crates/
  ct-core         # normalized schema + Connector trait + content hashing
  ct-connectors   # source-tool parsers (Claude Code + Cursor implemented)
  ct-cli          # `crossthreads` CLI (Phase 0: `index` dry-run)
  ct-daemon       # `crossthreadsd` background daemon (scaffold)
  ct-mcp          # MCP server (scaffold)
```

Try it: `cargo run -p ct-cli -- index` (reads `~/.claude/projects/`).

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
