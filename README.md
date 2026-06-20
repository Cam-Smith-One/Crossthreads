<div align="center">

<img src=".github/assets/mark.png" alt="Crossthreads" width="110" />

# Crossthreads

**Search, recall, and resume every AI coding conversation — across all your tools, in one place.**

[![CI](https://github.com/Cam-Smith-One/Crossthreads/actions/workflows/ci.yml/badge.svg)](https://github.com/Cam-Smith-One/Crossthreads/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Local-first](https://img.shields.io/badge/local--first-no%20telemetry-2ea44f.svg)](#-principles)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584.svg)](#-how-it-works)
[![MCP](https://img.shields.io/badge/agent--ready-MCP-6e56cf.svg)](#-for-agents-mcp)
[![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](CONTRIBUTING.md)

⚠️ Early Beta: Under active development. Expect a lot of rough edges.

<br/>

<img src=".github/assets/screenshot-dark.png" alt="Crossthreads searching across tools" width="760" />

</div>

---

Crossthreads is a **local-first session indexer and memory layer** for AI coding agents. It auto-discovers your conversation history wherever each tool stores it, normalizes everything into one schema, and gives you **fast hybrid search** (keyword + semantic) over the whole corpus — then makes every result *actionable*: open the original, copy a resume command, or build a paste-ready context block for a fresh agent. A background daemon keeps the index live, and an MCP server lets your agents query it natively.

> **Where's the thread where we fixed the OAuth refresh retry?** — one query, every tool, instant answer.

## Contents

- [Why](#-why) · [Quick start](#-quick-start) · [Features](#-features) · [Supported tools](#-supported-tools)
- [Search modes](#-search-modes) · [Screenshots](#-screenshots) · [For agents (MCP)](#-for-agents-mcp)
- [How it works](#-how-it-works) · [Privacy](#-privacy) · [Documentation](#-documentation) · [Roadmap](#-roadmap) · [Contributing](#-contributing)

## ⚡ Why

Every coding agent stores its own history in its own format — Claude Code JSONL here, Cursor SQLite there, Codex rollouts somewhere else — and each tool's built-in search is siloed and weak. When you switch tools daily, your most valuable context (how you solved something last week) becomes unsearchable.

Crossthreads indexes **all of it** into one place and answers cross-tool questions in milliseconds — locally, with no telemetry and no account.

## 🚀 Quick start

**Install — no toolchain required.** Grab a prebuilt build and open the app:

```sh
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/Cam-Smith-One/Crossthreads/main/scripts/install.sh | bash
crossthreads-up        # indexes your sessions, then opens http://127.0.0.1:47101
```

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/Cam-Smith-One/Crossthreads/main/scripts/install.ps1 | iex
crossthreads-up
```

This installs to `~/.crossthreads` (`%USERPROFILE%\.crossthreads` on Windows), links the `crossthreads` / `crossthreadsd` / `ct-mcp` commands onto your PATH, indexes whatever tools you have, watches for new sessions, and opens the web app.

> Hit a snag? See [**Troubleshooting**](docs/TROUBLESHOOTING.md) — it covers the usual install/run/indexing gotchas.

<details>
<summary><b>Build from source</b> (Rust + Node)</summary>

```sh
# One command: build, index your real sessions, and open the app.
scripts/start.sh              # fast offline search
CT_ONNX=1 scripts/start.sh    # real semantic search (downloads all-MiniLM once)

# …or run the pieces directly:
cargo run --release -p ct-cli -- index
cargo run --release -p ct-cli -- search "oauth refresh retry" --mode hybrid
cd ui && npm install && npm run build && cd ..
cargo run --release --features onnx -p ct-daemon -- --http 127.0.0.1:47101 --ui ui/dist

# …or bring up a sample multi-tool corpus + UI:
CT_ONNX=1 scripts/demo.sh
```

> Prebuilt binaries use the deterministic offline embedder (instant, no network). Build with `--features onnx` for ONNX/all-MiniLM semantic search.

</details>

<details>
<summary><b>Search from the terminal</b></summary>

```sh
crossthreads search "where did we set up the websocket reconnect?" --mode hybrid
crossthreads context "oauth refresh retry" > prior-context.md   # paste-ready block
crossthreads status                                              # index health
```

</details>

## ✨ Features

| | |
|---|---|
| 🔎 **Hybrid search** | FTS5 keyword (BM25) **+** semantic embeddings fused with Reciprocal Rank Fusion — finds threads by meaning, not just keywords. |
| 🧩 **9 tools, one index** | Claude Code, Codex, Cursor, Aider, Cline, Copilot Chat, Gemini CLI, Windsurf, Antigravity — auto-detected, deduped by content hash. |
| 🧠 **Skills & prompts too** | Searches reusable Claude `SKILL.md` and Codex prompts alongside conversations (`kind` filter). |
| 📌 **Bookmarks & pins** | Durable, kept in a separate store keyed by a stable session id — they survive re-indexing *and* a conversation growing. |
| 🔗 **Actionable results** | Open the original file in your file manager, copy a `claude --resume` / `codex resume` command, or build a context block to inject into a new agent. |
| 🤖 **Agent-native (MCP)** | Agents call `search` / `recall` / `build_context` / `status` over MCP — your history becomes their memory. |
| 🛰️ **Live & automatic** | A background daemon watches your tools' storage and re-indexes new sessions as they land. |
| 🌗 **Polished web UI** | Fast React UI with light/dark themes, filters, keyboard nav, and a full transcript viewer. |
| 🔒 **Local-first** | No telemetry, no account, nothing leaves your machine. One SQLite file you can back up or delete. |

## 🧩 Supported tools

| Tool | Where it reads | Confidence |
|---|---|---|
| **Claude Code** | `~/.claude/projects/**/*.jsonl` | ✅ High |
| **Codex** | `~/.codex/sessions/**/rollout-*.jsonl` | ✅ High |
| **Cursor** | `…/Cursor/User/**/state.vscdb` (composer + legacy chat) | ✅ High |
| **Aider** | `**/.aider.chat.history.md` | ✅ High |
| **Cline** | `…/<editor>/globalStorage/saoudrizwan.claude-dev/tasks/**` | ✅ High |
| **GitHub Copilot Chat** | `…/<editor>/**/chatSessions/*.{json,jsonl}` | ✅ High |
| **Gemini CLI** | `~/.gemini/tmp/*/chats/session-*.{json,jsonl}` | ✅ High |
| **Windsurf** | `…/Windsurf/User/**/state.vscdb` | 🟡 Medium |
| **Antigravity** | `~/.gemini/antigravity*/brain/<uuid>/*.md` (best-effort)¹ | 🟠 Low |
| **Skills / prompts** | Claude `~/.claude/skills/**/SKILL.md`, Codex `~/.codex/prompts/*.md` | ✅ High |

<sub>¹ Antigravity's full conversation lives in an undocumented protobuf format; Crossthreads indexes the readable Markdown task/plan artifacts until that format is documented. Connectors detect-and-skip gracefully and are stamped with a versioned fingerprint, so a format change degrades to "missed a session," never a crash.</sub>

## 🔎 Search modes

| Mode | What it does | Best for |
|---|---|---|
| `lexical` | FTS5 / BM25 keyword search | exact terms, identifiers, error strings |
| `semantic` | cosine over all-MiniLM embeddings (in-memory, normalized) | "the thread about login failing" → finds *OAuth 401* |
| `hybrid` *(default)* | Reciprocal Rank Fusion of both | the best of each — recommended |

All modes support **filters**: tool, `kind` (thread/skill), project substring, and a date range.

## 📸 Screenshots

<div align="center">
<img src=".github/assets/screenshot-dark.png" alt="Dark mode" width="48%" />
<img src=".github/assets/screenshot-light.png" alt="Light mode" width="48%" />
</div>

## 🤖 For agents (MCP)

Point any MCP client at the `ct-mcp` binary and your agent gets four tools:

| Tool | Purpose |
|---|---|
| `crossthreads_search` | hybrid/semantic/lexical search with filters |
| `crossthreads_recall` | fetch a full conversation by id |
| `crossthreads_build_context` | render top matches into a paste-ready context block |
| `crossthreads_status` | index health and counts |

```jsonc
// e.g. in an MCP client config
{ "mcpServers": { "crossthreads": { "command": "ct-mcp" } } }
```

See [docs/AGENT_API.md](docs/AGENT_API.md) for the full CLI/JSON, HTTP, and MCP contracts.

## 🏗️ How it works

```text
   AI coding tools                Crossthreads daemon                Surfaces
 ┌──────────────────┐          ┌───────────────────────┐        ┌──────────────┐
 │ Claude Code JSONL│          │  connectors → normalize│        │  Web UI       │
 │ Cursor  state.vscdb│  watch │  → SQLite index        │  RPC   │  CLI          │
 │ Codex rollouts   │ ───────► │  • FTS5 (BM25)         │ ─────► │  MCP server   │
 │ Cline / Copilot  │          │  • vectors (cosine)    │        │  (agents)     │
 │ Gemini / Windsurf│          │  • RRF hybrid + filters│        └──────────────┘
 │ Aider / Antigrav.│          │  • durable user_state  │
 └──────────────────┘          └───────────────────────┘
```

A single-writer daemon owns one SQLite database (conversations, an FTS5 index, vector embeddings, and a separate durable `user_state` table for bookmarks/pins). It serves search/status/context over a loopback socket **and** an HTTP bridge that also serves the web UI. The same daemon backs the CLI, the desktop shell, and the MCP server.

```text
crates/
  ct-core         normalized schema + Connector trait + content hashing
  ct-connectors   9 conversation connectors + skills/prompts (+ shared VS Code helpers)
  ct-embed        Embedder trait: deterministic hash (default) + ONNX/all-MiniLM (`onnx`)
  ct-store        one SQLite index: FTS5 + vectors + RRF hybrid + filters + user_state
  ct-index        indexing orchestration shared by CLI + daemon
  ct-daemon       crossthreadsd: single-writer daemon + loopback/HTTP API + file watcher
  ct-cli          crossthreads CLI (index / search / context / status)
  ct-mcp          MCP server: agents query your history natively
ui/               React + Vite frontend (web + Tauri), talks to the daemon
```

## 🔒 Privacy

- **Local-first, no telemetry.** Nothing leaves your machine; there's no account and no network call except the optional one-time model download for ONNX semantic search.
- **One file.** The entire index is a single SQLite database under your platform data dir — back it up, inspect it, or delete it anytime.
- **Rebuildable by design.** The index is a cache regenerated from your tools' own files; your bookmarks/pins live in a separate table so a rebuild never loses them.

## 📚 Documentation

| | |
|---|---|
| [DEVELOPMENT](docs/DEVELOPMENT.md) | Build, test, run the daemon/UI/MCP; the `onnx` feature; releasing |
| [TROUBLESHOOTING](docs/TROUBLESHOOTING.md) | Fixes for common install/run/indexing issues |
| [ARCHITECTURE](docs/ARCHITECTURE.md) | Components, data model, daemon process model, privacy posture |
| [AGENT_API](docs/AGENT_API.md) | CLI/JSON, MCP, and HTTP interface (search / recall / build_context) |
| [PRD](docs/PRD.md) · [REQUIREMENTS](docs/REQUIREMENTS.md) · [ROADMAP](docs/ROADMAP.md) | Product intent, requirements, status |
| [DECISIONS](docs/DECISIONS.md) | Architecture decision records (ADRs) |

```sh
scripts/check.sh   # everything CI runs: fmt + clippy + tests + UI build
scripts/demo.sh    # bring up the full stack against a sample corpus
```

## 🗺️ Roadmap

- ✅ Connectors for 9 tools + skills/prompts, hybrid search, daemon, web UI, MCP, durable bookmarks/pins, prebuilt releases
- ⏳ Native desktop app (Tauri shell scaffolded), richer resume/deeplinks, notes & tags on conversations
- 🔭 `sqlite-vec` ANN for very large corpora, optional encrypted cross-machine sync, community connector plugins

See [docs/ROADMAP.md](docs/ROADMAP.md) for the phased plan mapped to requirement IDs.

## 🤝 Contributing

Contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md). Adding a connector is a self-contained change: implement the `Connector` trait, register it in `builtin()`, and add a regression fixture. Security issues: please report privately per [SECURITY.md](SECURITY.md).

> **Status:** pre-release. The backend (connectors, search, daemon, MCP, web UI) is implemented and tested; the native desktop app is scaffolded. Not yet publicly announced.

## 🧭 Principles

1. **Local-first & private by default** — no telemetry; your data never leaves your machine unless you opt in.
2. **Better than grep** — hybrid lexical + semantic retrieval with natural-language queries.
3. **Actionable, not just searchable** — every result can be opened, resumed, or injected.
4. **Resilient connectors** — tool formats change; detection is versioned and degrades gracefully.
5. **Agent-friendly** — agents query Crossthreads over CLI/JSON and MCP.

## 📄 License

[Apache-2.0](LICENSE) © Crossthreads contributors.
