---
name: crossthreads
description: Recall and search past AI coding conversations across every tool (Claude Code, Codex, Cursor, Copilot, Gemini, Windsurf, and more) via the Crossthreads MCP server. Use at the start of a non-trivial task to recall prior decisions, and whenever you might be re-solving something you've worked on before.
---

# Crossthreads — cross-tool conversation memory

You have access to the **Crossthreads** MCP server, which indexes the user's AI
coding conversations from every tool into one searchable, local index. Use it to
avoid re-deriving past work, reuse earlier decisions, and resume prior threads.

## When to use it

- **At the start of a non-trivial task**, call `crossthreads_recall` with a
  question describing the task (e.g. "how did we set up the OAuth refresh
  retry?"). Read the digest before you plan.
- **When a problem feels familiar**, or you're about to build something that may
  have been solved before, `crossthreads_search` past sessions first
  (mode: `hybrid`).
- **To resume prior work**, call `crossthreads_build_context` for the topic and
  read the returned markdown block.
- Use `crossthreads_status` if you're unsure the index is populated.

## Tools

- `crossthreads_recall(question, limit?)` — answer-oriented digest of the most
  relevant past sessions.
- `crossthreads_search(query, mode?, limit?, filters?)` — ranked results; modes
  `lexical | semantic | hybrid` (default `hybrid`). Filters: tool, kind
  (`thread`/`skill`), project, date range, tag.
- `crossthreads_build_context(query, mode?, limit?, max_chars?)` — a paste-ready
  context block to bring this session up to speed.
- `crossthreads_status()` — index health (counts).

## Notes

- This needs the Crossthreads MCP server configured **and** the `crossthreadsd`
  daemon running. If a tool returns a connection error, tell the user to start it
  (`crossthreads-up`, or `scripts/start.sh`).
- Don't dump raw tool output at the user — synthesize what's relevant to the
  current task, and cite which past session it came from.
