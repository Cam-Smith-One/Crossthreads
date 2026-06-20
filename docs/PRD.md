# Crossthreads — Product Requirements Document (PRD)

| | |
|---|---|
| **Product** | Crossthreads |
| **Doc type** | Product Requirements Document |
| **Status** | Draft v0.1 |
| **Last updated** | 2026-06-18 |
| **Owner** | Crossthreads maintainers |
| **Related docs** | [REQUIREMENTS.md](REQUIREMENTS.md) · [ARCHITECTURE.md](ARCHITECTURE.md) · [ROADMAP.md](ROADMAP.md) |

---

## 1. Summary

Crossthreads is a **local-first session indexer and memory layer for AI coding tools**. It auto-discovers, parses, and normalizes conversation history from Claude Code, Codex, Cursor, Aider, Gemini CLI, and other agents into a common schema, then exposes fast **hybrid search** (lexical + semantic + natural language) over the entire corpus. Beyond search, it turns history into **actionable outputs** — open the original session, export ready-to-paste context, inject context into a new agent, or resume work — and into a **durable memory** that agents themselves can query.

The wedge is simple: the built-in history/search in each tool is siloed and weak. Developers who switch tools daily have no reliable way to answer *"where's the thread where we implemented X?"* across their toolchain. Crossthreads makes that a one-line query.

## 2. Problem

Developers increasingly run **multiple coding agents** (Claude Code, Cursor, Codex, Aider, Gemini CLI, etc.) across the same projects. Each tool stores its session history in its own location and format, and offers only weak in-tool search. This produces concrete pain:

- **Fragmented memory.** Context from yesterday's Cursor session is invisible from inside Claude Code today.
- **No reliable recall.** "Find the thread where we fixed the OAuth refresh bug" is unanswerable across tools; even within a single tool, search is often just substring matching.
- **Lost decisions & rationale.** Why did we choose approach A over B? The reasoning is buried in a transcript no one can find.
- **Costly re-work.** Without recall, developers re-solve problems they already solved, or re-explain context to a fresh agent.
- **Weak handoff.** Resuming or transferring a piece of work to another tool/agent means manually copy-pasting context.

Existing OSS tools (e.g. CASS — `coding_agent_session_search`) prove the problem is solvable locally with filesystem access, but the bar for "meaningfully better than built-ins" — on UX, retrieval quality, memory depth, and actionability — is still wide open.

## 3. Goals & Non-Goals

### 3.1 Goals
- **G1 — Unified index.** Auto-discover and index sessions from the top coding agents into one normalized store.
- **G2 — Retrieval that beats grep.** Hybrid lexical + semantic search with reranking and natural-language query understanding.
- **G3 — Actionable results.** Every result supports open / export / inject / resume.
- **G4 — Local-first & private.** Default to fully local operation, no telemetry; sync/cloud strictly opt-in.
- **G5 — Agent-accessible.** Agents can query Crossthreads via CLI/JSON and MCP tools.
- **G6 — Resilient connectors.** Parsers are versioned and degrade gracefully when tool formats change.

### 3.2 Non-Goals (v1)
- Not a replacement for the coding agents themselves; we index and serve, we don't author code.
- Not a hosted multi-tenant SaaS at launch (team/cloud sync is post-MVP).
- Not a general-purpose note-taking app; scope is AI coding sessions.
- No proprietary cloud embedding requirement — cloud models are an optional fallback, never mandatory.

## 4. Target Users & Personas

| Persona | Description | Primary need |
|---|---|---|
| **Polyglot Power User** | Indie hacker / senior dev using 2–4 agents daily across many repos | Reliable cross-tool recall + resume |
| **Small Team Lead** | 3–10 person team wanting shared institutional knowledge | Shared/searchable decision history (post-MVP) |
| **The Agent Itself** | An AI coding agent that needs to recall prior context/decisions | Programmatic `search()` / `recall_decision()` |

### 4.1 Primary Jobs-to-be-Done
- "When I switch tools, let me **find and resume** the relevant past thread in one query."
- "Let me **recall a decision** and its rationale without reading whole transcripts."
- "Let me **hand off context** to a fresh agent without manual copy-paste."
- "Let my agent **query my own history** as a memory source."

## 5. Value Proposition

> **One place to search all your AI coding conversations — with intelligent retrieval and actionable outputs — that lives on your machine and gets smarter over time.**

Differentiators vs. built-ins and existing OSS:
1. **Cross-tool by default**, normalized into one schema.
2. **Retrieval quality** — hybrid search + reranking + NL query rewriting, not substring match.
3. **Actionability** — resume/handoff/inject, not just a results list.
4. **Memory layers** — episodic → semantic → procedural, improving over time.
5. **Polished UX** — TUI + optional desktop app, not just a CLI dump.

## 6. Scope

### 6.1 MVP (v1)
1. **Ingestion / connectors** for **Claude Code** (JSONL in `~/.claude/projects/`) and **Cursor** (SQLite `state.vscdb`), plus one of **Codex** or **Aider**. File-watch incremental indexing.
2. **Normalized schema** (conversations, messages, metadata) persisted in SQLite + FTS.
3. **Hybrid search** — BM25/FTS + local embeddings (ONNX/all-MiniLM-class) in a local vector store, with reranking. Filters: tool, project, date, model.
4. **Natural-language queries** via lightweight query rewriting + retrieval.
5. **Actions** — open in original tool, copy/export context (markdown), "resume here."
6. **Desktop app (Tauri)** as the primary interface; **CLI / JSON + agent API** alongside for scripting and programmatic/agent access.
7. **Deduplication** via content hashing; **incremental** re-index.

### 6.2 Fast-Follow (v1.x)
- **TUI** — keyboard-first terminal surface over the same core.
- MCP server exposing `search` / `recall` tools to agents.
- Timeline view + basic analytics.
- More connectors (Gemini CLI, Windsurf, Claude.ai web import).

### 6.3 Post-MVP (v2+)
- **Outcome tracking** — link sessions to git diffs/PRs/tests; surface "what worked."
- **Memory layers** — episodic → semantic (summarized lessons) → procedural (rules).
- **Multi-machine / team sync** — encrypted (git/rsync/self-hosted).
- **Proactive memory** — background summarization & suggestions.
- **Export ecosystem** — Obsidian, Notion, `AGENTS.md` / `CLAUDE.md` memory files.
- **Plugin system** for community connectors.

A full requirement-by-requirement breakdown lives in [REQUIREMENTS.md](REQUIREMENTS.md).

## 7. User Experience

### 7.1 Key flows
1. **First run / onboarding.** Auto-detect installed tools and history locations → show what was found → build initial index with progress → land on search.
2. **Search & act.** Type a query (keyword or NL) → ranked results with snippets/highlights and a context preview → pick an action (open / copy / export / resume).
3. **Resume / handoff.** From a result, generate a ready-to-paste context block or deep-link back into the originating tool/session.
4. **Agent recall.** An agent calls `crossthreads search "..."` (or MCP tool) and receives structured JSON results.

### 7.2 UX principles
- Sub-second perceived search on a typical corpus.
- Keyboard-first TUI; nothing requires a mouse.
- Always show provenance (which tool/project/session/timestamp).
- Privacy is visible and controllable (what's indexed, where, how to purge).

## 8. Success Metrics

| Category | Metric | Target (first 6 months) |
|---|---|---|
| **Adoption** | GitHub stars | 1k+ |
| | Weekly active installs (opt-in, anonymous count or self-reported) | Growing WoW |
| **Activation** | % users who index ≥2 tools | ≥ 60% |
| | % who run ≥3 searches in week 1 | ≥ 50% |
| **Retrieval quality** | Top-3 result is "useful" (in-app thumbs / eval set) | ≥ 80% |
| | Median search latency (warm) | < 500 ms |
| **Engagement** | Resume/export/inject actions per active user / week | ≥ 3 |
| **Reliability** | Connector parse success rate | ≥ 99% of sessions |
| **Trust** | Zero data egress in local-only mode | 100% (verifiable) |

## 9. Competitive Landscape

| Tool | Strengths | Gaps Crossthreads exploits |
|---|---|---|
| **Built-in tool history** (Claude Code, Cursor, …) | In-context, zero setup | Siloed, weak search, no cross-tool |
| **CASS** (`coding_agent_session_search`) | Fast local Rust TUI/CLI, 20+ agents, hybrid search, sync | UX depth, actionability, memory layers, desktop polish |
| **Orca Build** | Strong session history, search, drag-to-resume | Cross-tool breadth, agent API, local-first memory |
| **cass-memory / gbrain / AthenaCode** | Procedural memory experiments | Unified product UX, connector breadth |

**Positioning:** the most *actionable*, *polished*, and *cross-tool* of the local-first session memory tools — optionally building on/interoperating with CASS-style indexing rather than competing on raw indexing speed alone.

## 10. Go-to-Market

- **Open-source core** (MIT or Apache-2.0) — indexer + search engine + TUI, to earn trust and contributors (mirrors CASS's traction).
- **Monetization (later):** freemium desktop app (advanced memory/analytics paid); team SaaS tier (hosted/synced index, sharing, audit logs, SSO); possible one-time desktop license.
- **Distribution:** GitHub releases, Homebrew/Scoop, Product Hunt, Hacker News, and direct engagement with the originating X/Grok thread community and dev forums.
- **Wedge audience:** frustrated multi-tool power users (high willingness-to-pay); expand to teams/enterprise (compliance/security add-ons).

## 11. Risks & Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| **Parser breakage** on tool updates | Sessions silently missing | Versioned connectors, schema-fingerprint detection, graceful degradation, community connectors, regression corpus tests |
| **Privacy concerns** | Adoption blocker | Local-only default, no telemetry, explicit data controls, easy purge, auditable network behavior |
| **Embedding cost/perf locally** | Slow/heavy indexing | Quantized ONNX models, incremental + background indexing, configurable model size, optional cloud fallback |
| **Competition / commoditization** | Hard to differentiate | Compete on UX, actionability, memory depth, connector breadth — not raw speed |
| **Scope creep** | Slow MVP | Strict MVP gate (3–4 tools, hybrid search, desktop app, agent API) before v2 features |
| **Clean-build connector burden** | Higher upfront cost, sole maintenance of parsers | Port proven extraction patterns from CASS/public extractors, versioned connectors + regression corpus, plugin system (P2) to crowdsource drift |
| **Corpus scale** (large/old histories) | Index bloat, slow queries | Chunking, dedup, pruning/retention controls, optional Postgres backend |

## 12. Open Questions

**Resolved** (see §13 Decisions): engine strategy, primary surface, embedding default, license, process model, ML runtime, vector store, and 3rd connector are decided.

Still open:

1. **Resume mechanics:** how deep can we deep-link back into each tool's session vs. only export context? (Per-tool capability varies — a research task, tracked in [AGENT_API.md](AGENT_API.md) §6.)
2. **Schema versioning & migration** strategy as connectors evolve.
3. **Team sync trust model:** git-based vs. self-hosted service; encryption & key management (P2).
4. **Minor / early-P1:** cross-encoder reranker timing, exact embedding model + chunk sizing, frontend↔daemon transport (Tauri command shim vs. loopback HTTP) — low-risk, settled in early P1.

## 13. Assumptions & Decisions

| Decision | Choice | Rationale | Date |
|---|---|---|---|
| **Name** | Crossthreads | Repo already named this | 2026-06-18 |
| **Engine strategy** | **Clean build, port patterns** | Own the core engine outright; study CASS's connector/extraction patterns but take no code/runtime dependency — avoids license entanglement and format-coupling, at the cost of more upfront work | 2026-06-18 |
| **Primary surface** | **Desktop-first (Tauri)** | Lead with a polished GUI as the adoption wedge; CLI + agent API ship alongside for scripting/agents; TUI demoted to fast-follow | 2026-06-18 |
| **Embeddings** | **Bundled ONNX (all-MiniLM-class), Ollama optional** | Zero-dependency local default for fast activation; Ollama as opt-in upgrade for power users | 2026-06-18 |
| **License** | **Apache-2.0** | Permissive + explicit patent grant; better for enterprise/team adoption and contributors | 2026-06-18 |
| **Platforms** | macOS + Linux first; Windows fast-follow | Wedge users' primary platforms | 2026-06-18 |
| **Core language** | Rust | Performance + single-binary distribution; pairs with Tauri | 2026-06-18 |
| **Process model** | **Background daemon (`crossthreadsd`)** | Single-writer index; desktop/CLI/MCP are thin clients over loopback — clean concurrency, always-on file-watching | 2026-06-19 |
| **ML runtime** | **Pure-Rust ONNX (`ort`/`candle`)** | Single binary, clean signing/distribution; no Python sidecar | 2026-06-19 |
| **Vector store** | **`sqlite-vec` (same SQLite DB)** | One store/file to back up, sync, purge; no second backend | 2026-06-19 |
| **3rd MVP connector** | **Aider** | Well-documented history format; lower-risk parse alongside Claude Code + Cursor | 2026-06-19 |

> Decisions are recorded in [DECISIONS.md](DECISIONS.md) (ADR log); remaining open questions in §12 are scoped to early P1 or P2.
