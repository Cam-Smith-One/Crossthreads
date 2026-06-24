# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it reaches
a tagged release.

## [0.7.0] - 2026-06-24

### Added
- **Ask (RAG over your history)** — ask a natural-language question and get a
  synthesized, **cited** answer drawn from your own past sessions across every
  tool and device. Retrieves the most relevant sessions, then answers from them;
  with a model it synthesizes, without one it returns the retrieved context (like
  `recall`). New 💬 panel in the web app, `crossthreads ask "…"` on the CLI, and
  the `crossthreads_ask` MCP tool. This also covers cross-tool synthesis.
- **Temporal view (activity over time)** — see your sessions bucketed by day or
  week with a per-tool breakdown. New 📅 timeline in the web app, `crossthreads
  activity [--by day|week]` on the CLI, and the `crossthreads_activity` MCP tool.
  Offline; no model.
- **Knowledge graph** — a map of your work: projects, tools, and tags as nodes
  (sized by session count) with co-occurrence edges. New 🕸️ view in the web app,
  `crossthreads graph` on the CLI, and the `crossthreads_graph` MCP tool.
  Deterministic; offline.
- **Recurrence alerts** — a new insight that surfaces problems and patterns you
  keep hitting across sessions, each with a suggestion to break the loop. New
  "Recurring" tab in the Insights panel, `crossthreads insight recurrence`, and
  the `crossthreads_recurring` MCP tool.

### Notes
- MCP now exposes **15 tools**. Ask uses your model login when present and
  degrades to retrieval-only without one; activity and graph never need a model.

## [0.6.1] - 2026-06-24

### Changed
- **Insights span the whole history, not just recent threads.** The synthesis
  corpus now includes **skills/prompts** alongside conversations and **every
  tool**, reads a much larger subset (200 records for open loops/digest, 400 for
  knowledge cards / decisions / how-I-work), with a bigger per-record and total
  text budget (~300k chars / ~75k tokens) and a roomier output budget. Skills are
  tagged in the corpus so the model can weigh them appropriately.

### Fixed
- **Repeated macOS Keychain prompts.** The LLM key store now caches each keychain
  item in memory for the daemon's lifetime, so provider resolution on the
  completion hot path reads each secret at most once per launch instead of on
  every model call — no more prompt storms during synthesis. Writes update the
  cache in place. (After upgrading, click **Always Allow** once per secret.)

## [0.6.0] - 2026-06-24

### Added
- **Insights** — LLM synthesis over your recent sessions, exposed everywhere
  (CLI, MCP, web app). Five kinds share one engine (`ct-daemon::insights`):
  - **Open loops** — unresolved work, dangling TODOs, and fixes never confirmed.
  - **Knowledge cards** — durable Q→A pairs worth remembering.
  - **Decision log** — notable decisions ("chose X over Y because Z") with rationale.
  - **How I work** — your conventions, ready to drop into a CLAUDE.md / AGENTS.md.
  - **Digest** — a short reflective catch-up on recent work.
- **MCP insight tools** — `crossthreads_open_loops`, `crossthreads_knowledge_cards`,
  `crossthreads_decision_log`, `crossthreads_how_i_work`, and `crossthreads_digest`,
  so agents can pull these views natively. Each takes an optional `limit`.
- **`crossthreads insight <kind>`** CLI command (`--limit N`), with friendly
  aliases (`loops`, `cards`, `decisions`/`adr`, `profile`, `weekly`).
- **Insights in the web app** — a 💡 view with tabbed kinds (Open loops,
  Decisions, Knowledge cards, How I work, Digest), rendered from the same daemon
  `Insight` endpoint.
- **AI-named themes** — the theme map gains a "✨ Name with AI" button (and the
  daemon/MCP `themes` op a `name` flag) that labels each cluster with a short
  generated name instead of keyword terms; falls back to the keyword label when
  no model is configured.
- **Proactive recall in the agent skill** — the installed Claude Code / Codex
  skill now nudges agents to consult Crossthreads memory *before* planning
  non-trivial work, and documents the new higher-level tools.

### Notes
- All insight features and AI naming reuse the existing model-auth layer
  (Settings → Models / `llm-auth`): your Claude Code / Codex / Gemini login or a
  BYO key. Search, recall, and offline themes still work with no model at all.

## [0.5.0] - 2026-06-24

### Added
- **Theme map in the web app** — a 🗺️ view that clusters your indexed sessions
  by topic and shows each theme as a size-scaled card (label, tool mix, sample
  titles you can click to open). Backed by a shared `ct-store::themes` core and a
  new daemon `Themes` endpoint, so the CLI, UI, and MCP all show the same themes.
- **MCP `crossthreads_themes` tool** — agents can pull your work themes (label,
  size, tool mix, samples) over MCP, with an optional `k` (cluster count).
- **Google (Gemini) provider** — third option in the model auth layer and
  Settings → Models: keychain BYO key plus `GEMINI_API_KEY` / `GOOGLE_API_KEY` /
  the `gemini` CLI, calling the Gemini `generateContent` API
  (`gemini-2.0-flash` by default; `CROSSTHREADS_GOOGLE_MODEL`).

## [0.4.0] - 2026-06-23

### Added
- **Themes** — `crossthreads themes` clusters your indexed conversations by
  embedding (k-means over per-conversation centroids) and surfaces the themes you
  spend time on, labeled by each cluster's most distinctive terms. Fully local and
  offline. `--name` labels clusters with your local model login (below).
- **Optional LLM backend** (`ct-llm`) that reuses your **existing** Claude Code or
  Codex login — no new key required. Tiered, per provider, most-preferred first:
  a stored key → an API key in the environment → an explicit token → the agent CLI
  (`claude -p` / `codex exec`) → a best-effort token scavenged from the agent's
  own login. Secrets are never logged.
- **Settings → Models** — bring-your-own API key per provider (Anthropic, OpenAI),
  stored in the **OS keychain**, plus a picker for which provider AI features
  prefer. New `crossthreads llm-auth` shows what's resolved (secret-free).

### Notes
- Everything here is opt-in; the index and search never call a model. Override the
  provider with `CROSSTHREADS_LLM_PROVIDER`, and the model with
  `CROSSTHREADS_ANTHROPIC_MODEL` / `CROSSTHREADS_OPENAI_MODEL`.

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

[Unreleased]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/Cam-Smith-One/Crossthreads/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Cam-Smith-One/Crossthreads/releases/tag/v0.1.0
