# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it reaches
a tagged release.

## [0.3.2] - 2026-06-21

### Fixed
- `crossthreads-up` now **stops any crossthreadsd already running before it
  starts** (a previous launch left open, or one auto-started by the MCP server),
  so relaunching no longer fails with `Address already in use` and silently keeps
  serving the stale daemon. Opt out with `CT_NO_KILL=1`. Same for the Windows
  `.cmd` launcher.

## [0.3.1] - 2026-06-21

### Fixed
- Locate the **Tailscale CLI inside the macOS app bundle** when it isn't on
  PATH (`/Applications/Tailscale.app/Contents/MacOS/Tailscale`, plus Homebrew
  paths), so `--addr auto` and "Discover my devices" work for users who
  installed the Tailscale app rather than the standalone CLI.

## [0.3.0] - 2026-06-21

### Added
- **Automatic tailnet binding**: `crossthreadsd --addr auto` (and the
  `crossthreads-up` launcher, which now passes it) binds federation to this
  device's Tailscale IP so cross-device search works out of the box — **only
  once a federation token is set**, so the daemon is never exposed on the tailnet
  without authentication. Falls back to loopback when there's no token or
  Tailscale isn't connected.
- **Show this device's pairing code** in Settings → Devices: the code is masked
  by default with a **Show/Hide** reveal and a **Copy** button, so you can read
  it before pasting it on another device. (Previously the panel offered only a
  blind "Copy" and no way to see the code.)

### Changed
- Settings → Devices "pairing" hint now explains that the code appears
  automatically once a token is set and Tailscale is connected.

## [0.2.1] - 2026-06-21

### Fixed
- `crossthreads-up` now resolves symlinks before locating its install directory,
  so the launcher finds the bundled web UI (`~/.crossthreads/ui`) when it is run
  through the `~/.local/bin` symlink that `install.sh` creates — previously it
  pointed `--ui` at a nonexistent path and the app served a blank "not found".
- `crossthreadsd` warns at startup when `--ui <dir>` has no `index.html`, instead
  of silently 404ing every page.

## [0.2.0] - 2026-06-21

### Added
- **Cross-device search** (ADR-010): search the history on your other machines,
  federated daemon-to-daemon over a private Tailscale/WireGuard tunnel —
  local-first, no central server. Includes device selection (a default-all
  picker + an optional `devices` argument on the search/recall/build_context
  tools), on-demand **"Discover my devices"** (a one-shot `tailscale status`
  scan), result **device chips**, opening/previewing a remote result's
  transcript, cross-device `build_context`, per-device **serve-scope** filters,
  the shared token stored in the **OS keychain**, and a copy-paste **pairing
  code** to onboard a second device.
- **In-app Settings panel** with a **Documentation** section and a **Devices**
  tab (discover/approve, identity, serve-scope), plus a step-by-step
  **multi-device setup guide** (`docs/MULTI_DEVICE_SETUP.md`).
- New MCP tool **`crossthreads_devices`** and an optional `devices` argument on
  the existing search-style tools.

### Changed
- **Concurrent reads**: the index runs in **WAL mode** with a pool of read-only
  connections sharing one vector cache, so a heavy search no longer blocks other
  searches, status, or background indexing (single-writer invariant preserved).
- UI responsiveness: search responses are guarded against out-of-order overwrite,
  the open transcript is filtered/highlighted once per change (memoized), and
  "Load more" keeps the selection.

### Fixed
- Panic on a non-ASCII `started_at` in the date filter; connection-string /
  URL-userinfo credentials are now redacted; the Aider `####` user delimiter no
  longer misreads assistant Markdown headings; the federation token is compared
  in constant time; accepted connections get a read timeout (no thread-pinning
  half-open clients); the keychain token is written once (no per-startup
  re-prompt); MCP autostart retries after a failed spawn; FTS match markers use
  control-char sentinels so literal `[brackets]` in code aren't mis-highlighted;
  the UI surfaces HTTP / non-JSON RPC errors; assorted accessibility fixes.

## [0.1.0] - 2026-06-20

Local-first cross-agent session indexer: search, recall, and resume every AI
coding conversation across tools. Backend, web UI, and MCP server are
functionally complete and verified end-to-end. The native desktop window is
scaffolded but not yet released.

### Added
- **Connectors** for nine tools — Claude Code (JSONL), Codex (`rollout-*.jsonl`),
  Cursor (`state.vscdb`), Aider (`.aider.chat.history.md`), Cline (VS Code
  globalStorage tasks), GitHub Copilot Chat (VS Code `chatSessions`, incl. the
  1.109+ JSONL mutation log), Gemini CLI (`~/.gemini/tmp/*/chats`), Windsurf
  (`state.vscdb`), and Antigravity (Markdown artifacts scanned from its
  `~/.gemini/antigravity/` home, best-effort) —
  plus **skills/prompts** (`SKILL.md`, Codex prompts) as `kind = skill`. Each is
  versioned, detect-and-skips gracefully, and has a regression test.
- **Single SQLite index** (`ct-store`): content-hash dedup, FTS5 lexical search,
  vector storage with a parallelized in-memory cosine scan, and **RRF hybrid**
  search with a similarity floor. Tool / kind / project / date / **tag** filters.
- **Durable user state**: **bookmarks & pins** and **notes & tags**, kept in a
  separate table keyed by a stable session id so they survive re-indexing and a
  growing conversation.
- **Forget a thread**: deletes a conversation from the index and tombstones it so
  the watcher won't re-add it.
- **Secret redaction** (`ct-core::redact`): scrubs API keys, tokens, JWTs, and
  PEM private keys during indexing — nothing secret reaches the index.
- **Embeddings** (`ct-embed`): a deterministic offline default and a real
  all-MiniLM-L6-v2 ONNX backend behind the `onnx` feature.
- **Daemon** (`crossthreadsd`): single-writer index, debounced auto-reindex
  watcher, loopback protocol server, and an HTTP/JSON bridge that serves the UI.
- **CLI** (`crossthreads`): `index`, `search`, `context`, `status`.
- **MCP server** (`ct-mcp`): `crossthreads_search` / `recall` / `build_context`
  / `status` over stdio for agents. **Auto-starts `crossthreadsd`** on first use
  (and reuses one already running), so registering the MCP server is the only
  setup step — no daemon to keep up by hand.
- **Agent skill**: `crossthreads skill install` drops a Claude Code `SKILL.md`
  and a Codex `/crossthreads` prompt that nudge agents to recall prior work via
  the MCP tools.
- **Web UI** (`ui/`): React + Vite with the Crossthreads logo, **light/dark**
  themes, hybrid search, filters, highlighted results, a conversation viewer with
  in-transcript find, bookmarks/pins/notes/tags, export (Markdown/JSON), "forget",
  reveal-source, result pagination, an empty-state onboarding card, and a
  re-index action.
- **Install & release**: prebuilt-binary release workflow (Linux x64, macOS
  arm64/x64, Windows x64), `scripts/install.sh` / `install.ps1`, and a
  `crossthreads-up` launcher.
- Project docs (PRD, requirements, architecture, agent API, decisions, roadmap,
  development, status), CI, and demo/screenshot/check scripts.

### Notes
- Local-first; the only optional network call is the one-time embedding-model
  download for ONNX semantic search.

[Unreleased]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.3.2...HEAD
[0.3.2]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Cam-Smith-One/Crossthreads/releases/tag/v0.1.0
