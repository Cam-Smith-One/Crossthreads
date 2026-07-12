<div align="center">

<img src=".github/assets/mark.png" alt="Crossthreads" width="110" />

# Crossthreads

**Search, recall, and resume every AI coding conversation — and understand how you work — across all your tools and devices, in one place.**

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

Crossthreads is a **local-first session indexer, memory layer, and analytics engine** for AI coding agents. It searches across **Claude Code, Codex, Cursor, Aider, Cline, GitHub Copilot Chat, Gemini CLI, Windsurf, and Antigravity** — auto-discovering each tool's conversation history wherever it lives, normalizing everything into one schema, and giving you **fast hybrid search** (keyword + semantic) over the whole corpus. Then it makes every result *actionable*: open the original, copy a resume command, or build a paste-ready context block for a fresh agent.

But it goes well past search. On top of the same index it builds a layer of **understanding**: ask a question and get a **cited answer** from your own history; synthesize **insights** (open loops, decisions, knowledge cards, recurring patterns); and — uniquely — analyze *how you actually work* from the full width of your data (turns, rework, delegation, languages, tempo) into a quantified **Work DNA** profile, **gaps**, **prompt coaching**, mined **repeatable skills**, and a **proactive weekly review**. A background daemon keeps the index live, an MCP server exposes all of it to your agents (**21 tools**), and **cross-device search** lets one query span your *other* machines too — federated over your own Tailscale network, with no central server.

> **Where's the thread where we fixed the OAuth refresh retry?** — one query, every tool, instant answer.
>
> **And: what am I leaving unfinished, what do I keep re-deriving, and how could I prompt better?** — your own history, answered.

## Contents

- [Why](#-why) · [Quick start](#-quick-start) · [Features](#-features) · [Beyond search](#-beyond-search--understand-how-you-work) · [Supported tools](#-supported-tools)
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
crossthreads themes --k 8                                        # cluster your work into themes
crossthreads themes --name                                       # …and name them with your local model login
crossthreads metrics                                             # how you work, by the numbers (no model)
crossthreads optimize                                            # the biggest change to make now + a verdict on the last one
crossthreads trends                                              # how your metrics are trending (30d vs previous)
crossthreads weekly                                              # your weekly review + one thing to try
crossthreads insight work_dna                                    # a quantified profile of how you work
crossthreads insight process_miner                               # repeatable procedures → draft skills
crossthreads insight open_loops                                  # unresolved work across recent sessions
crossthreads ask "how did we handle token refresh?"             # cited answer from your own history
crossthreads activity --by week                                  # your sessions over time
crossthreads graph                                               # projects ↔ tools ↔ tags
crossthreads llm-auth                                            # which model credentials are available
```

</details>

## ✨ Features

| | |
|---|---|
| 🔎 **Hybrid search** | FTS5 keyword (BM25) **+** semantic embeddings fused with Reciprocal Rank Fusion — finds threads by meaning, not just keywords. |
| 🧩 **9 tools, one index** | Claude Code, Codex, Cursor, Aider, Cline, Copilot Chat, Gemini CLI, Windsurf, Antigravity — auto-detected, deduped by content hash. |
| 🖥️ **Cross-device search** | Search the history on your *other* machines too — federated over your private Tailscale tunnel, results tagged by device, local-first. [Set up →](docs/MULTI_DEVICE_SETUP.md) |
| 🗺️ **Theme map** | Clusters your sessions by topic so you can see what you've been working on across tools — in the web app (🗺️) and as `crossthreads themes`. Offline; "✨ Name with AI" labels clusters using your model login. |
| 🧬 **How you work** | A deterministic metrics pass over the *full width* of your history — turns per task, rework rate, prompt specificity, abandonment, tempo, **delegation** (agent actions like Edit/Bash per session), **languages**, **model mix**, **median session length**, friction by project (`crossthreads metrics`, no model) — that powers four behavioral insights: a quantified **Work DNA** profile, your **gaps & blind spots** (+ a draft CLAUDE.md), a **prompt coach** (before→after rewrites from your own prompts), and a **process miner** (repeated workflows → draft skills). Web app (💡), CLI, and MCP. |
| 📋 **Proactive weekly review** | Without being asked: "what you worked on, how you worked, and one thing to try next week," from the last 7 days. The web app surfaces a banner when a fresh one is ready (the daemon generates it in the background); also `crossthreads weekly` and MCP. |
| 💡 **Insights** | LLM synthesis over your whole history — **open loops**, a **decision log**, **knowledge cards**, a **how-I-work** profile, **recurring** patterns, and a **digest** — spanning every tool *and* your skills/prompts (hundreds of records). In the web app (💡), the CLI (`crossthreads insight`), and over MCP. Opt-in; uses your model login. |
| 💬 **Ask (RAG)** | Ask a question and get a **cited answer** drawn from your own past sessions across every tool — "how did we end up handling token refresh?". Retrieves, then synthesizes (or returns context without a model). Web app (💬), `crossthreads ask`, and MCP. |
| 📅 **Temporal view** | Your sessions over time, bucketed by day or week with a per-tool breakdown — when and with what you've been working. Web app (📅), `crossthreads activity`, and MCP. Offline. |
| 🕸️ **Knowledge graph** | A map of your work — projects, tools, and tags as nodes (sized by activity) with co-occurrence edges. Web app (🕸️), `crossthreads graph`, and MCP. Deterministic; offline. |
| 🤝 **Bring your own model (optional)** | Reuse your existing **Claude Code / Codex / Gemini** login — or paste an API key in Settings → Models (stored in the OS keychain) — to power insights and name themes. Off by default; the index never calls a model. |
| 🧠 **Skills & prompts too** | Searches reusable Claude `SKILL.md` and Codex prompts alongside conversations (`kind` filter). |
| 📌 **Bookmarks & pins** | Durable, kept in a separate store keyed by a stable session id — they survive re-indexing *and* a conversation growing. |
| 🔗 **Actionable results** | Open the original file in your file manager, copy a `claude --resume` / `codex resume` command, or build a context block to inject into a new agent. |
| 🤖 **Agent-native (MCP)** | **24 tools** over MCP — search/recall/ask/build_context plus the whole insights & analytics suite — so your history *and* your working profile become an agent's memory. |
| 🛰️ **Live & automatic** | A background daemon watches your tools' storage and re-indexes new sessions as they land. |
| 🌗 **Polished web UI** | Fast React UI with light/dark themes, filters, keyboard nav, and a full transcript viewer. |
| 🔒 **Local-first** | No telemetry, no account, nothing leaves your machine. One SQLite file you can back up or delete. |

## 🧠 Beyond search — understand how you work

Search answers *"where's that thread?"*. The layer on top answers *"how do I work, and what should I do differently?"* — built from the **full width** of your indexed sessions (not just the text: turns, corrections, agent actions, languages, models, timings). All of it is local, opt-in for the model-backed parts, and available in the web app (💡 / 📋), the CLI, and over MCP.

| | What it tells you | How |
|---|---|---|
| 💬 **Ask** | A **cited answer** to any question, synthesized from your own past sessions across every tool. | `crossthreads ask "…"` · `crossthreads_ask` |
| 💡 **Insights** | **Open loops** (unfinished work), a **decision log**, **knowledge cards**, **recurring** problems you keep hitting, a **how-I-work** profile, and a **digest** — over your whole history including skills. | `crossthreads insight <kind>` · `crossthreads_open_loops`, … |
| 🧬 **Work DNA & metrics** | A quantified, evidence-backed profile of how you work — rhythm, **rework rate**, prompt **specificity**, **delegation** (agent actions/session), **languages**, **model mix**, **session length**, **time-to-first-response**, **error recovery**, **context-switching**, and where **friction** concentrates. The raw numbers need no model. | `crossthreads metrics` / `insight work_dna` · `crossthreads_metrics`, `crossthreads_work_dna` |
| 🎯 **Optimize (closed loop)** | Turns the metrics from *descriptive* into *prescriptive and verified*: the **single highest-impact change** to make now, then a week later a **measured verdict** on whether it worked (validation-gated), plus **trends** (↑ better / ↓ worse vs last month) and a running optimization log. Deterministic. | `crossthreads optimize` / `trends` · `crossthreads_optimize`, `crossthreads_trends` |
| 🧭 **AI-fluency + reflection** | The same signals organized around the **4D framework** — Delegation, Description, Discernment, Diligence — each **scored 0–100** with its movement, the metrics behind it, and a plain read. Ends on a **reflective question, not a to-do** (what to keep doing yourself), which you can talk through in *Ask*. Deterministic. | `crossthreads fluency` · `crossthreads_fluency` |
| 🩺 **Gaps & prompt coach** | What you start but don't finish and practices you're missing (+ a draft **CLAUDE.md**); plus your smoothest vs roughest sessions with **before→after rewrites** from your own prompts. | `crossthreads insight gaps` / `prompt_coach` |
| 🛠️ **Process miner** | Procedures you keep re-deriving, mined into **draft, installable `SKILL.md`** artifacts so you stop repeating them. | `crossthreads insight process_miner` · `crossthreads_process_miner` |
| 📋 **Proactive weekly review** | Without being asked: what you worked on, how you worked, and **one thing to try next week** — generated in the background and surfaced when ready. | `crossthreads weekly` · `crossthreads_weekly_review` |
| 🗺️ 📅 🕸️ **Maps** | A **theme map**, a **temporal view** (activity over time), and a **knowledge graph** (projects ↔ tools ↔ tags). All offline. | `crossthreads themes` / `activity` / `graph` |

> The deterministic parts (search, metrics, themes, activity, graph) never call a model. The synthesis parts (ask, insights, Work DNA, weekly review) reuse your existing **Claude Code / Codex / Gemini** login or a key you paste in Settings → Models — nothing leaves your machine unless you set that up.

## 🧩 Supported tools

These are the AI coding agents and assistants Crossthreads reads from. Each one
keeps its own conversation history in its own place and format — a `.jsonl` log
here, a SQLite `state.vscdb` there, Markdown somewhere else. Crossthreads has a
small **connector** per tool that:

1. **detects** whether the tool's data is present on your machine,
2. **reads it where it lives** (always **read-only** — open editors are fine),
3. **normalizes** every session into one common schema (tool, project, messages,
   timestamps), and
4. **deduplicates** by content hash into a single index.

So a single search spans *all* of them at once — no per-tool exporting, no
copy-paste. Add a tool to your workflow and its history shows up automatically on
the next index.

| Tool | What it is | Where Crossthreads reads it | Confidence |
|---|---|---|---|
| **Claude Code** | Anthropic's terminal coding agent | `~/.claude/projects/**/*.jsonl` | ✅ High |
| **Codex** | OpenAI's CLI coding agent | `~/.codex/sessions/**/rollout-*.jsonl` | ✅ High |
| **Cursor** | AI-first VS Code fork | `…/Cursor/User/**/state.vscdb` (composer + legacy chat) | ✅ High |
| **Aider** | Terminal pair-programmer | `**/.aider.chat.history.md` | ✅ High |
| **Cline** | Agentic VS Code extension | `…/<editor>/globalStorage/saoudrizwan.claude-dev/tasks/**` | ✅ High |
| **GitHub Copilot Chat** | Copilot's chat in VS Code | `…/<editor>/**/chatSessions/*.{json,jsonl}` | ✅ High |
| **Gemini CLI** | Google's terminal coding agent | `~/.gemini/tmp/*/chats/session-*.{json,jsonl}` | ✅ High |
| **Windsurf** | Codeium's agentic editor (Cascade) | `…/Windsurf/User/**/state.vscdb` | 🟡 Medium |
| **Antigravity** | Google's agentic IDE | Markdown artifacts under `~/.gemini/antigravity/` (best-effort)¹ | 🟠 Low |
| **Skills / prompts** | Reusable Claude `SKILL.md` + Codex prompts | `~/.claude/skills/**/SKILL.md`, `~/.codex/prompts/*.md` | ✅ High |

<sub>¹ Antigravity's full conversation lives in an undocumented protobuf format; Crossthreads scans the Antigravity home and indexes the readable Markdown task/plan artifacts (wherever they sit under it) until that format is documented. Connectors detect-and-skip gracefully and are stamped with a versioned fingerprint, so a format change degrades to "missed a session," never a crash.</sub>

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

Point any MCP client at the `ct-mcp` binary and your agent gets twenty-four tools:

| Tool | Purpose |
|---|---|
| `crossthreads_search` | hybrid/semantic/lexical search with filters |
| `crossthreads_recall` | answer-oriented digest of relevant past sessions for a question |
| `crossthreads_ask` | a synthesized, **cited** answer to a question, from the user's history |
| `crossthreads_build_context` | render top matches into a paste-ready context block |
| `crossthreads_status` | index health and counts |
| `crossthreads_devices` | list the devices available to cross-device search |
| `crossthreads_themes` | cluster the user's sessions into themes (`name` for AI labels) |
| `crossthreads_activity` | session activity over time (day/week), with a per-tool breakdown |
| `crossthreads_graph` | a knowledge graph of projects, tools, and tags with co-occurrence edges |
| `crossthreads_metrics` | hard behavioral metrics on how the user works (turns, friction, delegation, languages, tempo) |
| `crossthreads_optimize` | the biggest change to make now + a measured verdict on the last one + trends |
| `crossthreads_trends` | how each working metric is trending (last 30 days vs previous) |
| `crossthreads_fluency` | AI-fluency across 4 dimensions (delegation/description/discernment/diligence), scored, + a reflective question |
| `crossthreads_weekly_review` | the user's weekly review: what they did, how they worked, one thing to try |
| `crossthreads_work_dna` | a quantified, evidence-backed profile of how the user works |
| `crossthreads_gaps` | gaps & blind spots, plus a personalized draft CLAUDE.md |
| `crossthreads_prompt_coach` | prompt coaching from the user's smoothest vs roughest sessions |
| `crossthreads_process_miner` | repeatable procedures mined into draft SKILL.md artifacts |
| `crossthreads_open_loops` | unresolved work, dangling TODOs, and unconfirmed fixes |
| `crossthreads_knowledge_cards` | durable Q→A cards worth remembering |
| `crossthreads_decision_log` | notable decisions ("chose X over Y because Z") with rationale |
| `crossthreads_how_i_work` | the user's working conventions, for a CLAUDE.md / AGENTS.md |
| `crossthreads_recurring` | recurring problems/patterns the user keeps hitting, with fixes |
| `crossthreads_digest` | a short reflective digest of recent work |

`metrics`, `search`, `themes`, `activity`, and `graph` work offline; the synthesis
tools (`ask`, `work_dna`, `gaps`, `prompt_coach`, `process_miner`, and the
`insight` family) use the user's model login.

```jsonc
// e.g. in an MCP client config
{ "mcpServers": { "crossthreads": { "command": "ct-mcp" } } }
```

**Install the agent skill** so your tools proactively recall prior work (it
encodes "recall before you build, search when stuck"):

```sh
crossthreads skill install        # Claude Code skill + Codex /crossthreads prompt
```

`ct-mcp` **auto-starts `crossthreadsd`** on first use (and reuses one that's
already running — there's only ever one writer per index), so registering the
MCP server is the only step. Use the **absolute path** to `ct-mcp` so it can find
its sibling daemon. See [docs/AGENT_API.md](docs/AGENT_API.md) for the full
CLI/JSON, HTTP, and MCP contracts.

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

A single-writer daemon owns one SQLite database (conversations, an FTS5 index, vector embeddings, a durable `user_state` table for bookmarks/pins, and a small `meta_kv` for cached artifacts like the weekly review). It serves search/status/context **and** the higher-level surfaces — themes, the deterministic behavioral metrics, and (with your model login) ask/insights/Work DNA/weekly-review synthesis — over a loopback socket and an HTTP bridge that also serves the web UI. The same daemon backs the CLI, the desktop shell, and the MCP server.

```text
crates/
  ct-core         normalized schema + Connector trait + content hashing
  ct-connectors   9 conversation connectors + skills/prompts (+ shared VS Code helpers)
  ct-embed        Embedder trait: deterministic hash (default) + ONNX/all-MiniLM (`onnx`)
  ct-store        one SQLite index: FTS5 + vectors + RRF hybrid + filters + user_state
                  + themes/clustering + behavioral metrics ("how you work")
  ct-index        indexing orchestration shared by CLI + daemon
  ct-llm          optional model backend: reuses your Claude/Codex/Gemini login (or a BYO key)
  ct-daemon       crossthreadsd: single-writer daemon + loopback/HTTP API + watcher
                  + insights/ask/weekly synthesis on top of the index
  ct-cli          crossthreads CLI (index/search/context/themes/insight/ask/activity/graph/metrics/weekly)
  ct-mcp          MCP server: agents query your history + working profile natively
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
| [CROSS_DEVICE_SEARCH](docs/CROSS_DEVICE_SEARCH.md) | Search your other machines, local-first (federation over Tailscale) |
| [MULTI_DEVICE_SETUP](docs/MULTI_DEVICE_SETUP.md) | Step-by-step: connect your devices for cross-device search |

```sh
scripts/check.sh   # everything CI runs: fmt + clippy + tests + UI build
scripts/demo.sh    # bring up the full stack against a sample corpus
```

## 🗺️ Roadmap

- ✅ Connectors for 9 tools + skills/prompts, hybrid search, daemon, web UI, MCP (24 tools), durable bookmarks/pins, notes & tags, prebuilt releases, **cross-device search** (federation over Tailscale)
- ✅ **Understanding layer:** cited **Ask** (RAG), **insights** (open loops / decisions / cards / recurring / digest), theme map, temporal view, knowledge graph, **behavioral metrics + Work DNA / gaps / prompt coach / process miner**, a **proactive weekly review**, an **optimize** closed loop, and a **4D AI-fluency** view with reflective prompts — all reusing your existing model login
- ✅ **Native menu-bar app (Tauri)** — a tray icon that toggles a compact **glance** popover fusing per-tool usage (today vs. your baseline) with the insight read (fluency, unresolved threads, optimize focus); Mac + Windows + Linux
- ⏳ Live provider quota/reset countdowns in the glance (on-device readers), richer resume/deeplinks, scheduled weekly-review delivery (cron/launchd)
- 🔭 `sqlite-vec` ANN for very large corpora, offline cross-device access (index replication) + team sharing, community connector plugins

See [docs/ROADMAP.md](docs/ROADMAP.md) for the phased plan mapped to requirement IDs.

## 🤝 Contributing

Contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md). Adding a connector is a self-contained change: implement the `Connector` trait, register it in `builtin()`, and add a regression fixture. Security issues: please report privately per [SECURITY.md](SECURITY.md).

> **Status:** pre-release. The backend (connectors, search, the insights/analytics layer, daemon, MCP, web UI) is implemented and tested; the native menu-bar app (Tauri) is built and validated on-device. Not yet publicly announced.

## 🧭 Principles

1. **Local-first & private by default** — no telemetry; your data never leaves your machine unless you opt in.
2. **Better than grep** — hybrid lexical + semantic retrieval with natural-language queries.
3. **Actionable, not just searchable** — every result can be opened, resumed, or injected.
4. **Understanding, not just retrieval** — your history reveals how you work; metrics are deterministic, narratives cite them.
5. **Resilient connectors** — tool formats change; detection is versioned and degrades gracefully.
6. **Agent-friendly** — agents query Crossthreads over CLI/JSON and MCP.

## 📄 License

[Apache-2.0](LICENSE) © Crossthreads contributors.
