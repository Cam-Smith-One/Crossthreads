# Crossthreads — Detailed Requirements

| | |
|---|---|
| **Status** | Draft v0.1 |
| **Last updated** | 2026-06-18 |
| **Companion to** | [PRD.md](PRD.md) |

This document enumerates functional (FR) and non-functional (NFR) requirements with stable IDs, priorities, and acceptance criteria. Priorities use **MoSCoW**: **M**ust / **S**hould / **C**ould / **W**on't-yet. The **Phase** column maps to [ROADMAP.md](ROADMAP.md) (P1 = MVP, P1.x = fast-follow, P2 = post-MVP).

---

## 1. Ingestion & Connectors

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-ING-01 | **Auto-discovery.** On launch, detect installed agents by probing known history locations per OS (macOS/Linux first, Windows fast-follow) and report what was found. | Must | P1 |
| FR-ING-02 | **Claude Code connector.** Parse JSONL session files in `~/.claude/projects/<project>/*.jsonl`, extracting conversations, messages, roles, timestamps, model, tool calls, and code snippets. | Must | P1 |
| FR-ING-03 | **Cursor connector.** Read SQLite `state.vscdb` from `workspaceStorage/` and global storage (e.g. `~/Library/Application Support/Cursor/User/...` on macOS; platform equivalents on Win/Linux), extracting chat/composer sessions. | Must | P1 |
| FR-ING-04 | **Third connector (Codex or Aider).** Parse at least one additional agent's history at MVP, following CASS-style extraction patterns. | Must | P1 |
| FR-ING-05 | **Incremental indexing via file watchers.** Detect new/changed session files in near real-time (e.g. `notify`-style watcher) and index only deltas. | Must | P1 |
| FR-ING-06 | **Deduplication.** Identify duplicate conversations/messages via content hashing; never double-index the same content. | Must | P1 |
| FR-ING-07 | **Graceful degradation.** A malformed/unrecognized session must be skipped and logged without aborting the whole index run. | Must | P1 |
| FR-ING-08 | **Connector versioning.** Each connector declares a schema/format version and detects the source format fingerprint, so tool updates can be handled or flagged rather than silently mis-parsed. | Must | P1 |
| FR-ING-09 | **Additional connectors.** Gemini CLI, Windsurf, and others. | Should | P1.x |
| FR-ING-10 | **Claude.ai web import.** Import from official data exports and/or browser-based extraction. | Could | P1.x |
| FR-ING-11 | **Connector plugin system.** Allow community-authored connectors via a documented interface. | Could | P2 |
| FR-ING-12 | **Manual re-index / repair.** User can force a full re-index or repair the index from the UI/CLI. | Should | P1 |

**Acceptance (sample, FR-ING-02):** Given a real `~/.claude/projects` directory, indexing produces one normalized conversation per session file with ≥99% of messages captured, roles correctly attributed, and timestamps preserved; re-running indexes zero new items (dedup holds).

## 2. Normalization & Schema

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-SCH-01 | **Common schema.** Normalize all sources to a shared model: `Conversation` (id, tool, project/workspace, started_at, model, source_path, hash) and `Message` (id, conversation_id, role, content, timestamp, code_snippets[], tool_calls[], metadata). | Must | P1 |
| FR-SCH-02 | **Rich metadata capture.** Where available: workspace/project path, model name, linked files, and git context (branch/commit). | Must | P1 |
| FR-SCH-03 | **Chunking strategy.** Chunk content for retrieval by message turns and/or topic segments and/or code-change boundaries; store chunk→message→conversation lineage. | Must | P1 |
| FR-SCH-04 | **Stable IDs & provenance.** Every record traces back to its exact source file/offset for "open original." | Must | P1 |
| FR-SCH-05 | **Schema migrations.** Versioned schema with forward migrations; index survives app upgrades without full rebuild where possible. | Should | P1.x |

## 3. Storage & Indexing

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-STO-01 | **Metadata + full-text store.** SQLite with FTS (BM25-class lexical search) as the system of record for metadata and text. | Must | P1 |
| FR-STO-02 | **Vector store.** Local embeddings persisted in a local vector index (e.g. LanceDB/Chroma-class) for semantic search. | Must | P1 |
| FR-STO-03 | **Local embedding model.** Default to a bundled local model (ONNX, all-MiniLM-class) or local Ollama; no network required to index/search. | Must | P1 |
| FR-STO-04 | **Optional cloud embeddings.** Allow opt-in cloud embedding/reranking as a fallback, clearly gated behind explicit consent. | Could | P1.x |
| FR-STO-05 | **Scale backend.** Optional Postgres backend for large corpora/teams. | Could | P2 |
| FR-STO-06 | **Retention & pruning.** User-configurable retention (by age/size/tool) and the ability to purge specific tools/projects/sessions from the index. | Should | P1.x |
| FR-STO-07 | **Index integrity.** Detect and recover from a corrupted index (rebuild from source files). | Should | P1 |

## 4. Search & Query

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-SRCH-01 | **Hybrid search.** Combine lexical (FTS/BM25) and semantic (vector) results with a fusion/rerank step. | Must | P1 |
| FR-SRCH-02 | **Filters.** Filter results by tool, project/workspace, date range, and model; combinable with the query. | Must | P1 |
| FR-SRCH-03 | **Natural-language queries.** Support NL queries (e.g. "find where we implemented the auth retry") via lightweight query rewriting + retrieval; works with a small local LLM, degrades to hybrid search if no LLM is configured. | Must | P1 |
| FR-SRCH-04 | **Ranked results with context.** Return a ranked list with snippets, highlighted matches, a context preview, and provenance (tool/project/time). | Must | P1 |
| FR-SRCH-05 | **Reranking.** Apply a reranker over fused candidates to improve top-k relevance. | Should | P1 |
| FR-SRCH-06 | **Success/outcome filters.** Filter/boost by success indicators (e.g. session linked to merged PR/passing tests). | Could | P2 |
| FR-SRCH-07 | **Saved searches / history.** Persist recent and saved queries. | Could | P1.x |

**Acceptance (sample, FR-SRCH-01/03):** On a labeled eval set of NL queries against a seeded corpus, the intended thread appears in the top-3 for ≥80% of queries; disabling the semantic side measurably lowers recall (proving hybrid contributes).

## 5. Results & Actions

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-ACT-01 | **Open original.** Open/deep-link the result back into its originating tool/session where the tool supports it; otherwise reveal the source file. | Must | P1 |
| FR-ACT-02 | **Export context (markdown).** Export a conversation or selected range to clean markdown. | Must | P1 |
| FR-ACT-03 | **Copy-as-prompt / inject.** Produce a ready-to-paste context block for a new agent prompt; optionally inject into a new session. | Must | P1 |
| FR-ACT-04 | **Resume here.** One action to resume work from a result — deep-link resume where supported, else exported handoff context. | Should | P1.x |
| FR-ACT-05 | **HTML export.** Export sessions/results to a shareable HTML view. | Could | P1.x |
| FR-ACT-06 | **Export ecosystem.** Push to Obsidian/Notion and to project memory files (`AGENTS.md` / `CLAUDE.md`). | Could | P2 |

## 6. Interfaces (TUI / CLI / Desktop / Agent API)

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-UI-01 | **TUI.** Keyboard-first terminal UI for search, filtering, preview, and actions as the primary MVP surface. | Must | P1 |
| FR-UI-02 | **CLI / JSON mode.** Scriptable CLI with structured JSON output for all core operations (index, search, export). | Must | P1 |
| FR-UI-03 | **Agent API.** A programmatic interface agents can call, e.g. `search(query, filters)` and `recall_decision(topic)`, returning structured results. | Must | P1 |
| FR-UI-04 | **MCP server.** Expose search/recall as MCP tools for direct use inside agents that support MCP. | Should | P1.x |
| FR-UI-05 | **Desktop app.** Tauri (Rust backend + web frontend, shadcn/ui-style) for a polished GUI. | Should | P1.x |
| FR-UI-06 | **Timeline & analytics.** Timeline view and activity analytics (by tool/project, frequent topics). | Could | P1.x |
| FR-UI-07 | **Web dashboard.** Dashboard over synced data (team context). | Won't-yet | P2 |

## 7. Memory Layers & Intelligence (Post-MVP)

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-MEM-01 | **Episodic layer.** Raw sessions as the base memory layer (this is the MVP index). | Must | P1 |
| FR-MEM-02 | **Semantic layer.** Background summarization into "lessons learned" / decisions, queryable separately. | Could | P2 |
| FR-MEM-03 | **Procedural layer.** Distill rules/patterns that succeeded; surface as reusable guidance. | Could | P2 |
| FR-MEM-04 | **Outcome tracking.** Link sessions to git diffs/PRs/test results; surface "what worked" patterns. | Could | P2 |
| FR-MEM-05 | **Proactive suggestions.** "You discussed similar auth logic in Session Z" while working. | Won't-yet | P2 |
| FR-MEM-06 | **Knowledge graph / visualization.** Conversation flow graphs and decision timelines. | Won't-yet | P2 |

## 8. Sync & Collaboration (Post-MVP)

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-SYNC-01 | **Multi-machine sync.** Encrypted sync of the index across a user's machines (git/rsync/SSH or self-hosted). | Could | P2 |
| FR-SYNC-02 | **Team sharing.** Shared index/knowledge with access controls. | Won't-yet | P2 |
| FR-SYNC-03 | **Audit logs / SSO.** Enterprise controls for team/SaaS tier. | Won't-yet | P2 |

## 9. Non-Functional Requirements

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| NFR-PRIV-01 | **Local-only by default.** No data leaves the machine unless the user explicitly enables sync/cloud. Behavior is auditable (no hidden network calls). | Must | P1 |
| NFR-PRIV-02 | **No telemetry by default.** Any analytics are opt-in and anonymous. | Must | P1 |
| NFR-PRIV-03 | **Data controls.** Clear visibility into what is indexed and where it is stored; one-command purge of all indexed data. | Must | P1 |
| NFR-PERF-01 | **Search latency.** Median warm search < 500 ms on a typical corpus (target tens of thousands of messages). | Must | P1 |
| NFR-PERF-02 | **Incremental indexing.** New sessions are searchable within seconds of being written. | Should | P1 |
| NFR-PERF-03 | **Resource footprint.** Indexing must be throttleable/backgroundable to avoid harming foreground dev work. | Should | P1 |
| NFR-REL-01 | **Connector robustness.** ≥99% of well-formed sessions parse successfully; failures are isolated and logged. | Must | P1 |
| NFR-REL-02 | **Crash safety.** Index is not corrupted by interrupted runs; recoverable from source. | Must | P1 |
| NFR-PORT-01 | **Platforms.** macOS + Linux at MVP; Windows fast-follow. | Must | P1 |
| NFR-SEC-01 | **At-rest protection (sync).** Synced data is encrypted; keys are user-controlled. | Should | P2 |
| NFR-MAINT-01 | **Extensibility.** Connectors and embedding models are pluggable behind stable interfaces. | Should | P1.x |
| NFR-LIC-01 | **OSS core.** Core indexer/search/TUI released under a permissive license (MIT/Apache-2.0 — TBD). | Must | P1 |
| NFR-OBS-01 | **Diagnostics.** Local logs/metrics for index health, connector status, and search performance (no egress). | Should | P1 |

## 10. Traceability

Every requirement above maps to a PRD goal:

- **G1 Unified index** → FR-ING-*, FR-SCH-*, FR-STO-*
- **G2 Retrieval beats grep** → FR-SRCH-*
- **G3 Actionable results** → FR-ACT-*
- **G4 Local-first & private** → NFR-PRIV-*, NFR-SEC-*
- **G5 Agent-accessible** → FR-UI-02/03/04
- **G6 Resilient connectors** → FR-ING-07/08/11, NFR-REL-*

## 11. Acceptance Gate for MVP (v1)

The MVP ships when all **Must / P1** requirements pass acceptance, specifically:
1. Three working connectors (Claude Code, Cursor, + one) with incremental indexing and dedup.
2. Hybrid search with filters and NL queries meeting FR-SRCH acceptance (≥80% top-3 on the eval set).
3. Open / export / inject actions functional.
4. TUI + CLI/JSON + agent API operational.
5. Local-only, no-telemetry, purgeable — verified (NFR-PRIV-*).
6. Search latency and connector robustness targets met (NFR-PERF-01, NFR-REL-01).
