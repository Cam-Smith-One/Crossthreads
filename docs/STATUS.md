# Crossthreads — Status & Audit Overview

_Last updated: 2026-06-20 · branch `claude/cross-agent-session-indexer-rw9e4x` · [PR #1](https://github.com/Cam-Smith-One/Crossthreads/pull/1)_

A snapshot of where the project stands after the end-to-end review: what's
verified, what's a known coverage gap, and what features are worth adding next.

---

## ✅ Privacy — clean

- **No PII in the build** — the one personal name (PRD owner field) was anonymized.
- **No secrets/keys/tokens** in any tracked file.
- **Test fixtures are synthetic** (`/home/dev/acme-api`, fabricated chats); **screenshots are illustrative mock data** — none of your real conversations.
- **`.gitignore` hardened** to ignore `*.db` / `*.sqlite*` so an index (which contains conversation history) can never be committed by accident. The real index lives outside the repo, in the platform data dir.
- `ui/dist`, `target/`, `node_modules/` correctly untracked.

## ✅ End-to-end test — passing

- **Gate:** `cargo fmt --check` + `clippy --all-targets -D warnings` + **69 tests** + UI build — all green.
- **Live RPC sweep** against real indexed data passed every path: lexical / semantic / hybrid search, filters, the `build_context` budget cap, `get_conversation`, bookmark/pin → `saved` → clear, plus negatives (bad id → null, empty query → `[]`) and static assets (all 200).
- **v4 → v5 DB upgrade** is covered by a migration test (bookmarks preserved, search works, idempotent re-open).

## 🐞 Bugs — none known remaining

Found and fixed during this audit:

| Bug | Impact | Status |
|---|---|---|
| `build_context` ignored `max_chars` *within* a conversation | A single long thread produced a 250 KB / 63k-token block that would overflow an agent's context window | ✅ Fixed + regression test |
| Schema created indexes on migration-added columns (`kind`, `session_key`) *before* the `ALTER` that adds them | **Any existing user upgrading an older index would fail to open it** (`SQL error: no such column`) | ✅ Fixed + migration test |

## 🔍 Gaps (coverage, not bugs)

These are environments this sandbox can't reproduce — flagged honestly rather than claimed as proven:

- **8 of 9 connectors are fixture-tested, not run against real installs.** Only **Claude Code** is exercised against live data here. Codex/Cursor were validated earlier on a real Mac. **Windsurf** (medium confidence) and **Antigravity** (low) are the likeliest to need real-data calibration — their formats are undocumented. All connectors detect-and-skip gracefully, so a mismatch means "missed sessions," never a crash.
- **ONNX semantic path** wasn't run this session (deterministic embedder; CI is the same). Validated earlier in the project.
- **File watcher** (live re-index on change) isn't exercised in CI — only `--no-watch` in tests.
- **Tauri desktop shell** can't build/run headless; the web path is the verified equivalent.

**Best next signal:** run it on your Mac against your real Cursor/Windsurf/Codex data and check the per-tool index counts — that's how we caught and fixed the Cursor issues last time.

## 💡 Potential features missing (prioritized)

| # | Feature | Why | Size |
|---|---|---|---|
| 1 | **Delete / "forget this thread"** | No way to remove a conversation short of a full rebuild — high-value for privacy | Small (1 RPC + UI + `user_state`-aware delete) |
| 2 | **Secret redaction on index** | Chats often contain API keys/tokens; scrub them before they sit in the index | Medium |
| 3 | **Windows release + installer** | Release builds Linux + macOS only; `install.sh` is bash | Medium |
| 4 | **In-UI "Re-index now" + empty-state onboarding** | Daemon auto-watches, but there's no manual trigger or "no tools found" screen | Small |
| 5 | **Export a conversation** (markdown/JSON) **+ notes/tags/collections** | `user_state` is already the right foundation for tags | Medium |
| 6 | **In-transcript search / jump-to-match + result pagination** | Viewer shows the full thread; no in-thread find, no "load more" | Medium |
| 7 | **`sqlite-vec` ANN** | Only needed past tens of thousands of messages (brute-force cosine is fine until then) | Medium |

**Recommendation:** start with **#1 (delete/forget)** and **#2 (redaction)** — they round out the privacy story; **#3 (Windows)** is the biggest reach gap.

## Architecture recap

```text
crates/  ct-core · ct-connectors · ct-embed · ct-store · ct-index · ct-daemon · ct-cli · ct-mcp
ui/      React + Vite (no UI framework — hand-written CSS, light/dark themes)
```

Single-writer daemon owns one SQLite database (conversations + FTS5 + vectors +
a separate durable `user_state` table for bookmarks/pins). It serves the CLI,
web UI, desktop shell, and MCP server. Local-first, no telemetry.
