# Crossthreads documentation

End-to-end docs for the project, from product intent down to how the code runs.

## Start here
- [DEVELOPMENT.md](DEVELOPMENT.md) — build, test, run the daemon/UI/MCP, the
  `onnx` feature, and the crate layout.
- [MULTI_DEVICE_SETUP.md](MULTI_DEVICE_SETUP.md) — connect several of your
  machines: one search, one board, and rooms that span them.
- [PHONE.md](PHONE.md) — open your board on your phone over Tailscale.
- [TROUBLESHOOTING.md](TROUBLESHOOTING.md) — fixes for common problems.

## Product
- [SPEC.md](SPEC.md) — the product spec: threads, board, rooms, workspaces,
  dispatch, the security model, and which milestones have shipped (§10).
- [PRD.md](PRD.md) — vision, users, scope, metrics, GTM, risks, open questions.
- [REQUIREMENTS.md](REQUIREMENTS.md) — functional & non-functional requirements
  with IDs and acceptance criteria.
- [ROADMAP.md](ROADMAP.md) — the original phased plan (current status: SPEC §10
  and the [CHANGELOG](../CHANGELOG.md)).

## Engineering
- [ARCHITECTURE.md](ARCHITECTURE.md) — components, data model, process model
  (daemon), tech-stack choices, privacy posture.
- [AGENT_API.md](AGENT_API.md) — the agent-facing interface (CLI/JSON, MCP, HTTP):
  search, recall, build_context, resume, threads, rooms, devices, phone, and who
  may call what.
- [DECISIONS.md](DECISIONS.md) — architecture decision records (ADRs).

## How it fits together

```
source tools                 ┌──────────── crossthreadsd (one per machine) ───────────┐
 Claude Code ─┐  fs watch     │ connectors → normalize → SQLite index (dedup)           │
 Codex       ─┤  + hooks      │  FTS5 + vectors → hybrid search · threads and links     │
 Cursor, …   ─┘──────────────▶│  live state · rooms and claims · insights and metrics   │
                              │ TCP protocol · HTTP bridge · phone listener (tailnet)    │
                              └───┬─────────┬─────────┬──────────┬───────────┬──────────┘
                                  ▼         ▼         ▼          ▼           ▼
                               Desktop     CLI       MCP      Web app     Phone (paired)
                               (Tauri)            (agents)   (browser)
                                  peer protocol (token) ⇄ your other machines' daemons
```

The engine is headless and client-agnostic: the CLI, MCP server, web app,
native shell and phone are all thin clients of the one daemon and its single
index (see [DECISIONS.md](DECISIONS.md) ADR-005 and ADR-009). Machines talk to
each other only through the token-checked peer protocol (ADR-010).
