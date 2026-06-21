# Crossthreads documentation

End-to-end docs for the project, from product intent down to how the code runs.

## Start here
- [DEVELOPMENT.md](DEVELOPMENT.md) — build, test, run the daemon/UI/MCP, the
  `onnx` feature, and the crate layout.
- [MULTI_DEVICE_SETUP.md](MULTI_DEVICE_SETUP.md) — step-by-step guide to connect
  several of your machines so one search spans all of them.

## Product
- [PRD.md](PRD.md) — vision, users, scope, metrics, GTM, risks, open questions.
- [REQUIREMENTS.md](REQUIREMENTS.md) — functional & non-functional requirements
  with IDs and acceptance criteria.
- [ROADMAP.md](ROADMAP.md) — phased delivery plan and current status.

## Engineering
- [ARCHITECTURE.md](ARCHITECTURE.md) — components, data model, process model
  (daemon), tech-stack choices, privacy posture.
- [AGENT_API.md](AGENT_API.md) — the agent-facing interface (CLI/JSON, MCP, HTTP):
  search, recall, build_context, resume.
- [DECISIONS.md](DECISIONS.md) — architecture decision records (ADRs).

## How it fits together

```
source tools                ┌──────────── crossthreadsd (daemon) ───────────┐
 Claude Code  ─┐  fs watch   │  connectors → normalize → SQLite index        │
 Cursor       ─┤────────────▶│   (dedup) → FTS5 + vectors → hybrid (RRF)      │
 Aider        ─┤             │   + filters                                    │
 Codex        ─┘             │            loopback protocol + HTTP/JSON       │
                             └──────┬───────────┬──────────┬─────────┬────────┘
                                    ▼           ▼          ▼         ▼
                                  Desktop      CLI        MCP       Web UI
                                  (Tauri)   (--remote)  (agents)  (browser)
```

The engine is headless and client-agnostic: the CLI, MCP server, web UI, and
native shell are all thin clients of the one daemon and its single index
(see [DECISIONS.md](DECISIONS.md) ADR-005 and ADR-009).
