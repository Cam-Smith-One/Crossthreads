# Crossthreads — Agent API & Interface Spec

| | |
|---|---|
| **Status** | Draft v0.1 (design sketch, pre-implementation) |
| **Last updated** | 2026-06-24 |
| **Companion to** | [PRD.md](PRD.md) · [REQUIREMENTS.md](REQUIREMENTS.md) · [ARCHITECTURE.md](ARCHITECTURE.md) |
| **Covers** | FR-UI-02 (CLI/JSON), FR-UI-03 (Agent API), FR-UI-04 (MCP), FR-ACT-01..04 |

This is a lightweight contract for how **agents and scripts** talk to Crossthreads. Three surfaces share **one core** and the **same result schema**:

1. **CLI / JSON** — `crossthreads <cmd> --json` (FR-UI-02)
2. **MCP server** — `crossthreads mcp` exposing tools to MCP-capable agents (FR-UI-04)
3. **Local HTTP** (optional) — `crossthreads serve` on loopback for non-MCP integrations

> Design rule: **the MCP tools, CLI `--json`, and HTTP endpoints are thin adapters over the same internal API and return the identical `SearchResult` / `Conversation` shapes.** Document and version the shapes once.

---

## 1. Core operations

| Operation | Purpose | CLI | MCP tool |
|---|---|---|---|
| **search** | Hybrid search over all sessions | `crossthreads search` | `crossthreads.search` |
| **recall** | NL "find the decision/thread about X" → best answer + sources | `crossthreads recall` | `crossthreads.recall` |
| **get** | Fetch a full conversation (or range) by id | `crossthreads get` | `crossthreads.get_conversation` |
| **context** | Build a ready-to-paste context block for a new prompt | `crossthreads context` | `crossthreads.build_context` |
| **resume** | Resume/handoff target for a result | `crossthreads resume` | `crossthreads.resume` |
| **list** | Enumerate tools/projects/models for filters | `crossthreads list` | `crossthreads.list_facets` |
| **status** | Index health, connector status, counts | `crossthreads status` | `crossthreads.status` |

All commands accept `--json` for machine output; MCP tools always return structured JSON.

---

## 2. `search`

Hybrid lexical + semantic search with optional filters and reranking (FR-SRCH-01..05).

### Request
```jsonc
{
  "query": "auth retry logic with token refresh",
  "filters": {
    "tool":    ["claude-code", "cursor"],   // optional
    "project": "~/code/acme-api",            // optional, path or repo name
    "model":   ["claude-opus-4-8"],          // optional
    "after":   "2026-05-01",                 // optional ISO date/datetime
    "before":  "2026-06-18"                  // optional
  },
  "mode":  "hybrid",   // "hybrid" | "lexical" | "semantic"  (default: hybrid)
  "limit": 10,          // default 10, max 100
  "rerank": true,       // default true
  "devices": ["linux-desktop"]  // optional cross-device routing (ADR-010);
                                //   omit to search this device + all reachable peers
}
```

Cross-device (ADR-010): when federation is configured, `search` fans out to peer
daemons and merges the results; each result carries a `device` field naming its
origin. `devices` restricts which devices are queried (names from the `devices`
op / `crossthreads_devices` tool). `build_context` and `recall` span devices the
same way.

### Response
```jsonc
{
  "query": "auth retry logic with token refresh",
  "took_ms": 142,
  "results": [
    {
      "conversation_id": "cv_8f3a...",
      "chunk_id":        "ck_19b2...",
      "score":           0.87,            // fused/reranked score, 0..1
      "tool":            "claude-code",
      "project":         "~/code/acme-api",
      "model":           "claude-opus-4-8",
      "started_at":      "2026-05-14T09:12:00Z",
      "title":           "Fix OAuth refresh 401 loop",
      "snippet":         "...we wrapped the refresh in a backoff and **retried** on 401...",
      "highlights":      ["retry", "token refresh"],
      "source": {
        "path":   "~/.claude/projects/acme-api/2026-05-14.jsonl",
        "offset": 10421                    // for "open original"
      },
      "actions": ["open", "export", "context", "resume"]
    }
  ]
}
```

### CLI
```bash
crossthreads search "auth retry logic" --tool claude-code,cursor \
  --project ~/code/acme-api --after 2026-05-01 --limit 5 --json
```

---

## 3. `recall`

Higher-level than `search`: takes an NL question, runs retrieval, and (when a local LLM is configured) returns a **synthesized answer with citations**. Degrades to top search results when no LLM is available (FR-SRCH-03).

### Request
```jsonc
{ "question": "why did we choose Postgres over SQLite for the worker queue?",
  "filters": { "project": "~/code/acme-api" },
  "max_sources": 5 }
```

### Response
```jsonc
{
  "answer": "You moved the worker queue to Postgres on 2026-05-20 to get SKIP LOCKED...",
  "confidence": "high",         // "high" | "medium" | "low" | "no_answer"
  "sources": [
    { "conversation_id": "cv_77c1...", "tool": "cursor",
      "started_at": "2026-05-20T15:02:00Z", "snippet": "...SKIP LOCKED gives us...",
      "source": { "path": "...state.vscdb", "offset": null } }
  ],
  "llm_used": true
}
```
> When `llm_used: false`, `answer` is omitted and `sources` carries the ranked hits — the caller (often an agent) does its own synthesis.

---

## 4. `get_conversation`

Fetch a full normalized conversation (FR-ACT-01, FR-SCH-01).

### Request
```jsonc
{ "conversation_id": "cv_8f3a...", "range": { "from_message": 0, "to_message": 40 } }
```

Cross-device (ADR-010): pass the result's `device` to fetch a transcript that
lives on another machine — `{ "id": "cv_8f3a...", "device": "linux-desktop" }`.
The owning peer serves it (token-gated), subject to that device's serve-scope.

### Response — normalized `Conversation` (matches ARCHITECTURE §4)
```jsonc
{
  "id": "cv_8f3a...",
  "tool": "claude-code",
  "project": "~/code/acme-api",
  "model": "claude-opus-4-8",
  "started_at": "2026-05-14T09:12:00Z",
  "git_context": { "branch": "fix/oauth", "commit": "a1b2c3d" },
  "messages": [
    { "id": "ms_001", "role": "user", "content": "...", "timestamp": "...",
      "code_snippets": [], "tool_calls": [] }
  ],
  "source": { "path": "...", "fingerprint": "claude-code/v2" }
}
```

---

## 5. `build_context` (inject)

Produce a compact, ready-to-paste context block for a new agent prompt (FR-ACT-03). The key memory primitive for agent workflows.

### Request
```jsonc
{
  "from": { "conversation_id": "cv_8f3a..." },   // or { "query": "...", "top_k": 3 }
  "format": "markdown",        // "markdown" | "xml" | "plain"
  "max_tokens": 2000,          // budget; server summarizes/truncates to fit
  "include": ["decisions", "code", "summary"]   // sections to emphasize
}
```

### Response
```jsonc
{
  "context": "## Prior context (Crossthreads)\n\n**Decision:** ...\n\n```python\n...\n```\n",
  "token_estimate": 1840,
  "sources": ["cv_8f3a...", "cv_77c1..."],
  "truncated": false
}
```

### Typical agent loop
```
1. agent calls crossthreads.recall / search to find relevant prior work
2. agent calls crossthreads.build_context to get a token-budgeted block
3. agent prepends that block to its working prompt and continues
```

---

## 6. `resume`

Return a resume/handoff target for a result (FR-ACT-04). Capability is **per-tool**; the response declares what's possible.

### Response
```jsonc
{
  "conversation_id": "cv_8f3a...",
  "method": "deeplink",                 // "deeplink" | "reopen_file" | "context_only"
  "deeplink": "cursor://...",           // present when method == deeplink
  "fallback_context": "## Resume...\n", // always present: paste-able handoff
  "instructions": "Open in Cursor, or paste fallback_context into a new session."
}
```
> If a tool can't be deep-linked, `method` is `context_only` and the agent/user uses `fallback_context`. This keeps the contract honest about per-tool limits (PRD Open Q4).

---

## 7. MCP server

`crossthreads mcp` (stdio) registers these tools so MCP-capable agents (e.g. Claude Code) call them natively:

| MCP tool | Maps to |
|---|---|
| `crossthreads_search` | §2 — **implemented** |
| `crossthreads_recall` | §3 — **implemented** (retrieval-only digest) |
| `crossthreads_build_context` | §5 — **implemented** |
| `crossthreads_status` | index health — **implemented** |
| `crossthreads_devices` | list cross-device search targets — **implemented** |
| `crossthreads_themes` | cluster sessions into themes (`k`, `name` args) — **implemented** |
| `crossthreads_ask` | cited answer to a question, synthesized from history — **implemented** |
| `crossthreads_activity` | session activity over time (day/week) — **implemented** |
| `crossthreads_graph` | knowledge graph (projects/tools/tags + edges) — **implemented** |
| `crossthreads_metrics` | behavioral metrics (how you work; deterministic) — **implemented** |
| `crossthreads_work_dna` | quantified profile of how the user works — **implemented** |
| `crossthreads_gaps` | gaps/blind spots + draft CLAUDE.md — **implemented** |
| `crossthreads_prompt_coach` | prompt coaching from smoothest vs roughest sessions — **implemented** |
| `crossthreads_process_miner` | repeatable procedures → draft skills — **implemented** |
| `crossthreads_open_loops` | unresolved work across recent sessions — **implemented** |
| `crossthreads_knowledge_cards` | durable Q→A cards worth remembering — **implemented** |
| `crossthreads_decision_log` | notable decisions + rationale — **implemented** |
| `crossthreads_how_i_work` | the user's working conventions (for CLAUDE.md/AGENTS.md) — **implemented** |
| `crossthreads_recurring` | recurring problems/patterns, with fixes — **implemented** |
| `crossthreads_digest` | short reflective digest of recent work — **implemented** |
| `crossthreads_devices` | list searchable devices (this host + peers) with liveness — **implemented** (ADR-010) |
| `crossthreads.get_conversation` | §4 — daemon op available (`GetConversation`) |
| `crossthreads.resume` | §6 — planned |
| `crossthreads.list_facets` | filter discovery — planned |

> Implemented tools live in `ct-mcp`, which forwards to `crossthreadsd` and
> **auto-starts it** on first use (reusing one already running — only ever one
> writer per index), so registering the absolute path to `ct-mcp` is the only
> setup step. Tool names use underscores (`crossthreads_search`) to satisfy MCP's
> name charset.

Tool input schemas mirror the request bodies above; outputs mirror the responses. Tools are **read-only** by default (no mutation of indexed data) — a safe surface to expose to an agent.

### 7.1 Agent skill

`crossthreads skill install` writes a workflow skill that nudges agents to use
the MCP tools at the right moments (recall at task start, search before
re-solving):

- **Claude Code:** `<~/.claude>/skills/crossthreads/SKILL.md` (loads
  automatically; `CLAUDE_CONFIG_DIR` overrides the location).
- **Codex:** `<~/.codex>/prompts/crossthreads.md`, invoked as `/crossthreads`
  (`CODEX_HOME` overrides).

Flags: `--claude` / `--codex` to install one, `--force` to overwrite. The skill
text is bundled in the binary; the canonical sources live in `assets/skills/`.

### 7.2 Insights (LLM synthesis)

The `crossthreads_*` insight tools (and `crossthreads insight <kind>` on the CLI,
`Request::Insight { kind, limit }` on the daemon) synthesize a high-level view
over the user's recent sessions with their configured model. One shared engine
(`ct-daemon::insights`) backs all surfaces; `kind` is one of:

| kind | tool | what it returns |
|---|---|---|
| `open_loops` | `crossthreads_open_loops` | unresolved work, dangling TODOs, unconfirmed fixes |
| `knowledge_cards` | `crossthreads_knowledge_cards` | durable Q→A cards worth remembering |
| `decision_log` | `crossthreads_decision_log` | decisions ("chose X over Y because Z") + rationale |
| `how_i_work` | `crossthreads_how_i_work` | the user's conventions, for a CLAUDE.md / AGENTS.md |
| `recurrence` | `crossthreads_recurring` | recurring problems/patterns, each with a fix |
| `digest` | `crossthreads_digest` | a short reflective digest of recent work |

These need a model login (Settings → Models / `crossthreads llm-auth`); without
one they return a clear hint. The corpus spans the user's **whole history** —
every tool *and* their skills/prompts, not just recent chat threads — reading a
large recent subset (200 records for `open_loops`/`digest`, 400 for the others)
bounded to ~300k chars. Override the count with `limit` (CLI `--limit N`, or the
tools' `limit` arg). The daemon gathers the text under the read lock, releases
it, then calls the model — searches stay responsive during synthesis. Output is
Markdown plus the source conversation ids drawn from.

### 7.3 Ask, activity & graph

- **`ask`** (`crossthreads_ask`, CLI `crossthreads ask`, daemon `Request::Ask`) —
  retrieval-augmented Q&A over the user's history: hybrid-search the most relevant
  sessions (across tools/devices), then synthesize a **cited** answer. Returns
  `{ markdown, sources, llm_used }`. With a model it synthesizes; without one
  `llm_used` is `false` and `markdown` is the retrieved context (the caller can
  synthesize), mirroring `recall`.
- **`activity`** (`crossthreads_activity`, CLI `crossthreads activity`, daemon
  `Request::Activity`) — session counts bucketed by `day`|`week` with a per-tool
  breakdown: `{ bucket, periods: [{ period, total, by_tool }] }`. Offline.
- **`graph`** (`crossthreads_graph`, CLI `crossthreads graph`, daemon
  `Request::Graph`) — a deterministic knowledge graph: `{ nodes: [{ id, label,
  kind, weight }], edges: [{ source, target, weight }] }` over projects, tools,
  and tags. Offline.

### 7.4 Behavioral insights ("how you work")

A two-layer design: a **deterministic metrics pass** over the user's *own*
messages and interaction shapes (no model), interpreted by **model-backed
products** so they cite real numbers instead of guessing.

- **`metrics`** (`crossthreads_metrics`, CLI `crossthreads metrics`, daemon
  `Request::Metrics`) — `WorkMetrics`: turns per task (median/mean), correction
  & first-prompt-miss rates, abandonment, opening-prompt specificity, code/error
  paste rates, test/commit mention rates, tempo by hour, highest-friction
  projects. Offline. This is the backbone the four products below are grounded in.
- **`work_dna`** — a quantified profile of how the user works.
- **`gaps`** — missing practices, unfinished work, and a draft `CLAUDE.md`.
- **`prompt_coach`** — smoothest vs roughest sessions (selected by friction) with
  before→after rewrites from the user's own prompts.
- **`process_miner`** — clusters of repeated work mined into draft `SKILL.md`s.

The four products are insight kinds (daemon `Request::Insight { kind }`, the
`crossthreads_*` tools, CLI `crossthreads insight <kind>`); they gather metrics
under the store lock, drop it, then call the model.

---

## 8. Conventions

- **Auth/scope:** local surfaces (CLI, MCP stdio, loopback HTTP) assume the local user; no auth at MVP. Network/team auth is a P2 concern.
- **Errors:** `{ "error": { "code": "not_found" | "bad_request" | "index_unavailable" | "llm_unavailable", "message": "..." } }`.
- **Stability:** request/response shapes are **versioned** (`x-crossthreads-api: 1`); additive changes are non-breaking, removals bump the version.
- **Determinism:** `mode: "lexical"` is fully deterministic for scripting; `hybrid`/`semantic`/`recall` may vary with model/index state.
- **Privacy:** every operation runs locally; `recall` only calls a cloud LLM if the user explicitly opted in (NFR-PRIV-01).

## 9. Open questions (tracked in PRD §12)

- **Resume depth** per tool (§6 `method`) — which tools support real deep-links vs. context-only.
- **`recall` LLM default** — bundled small local model vs. require user-configured (Ollama) vs. cloud opt-in.
- **HTTP surface** — ship at MVP or MCP+CLI only first.
