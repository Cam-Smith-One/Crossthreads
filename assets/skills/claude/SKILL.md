---
name: crossthreads
description: Recall and search past AI coding conversations across every tool (Claude Code, Codex, Cursor, Copilot, Gemini, Windsurf, and more) via the Crossthreads MCP server. Use at the start of a non-trivial task to recall prior decisions, surface unresolved work, and whenever you might be re-solving something you've worked on before.
---

# Crossthreads — cross-tool conversation memory

You have access to the **Crossthreads** MCP server, which indexes the user's AI
coding conversations from every tool into one searchable, local index. Use it to
avoid re-deriving past work, reuse earlier decisions, and resume prior threads.

## Recall before you act (do this proactively)

Treat Crossthreads as your memory of everything the user has done before — **you
are expected to consult it without being asked.** Before you plan a non-trivial
task, write code that might already exist, or make a decision the user may have
already made:

1. **Call `crossthreads_recall`** with a question describing the task (e.g. "how
   did we set up the OAuth refresh retry?"). Read the digest before you plan.
2. If it feels familiar or you're about to re-solve something, **`crossthreads_search`
   first** (mode `hybrid`) and reuse what you find.
3. To resume prior work, **`crossthreads_build_context`** for the topic and read
   the returned markdown block.

Doing this early is cheap and routinely saves re-deriving a past decision. When
in doubt, recall first. Don't announce that you're "checking memory" — just do
it and fold what you learn into your work.

## When to use the higher-level tools

- Starting a work session or unsure what's outstanding → `crossthreads_open_loops`
  to see unresolved threads, dangling TODOs, and unconfirmed fixes.
- Onboarding into this user's repo/style → `crossthreads_how_i_work` for their
  conventions, and `crossthreads_decision_log` for decisions already made.
- Want an overview of what they've been doing → `crossthreads_themes` (or
  `crossthreads_digest` for a short reflective summary).

## Tools

- `crossthreads_recall(question, limit?)` — answer-oriented digest of the most
  relevant past sessions.
- `crossthreads_ask(question, limit?, filters?)` — a synthesized, **cited** answer
  to a question, drawn from past sessions (uses the user's model; returns the
  retrieved context if none). Use when you want an answer, not a list.
- `crossthreads_search(query, mode?, limit?, filters?)` — ranked results; modes
  `lexical | semantic | hybrid` (default `hybrid`). Filters: tool, kind
  (`thread`/`skill`), project, date range, tag.
- `crossthreads_build_context(query, mode?, limit?, max_chars?)` — a paste-ready
  context block to bring this session up to speed.
- `crossthreads_open_loops(limit?)` — unresolved work across recent sessions.
- `crossthreads_decision_log(limit?)` — notable decisions ("chose X over Y
  because Z") with rationale.
- `crossthreads_knowledge_cards(limit?)` — durable Q→A cards worth remembering.
- `crossthreads_how_i_work(limit?)` — the user's working conventions, ready for a
  CLAUDE.md / AGENTS.md.
- `crossthreads_digest(limit?)` — a short reflective digest of recent work.
- `crossthreads_recurring(limit?)` — recurring problems/patterns the user keeps
  hitting, each with a fix.
- `crossthreads_themes(k?, name?)` — cluster sessions into themes (set
  `name: true` for short LLM-generated theme names).
- `crossthreads_activity(bucket?, filters?)` — session activity over time
  (day/week) with a per-tool breakdown.
- `crossthreads_graph(limit?)` — knowledge graph of projects, tools, and tags.
- `crossthreads_status()` — index health (counts).

## Notes

- The higher-level tools (`open_loops`, `decision_log`, `knowledge_cards`,
  `how_i_work`, `digest`, and `themes` with `name`) call the user's configured
  model. If none is set up, they return a hint to add one in Settings → Models;
  the search/recall tools work without it.
- This needs the Crossthreads MCP server configured **and** the `crossthreadsd`
  daemon running. If a tool returns a connection error, tell the user to start it
  (`crossthreads-up`, or `scripts/start.sh`).
- Don't dump raw tool output at the user — synthesize what's relevant to the
  current task, and cite which past session it came from.
