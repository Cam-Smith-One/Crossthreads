# Crossthreads — Agent API & Interface Spec

| | |
|---|---|
| **Status** | Implemented; sections 7.5–7.8 describe the current daemon (0.21.0) |
| **Last updated** | 2026-11-16 |
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
| `crossthreads_threads` | the user's threads, most recently active first — **implemented** |
| `crossthreads_thread` | one thread with its sessions and links — **implemented** |
| `crossthreads_activity` | session activity over time (day/week) — **implemented** |
| `crossthreads_metrics` | behavioral metrics (how you work; deterministic) — **implemented** |
| `crossthreads_optimize` | biggest change to make now + measured verdict on the last — **implemented** |
| `crossthreads_trends` | metric trends (last 30d vs previous) — **implemented** |
| `crossthreads_fluency` | 4D AI-fluency scored (delegation/description/discernment/diligence) + a reflective question — **implemented** |
| `crossthreads_weekly_review` | proactive weekly review (cached; `force` to regen) — **implemented** |
| `crossthreads_work_dna` | quantified profile of how the user works — **implemented** |
| `crossthreads_gaps` | gaps/blind spots + draft CLAUDE.md — **implemented** |
| `crossthreads_prompt_coach` | prompt coaching from smoothest vs roughest sessions — **implemented** |
| `crossthreads_process_miner` | repeatable procedures → draft skills — **implemented** |
| `crossthreads_open_loops` | unresolved work across recent sessions — **implemented** |
| `crossthreads_knowledge_cards` | durable Q→A cards worth remembering — **implemented** |
| `crossthreads_decision_log` | notable decisions + rationale — **implemented** |
| `crossthreads_recurring` | recurring problems/patterns, with fixes — **implemented** |
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
| `recurrence` | `crossthreads_recurring` | recurring problems/patterns, each with a fix |

These need a model login (Settings → Models / `crossthreads llm-auth`); without
one they return a clear hint. The corpus spans the user's **whole history** —
every tool *and* their skills/prompts, not just recent chat threads — reading a
large recent subset (200 records for `open_loops`, 400 for the others)
bounded to ~300k chars. Override the count with `limit` (CLI `--limit N`, or the
tools' `limit` arg). The daemon gathers the text under the read lock, releases
it, then calls the model — searches stay responsive during synthesis. Output is
Markdown plus the source conversation ids drawn from.

### 7.3 Ask & activity

- **`ask`** (`crossthreads_ask`, CLI `crossthreads ask`, daemon `Request::Ask`) —
  retrieval-augmented Q&A over the user's history: hybrid-search the most relevant
  sessions (across tools/devices), then synthesize a **cited** answer. Returns
  `{ markdown, sources, llm_used }`. With a model it synthesizes; without one
  `llm_used` is `false` and `markdown` is the retrieved context (the caller can
  synthesize), mirroring `recall`.
- **`activity`** (`crossthreads_activity`, CLI `crossthreads activity`, daemon
  `Request::Activity`) — session counts bucketed by `day`|`week` with a per-tool
  breakdown: `{ bucket, periods: [{ period, total, by_tool }] }`. Counts sessions
  only (not skills or room exchanges) unless a `kind` filter says otherwise. Offline.

### 7.4 Behavioral insights ("how you work")

A two-layer design: a **deterministic metrics pass** over the user's *own*
messages and interaction shapes (no model), interpreted by **model-backed
products** so they cite real numbers instead of guessing.

- **`metrics`** (`crossthreads_metrics`, CLI `crossthreads metrics`, daemon
  `Request::Metrics`) — `WorkMetrics`: turns per task (median/mean), correction
  & first-prompt-miss rates, abandonment, opening-prompt specificity, code/error
  paste rates, test/commit mention rates, **delegation** (agent actions/session +
  top actions), **languages**, **model mix**, **median session minutes**, tempo
  by hour, highest-friction projects. Offline. This is the backbone the four
  products below are grounded in. (Agent-action names are persisted at index time
  — schema v8; re-index to backfill.)
- **`weekly_review`** (`crossthreads_weekly_review`, CLI `crossthreads weekly`,
  daemon `Request::WeeklyReview { force }`) — a proactive review of the last 7
  days: what you worked on, how you worked (cites the metrics), and one concrete
  thing to try. Cached in the index (`meta_kv`) and regenerated when a week
  stale; the daemon refreshes it in the background on startup. Degrades to the
  week's metrics with no model. **Delivery:** `crossthreads weekly --deliver`
  writes the review to `<data dir>/crossthreads/reviews/weekly-<period>.md` and
  fires a desktop notification (per-period dedup); `crossthreads schedule
  install|uninstall|status` (a launchd agent on macOS, a managed crontab block
  elsewhere; `--day`/`--hour` cadence) runs that delivery weekly. CLI, op, MCP,
  and the scheduler all route through one `weekly::ensure_current`.
- **`optimize`** (`crossthreads_optimize`, CLI `crossthreads optimize`, daemon
  `Request::Optimize { force }`) — the closed "how you work" loop
  (`ct-daemon::optimize`): score a catalog of behavioral *levers* by expected
  impact, prescribe the single biggest one and persist it, and on the next run
  **grade** whether the targeted metric moved vs its baseline (validation-gated
  verdict) into an optimization log. Deterministic; state in `meta_kv`.
- **`trends`** (`crossthreads_trends`, CLI `crossthreads trends`, daemon
  `Request::Trends { days }`) — each tracked metric over the last window vs the
  previous, with a `improved` direction flag. Deterministic.
- **`fluency`** (`crossthreads_fluency`, CLI `crossthreads fluency`, daemon
  `Request::Fluency { window_days }`) — the same signals organized around the
  **4D AI-Fluency framework** (`ct-daemon::fluency`): Delegation, Description,
  Discernment, Diligence, each **scored 0–100** with its movement vs. the
  previous window, the component metrics that fed it, and a plain-language read.
  Ends on a **reflective question, not a prescription** (`reflection`) — the
  agency/intentionality complement to `optimize`. Deterministic; no state.
- **`glance`** (daemon `Request::Glance`, CLI `crossthreads glance`, powering the
  desktop tray popover; `ct-daemon::glance`) — a single cheap bundle that fuses
  **usage** (per-tool session volume today vs. your baseline + this week) with the
  **insight read** (fluency score + weakest dimension, unresolved-thread count,
  the optimize focus). Each provider's `quota` window is read from a documented
  sidecar — `~/.crossthreads/quota/<tool>.json` (or `$CROSSTHREADS_QUOTA_DIR`),
  shaped like `QuotaWindow` — that an on-device reader writes; Crossthreads never
  scrapes provider internals, and stale/malformed/missing files show nothing.
  Deterministic; read-only. Not an agent tool — a UI/CLI surface.
- **`resume`** (daemon `Request::Resume { id }`, CLI `crossthreads resume <ID>`;
  `ct-daemon::resume`) — the native way to pick a session back up: a runnable
  command for tools that have one (Claude Code `claude --resume <id>`, Codex
  `codex resume <uuid>`, prefixed with `cd <project>`), else reveal the
  transcript. One source of truth for the CLI, the daemon op, and the web UI's
  resume button. Deterministic; read-only.
- **`work_dna`** — a quantified profile of how the user works.
- **`gaps`** — missing practices, unfinished work, and a draft `CLAUDE.md`.
- **`prompt_coach`** — smoothest vs roughest sessions (selected by friction) with
  before→after rewrites from the user's own prompts.
- **`process_miner`** — clusters of repeated work mined into draft `SKILL.md`s.

The four products are insight kinds (daemon `Request::Insight { kind }`, the
`crossthreads_*` tools, CLI `crossthreads insight <kind>`); they gather metrics
under the store lock, drop it, then call the model.

---

### 7.5 Threads

A **thread** is a piece of work: one or more sessions linked across tools
([SPEC.md](SPEC.md) §6.1). Every session is in exactly one thread; a session
nobody has linked is a thread of one. All of this is deterministic and offline.

- **`threads`** (`crossthreads_threads`, CLI `crossthreads threads`, daemon
  `{"op":"threads","filters":{"status"?,"tool"?,"repo"?,"limit"?}}`) →
  `{"type":"threads","threads":[ThreadSummary]}`, most recently active first.
  `ThreadSummary` = `{id, title, status, repo_key, repo_label, tools[],
  branches[], session_count, message_count, first_activity, last_activity,
  pending_suggestions}`.
- **`thread`** (`crossthreads_thread`, CLI `crossthreads thread <ID>`, daemon
  `{"op":"thread","id":…}`) → `{"type":"thread","thread":ThreadDetail|null}`.
  `id` is a thread id or the conversation id of any session in it.
  `ThreadDetail` is a `ThreadSummary` plus `sessions[]` (oldest first; each has
  a `conversation_id` for `get_conversation` / `resume`), `links[]`
  (`{from, to, kind, origin, confidence}`) and `suggestions[]` (links proposed
  but not applied).
- **Editing** (daemon ops; CLI `crossthreads thread link|unlink|reject|rename|status`):
  `link_sessions {a, b, kind?}` puts two sessions in one thread and is how a
  suggestion is accepted; `unlink_session {id}` returns a session to a thread
  of one; `reject_link {a, b}` dismisses a suggestion for good;
  `set_thread {id, title?, status?}` renames (empty title = derived) or sets
  `active` | `parked` | `done`. `link_sessions` and `set_thread` answer
  `{"type":"thread_id","id":…}`.
- **Handoff** (`thread_handoff {id, device?, max_chars=8000}` → `Context`; CLI
  `crossthreads thread <ID> --handoff`; MCP `crossthreads_thread` with
  `"handoff": true`) — a paste-ready Markdown brief for continuing the work in
  another tool or agent: repository, branch, every session, the original
  request from the first session, and the tail of the latest one (where the
  work stands). Most of the budget goes to the tail.

- **Across devices.** With federation configured, `threads` takes
  `"devices":[…]` (routing, like `search`; omitted = this device and every
  reachable peer) and answers `{"threads":[…],"unreachable":[names]}`; each
  `ThreadSummary` has a `device`. `thread` takes `"device":…` to fetch a thread
  from the peer it lives on. Remote threads are read-only from here.
  CLI: `crossthreads threads --remote`.
- **Live state.** When known, `ThreadSummary.live` and each session's `live`
  are `{state, inferred, since}` with `state` one of `running` | `needs_you` |
  `idle` | `ended`. A thread shows its most pressing session. `inferred: true`
  means it was guessed from the transcript file's modification time (which
  can only ever say `running`) rather than reported by a hook.
- **Hooks.** `crossthreads hooks install|uninstall|status` manages the hook in
  Claude Code's `settings.json` and Codex's `hooks.json`. The hook runs
  `crossthreads hook <tool> [event]`, which reads the tool's JSON payload on
  stdin and sends `{"op":"hook_event","tool":…,"event":…,"transcript_path":…,
  "session_id"?,"cwd"?,"message"?}` to the daemon. Events map to state as:
  `SessionStart`/`Stop` → `idle`; `UserPromptSubmit`/`PreToolUse`/`PostToolUse`
  → `running`; `Notification` mentioning a permission → `needs_you`;
  `SessionEnd` → `ended`. An event with no `transcript_path` is ignored.

**Ids.** A thread of one has the id `th_<its session key>`. A thread with
several sessions keeps the id of its earliest session. An id can therefore
change when sessions join or leave; to hold on to a thread, hold a session's
`conversation_id` and pass that to `thread`.

**Repository identity.** `repo_key` is the normalised `origin` remote
(`github.com/owner/repo`), else `root:<root commit>`, else `path:<abs path>`
(device-local). All worktrees of a repository share one key.

**Link detection** runs after each index pass, within one repository only:

| Signal | Result |
|---|---|
| A later session opens with text from the tail of an earlier one, including the assistant's words (a transcript transfer or a resume) | applied as `continued`, origin `detected` |
| A session in a *different tool* starts while an earlier one is active, or within 30 minutes after | suggested only |

A pair the user unlinked or rejected is never proposed again.

### 7.6 Rooms

Agents and the human exchange messages in a **room** ([SPEC.md](SPEC.md) §6.4).
All room tools take `me` (the agent's name) and, except `release`, `room`: the
project root path, which the daemon resolves to the repository's key so every
checkout of a repository shares a room, or any plain name for an ad-hoc room.
Results are compact text lines (`#12 14:03:11 claude→codex: …`), not JSON, to
keep agent context small. These work offline; no model.

| MCP tool | Daemon op | What it does |
|---|---|---|
| `crossthreads_room_send(me, text, room, to?, wait=true, timeout=50)` | `room_send` | Post, then block until a reply arrives and return it. `to` makes it a direct message. Text starting with `DONE` closes the exchange and releases the sender's claims. |
| `crossthreads_room_wait(me, room, timeout=50)` | `room_wait` | Return unread messages, blocking up to `timeout` seconds (max 600; `0` just checks). |
| `crossthreads_room_history(me?, room, limit=30)` | `room_history` | Who is here, turns used, claims, last messages. |
| `crossthreads_claim(me, paths, room, minutes=20)` | `room_claim` | Reserve absolute file or folder paths. Conflicts are reported, not claimed. Max 240 minutes. |
| `crossthreads_release(me, paths?)` | `room_release` | Release the given claims, or all of the agent's. |

The same operations are available from a shell as `crossthreads room send|wait|
history|claim|release` (`--room` defaults to the current directory).

For the board: `rooms` → `{"rooms":[RoomView]}`; `room_messages {room, after,
limit}` → messages after an id; `room_post {room, text, to?, author?}` posts as
the human (`{"type":"room_posted","id":…}`).

For hooks: `room_stop {me, room, wait, transcript_path?}` holds an agent that is
mid-exchange until the next message and returns it; `room_guard {paths}`
returns live claims covering those paths.

**Rules the daemon enforces.** An agent never reads its own messages or other
agents' direct messages. A newcomer to a busy room gets the latest 20 with a
note. With `CROSSTHREADS_ROOM_MAX_TURNS` set, `send` is refused at the cap
unless the text starts with `DONE`; a `DONE` or a human message starts a fresh
exchange. Over HTTP the blocking calls return immediately (the HTTP bridge
serves one request at a time); agents use the socket protocol.

**Across devices.** With federation on, an approved peer that both devices have
granted *Can message* (`set_peer_rooms {name, rooms}` → `Devices`; each
`DeviceInfo` carries `rooms`) shares every room. An agent there appears as
`name@device`, and `to: "codex@linux"` reaches it. Room messages carry
`device` (absent for this device). Between daemons: `peer_room_post {token,
from, messages}` pushes new messages, and `peer_room_sync {token, from, after}`
→ `{"type":"room_wire","messages":[…]}` returns this device's own messages
after id `after` (last 24 hours, 500 at most). Both are token-gated and refused
unless the receiver has granted `from`. Claims are per device.

**After `DONE`.** The exchange is indexed as a conversation of tool `room`,
kind `exchange`, and the sessions of the agents that spoke are linked
`conversed` (origin `explicit`) when a hook has reported their transcripts.

### 7.7 Phone access

A phone opens the web app from a second HTTP listener on the computer's
Tailscale address (default port `47102`), off until turned on
([PHONE.md](PHONE.md), SPEC FR-BRD-04). Ops for the computer itself:

| Op | Answers | What it does |
|---|---|---|
| `setup_status` | `setup {hooks_claude, hooks_codex, mcp_claude, mcp_codex, skill_claude, skill_codex, model, devices, devices_can_message, phone_enabled, phones, claude_present, codex_present}` | What's installed and turned on here, for Settings → Get started. Reads the files each installer writes; changes nothing. |
| `session` | `{"type":"session","phone":false,"can_post":true}` | Who is asking. On the phone listener: `phone: true`, the phone's `name`, and its `can_post` grant. |
| `phone_status` | `phone {enabled, listening, url?, error?, phones:[{id,name,created,last_seen,can_post}]}` | Settings → Phone. |
| `phone_set_enabled {enabled}` | `phone` | Start or stop the listener. Binds `CROSSTHREADS_PHONE_ADDR`, else the saved address, else this device's tailnet IP; never `0.0.0.0`. |
| `phone_pair` | `phone_pairing {code, url, qr_svg?, expires_secs}` | A one-time 8-character code (10 minutes), the `…/pair#CODE` link and an SVG QR code of it. |
| `phone_revoke {id}` · `phone_set_post {id, can_post}` | `phone` | Remove a phone; grant or withdraw posting to rooms. |

The phone listener serves the web app and two endpoints:

- `POST /api/pair` `{"code","name"}` → `200` and `Set-Cookie: ct_phone=<token>;
  HttpOnly; SameSite=Strict`, or `403`. Five wrong codes cancel every open code.
- `POST /api/rpc` with that cookie. Without it: `401 {"unpaired":true}`. Allowed
  ops: `ping`, `status`, `facets`, `session`, `search`, `get_conversation`,
  `devices`, `glance`, `threads`, `thread`, `thread_handoff`, `rooms`,
  `room_messages`, `room_history`, `saved`, `resume`; `room_post` only with the
  phone's `can_post` grant, signed with the phone's name. Anything else is `403`.

Both require `Content-Type: application/json` and, when an `Origin` is sent, the
same origin. Tokens are stored only as SHA-256 hashes in `phone.json`.

### 7.8 Who may call what

| Caller | Gets |
|---|---|
| This machine, over the TCP protocol (loopback, or its own tailnet address) | Every op |
| This machine's browser, over the local HTTP bridge | Every op, if JSON, same-origin and to a host it answers to; blocking room calls return at once |
| Another machine, over the TCP protocol | Only the peer protocol: `ping`, `peer_search`, `peer_get_conversation`, `peer_threads`, `peer_thread`, `peer_room_post`, `peer_room_sync`. Each checks the shared token; room ops also need the receiver's *Can message* grant for the sender. Anything else answers `only the peer protocol is served to other machines`. |
| A paired phone, over the phone listener | The reading allowlist in §7.7 |

## 8. Conventions

- **Auth/scope:** see §7.8. Local surfaces (CLI, MCP stdio, loopback HTTP) act as the local user.
- **Errors:** `{ "error": { "code": "not_found" | "bad_request" | "index_unavailable" | "llm_unavailable", "message": "..." } }`.
- **Stability:** request/response shapes are **versioned** (`x-crossthreads-api: 1`); additive changes are non-breaking, removals bump the version.
- **Determinism:** `mode: "lexical"` is fully deterministic for scripting; `hybrid`/`semantic`/`recall` may vary with model/index state.
- **Privacy:** every operation runs locally; `recall` only calls a cloud LLM if the user explicitly opted in (NFR-PRIV-01).

## 9. Open questions (tracked in PRD §12)

- **Resume depth** per tool (§6 `method`) — which tools support real deep-links vs. context-only.
- **`recall` LLM default** — bundled small local model vs. require user-configured (Ollama) vs. cloud opt-in.
- **HTTP surface** — ship at MVP or MCP+CLI only first.
