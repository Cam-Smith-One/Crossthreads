# Crossthreads — Agent API & Interface Spec

| | |
|---|---|
| **Status** | Draft v0.1 (design sketch, pre-implementation) |
| **Last updated** | 2026-06-18 |
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
  "rerank": true        // default true
}
```

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
| `crossthreads.search` | §2 |
| `crossthreads.recall` | §3 |
| `crossthreads.get_conversation` | §4 |
| `crossthreads.build_context` | §5 |
| `crossthreads.resume` | §6 |
| `crossthreads.list_facets` | filter discovery |
| `crossthreads.status` | index/connector health |

Tool input schemas mirror the request bodies above; outputs mirror the responses. Tools are **read-only** by default (no mutation of indexed data) — a safe surface to expose to an agent.

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
