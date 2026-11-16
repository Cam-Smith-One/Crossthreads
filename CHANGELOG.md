# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it reaches
a tagged release.

## [0.20.0] - 2026-11-16

Rooms across devices: an agent on one machine can talk to an agent on another.

### Added
- **Rooms span your approved devices.** Tick **Can message** next to a device
  in Settings → Devices, on both devices, and their agents share every room:
  Claude Code on the laptop can ask `codex@linux-desktop` for a review in the
  same repository's room and block for the answer, as on one machine. Messages
  are pushed the moment they are posted; a device that was offline catches up
  within about 10 seconds of coming back (up to a day of messages).
- Agents on other devices show as `name@device` in replies, presence, the
  Rooms view and the room list.
- `crossthreads room devices [NAME on|off]` lists and changes the grant from a
  shell. Daemon ops `set_peer_rooms`, `peer_room_post` and `peer_room_sync`.
- Both devices archive a closed exchange identically, so cross-device search
  shows it once.

### Notes
- File claims still cover only the device they were made on (each device has
  its own checkout); agents are told to agree on files and branches instead.
- The index schema moves to v11 (two columns on room messages); upgrading is
  automatic.

## [0.19.0] - 2026-11-15

A review of the whole product: fewer places to look, two real bugs fixed, and a
way to carry a piece of work into another tool.

### Added
- **Pick this up.** Every thread now offers *Copy resume command* (reopen the
  latest session in its own tool) and *Copy handoff*: a Markdown brief with the
  repository, branch, every session, the original request, and the end of the
  latest session, ready to paste into another tool or agent. Also
  `crossthreads thread <ID> --handoff`, the `thread_handoff` daemon op, and
  `handoff: true` on the `crossthreads_thread` MCP tool.
- A **Thread** button in the session viewer opens the thread that session
  belongs to.
- **`/` or Ctrl/Cmd+K** jumps to search.
- Search can be narrowed to **room exchanges**.

### Changed
- **The header says what things are.** Eleven unlabelled emoji buttons became
  three labelled places — **Insights**, **Rooms**, **Ask** — plus theme and
  settings.
- **One Insights view** with four tabs replaces six separate panels: *How you
  work* (AI-fluency scores, your current focus and trends, activity), *Weekly
  review*, *Themes*, and *Analyses*. Analyses are labelled cards that run only
  when clicked; without a model you get a "Set up a model" card that opens
  Settings → Models, not a red error.
- **Search filters live behind a Filters button**, so the home screen leads
  with your threads. Modes are named for what they do: best match, by meaning,
  exact words.
- **One Save star** replaces the separate pin and bookmark. Existing pins count
  as saved.
- The status line counts **sessions**, not every indexed record, and leaves the
  embedder details to a tooltip. Tool chips count sessions too.
- Results and the viewer show tool chips and a short project path instead of
  the full path; export and forget moved to the viewer's footer.

### Removed
- **The knowledge graph** (web view, `crossthreads graph`, `crossthreads_graph`,
  the `graph` op). It listed projects, tools and tags with co-occurrence counts,
  which threads, activity and themes already show better.
- **The `how_i_work` and `digest` analyses** (and their MCP tools). *Gaps*
  covers your conventions and drafts a CLAUDE.md; the weekly review covers what
  the digest did. MCP now exposes 28 tools.

### Fixed
- **A session that grew showed up twice.** Re-indexing a longer version of a
  session added a new row but kept the old one, so search listed the session
  twice and counts were inflated. The older version is now dropped when its
  messages are a prefix of the new one, and existing indexes are cleaned up on
  the next index pass. Two different chats that only share a key are left alone.
- **Codex resume could use the wrong id.** The session UUID was taken from
  anywhere in the transcript's path, so a UUID in a parent folder name won.
  It now comes from the file name only.
- **Themes and activity counted installed skills as work.** The theme map was
  dominated by `SKILL.md` files and activity counted them as sessions. Both now
  cover sessions only.

## [0.18.0] - 2026-11-15

### Added
- **Rooms — your agents talk to each other, and you step in.** Meat Proxy's
  protocol, rebuilt inside the daemon. Open two agents in one project, tell one
  `/crossthreads listen` and the other `/crossthreads lead <task>`, and they
  hand work back and forth without you copy-pasting.
  - Five MCP tools: `crossthreads_room_send` (posts, then blocks for the reply:
    one call per turn, no polling), `crossthreads_room_wait`,
    `crossthreads_room_history`, `crossthreads_claim`, `crossthreads_release`.
    The room rules ship as MCP server instructions, so they apply without the
    skill. MCP now exposes 31 tools.
  - **A room is the repository**, keyed by the same machine-independent key
    threads use, so two checkouts both called `api` no longer share a room.
    Any other name is an ad-hoc room.
  - **File claims** with an edit guard: an agent claims files before editing;
    the PreToolUse hook denies another agent's edit to them (Claude Code's edit
    tools and Codex's `apply_patch`).
  - **Auto-wake:** an agent that ends its turn mid-exchange is held by the Stop
    hook until the next message arrives, then handed it.
  - **You outrank the agents.** The new Rooms view (🤝) shows each room live:
    who is listening, turns used, claims, the conversation. What you type is
    marked `(human)` and overrides them.
  - `DONE` closes an exchange and releases the sender's claims. An optional
    turn cap (`CROSSTHREADS_ROOM_MAX_TURNS`, off by default) bounds an
    unattended run; `DONE` or a message from you lifts it.
  - `crossthreads room send|wait|history|claim|release` does the same from a
    shell, for chats opened before the tools were installed.
- **Exchanges become part of your history.** A closed exchange is indexed for
  search (kind `exchange`), and the sessions that took part are linked into one
  thread as `conversed` when hooks have identified them.
- **`crossthreads room import`** brings in a Meat Proxy database.
- The board shows a **Listening** badge for a session parked in a room.

### Changed
- **The HTTP API refuses other websites.** `POST /api/rpc` now requires
  `Content-Type: application/json`, a `Host` that is loopback or the bound
  address, and, when the browser sends one, an `Origin` that is this server.
  This closes cross-site posting and DNS rebinding, which matter now that the
  API can post a message agents obey. Scripts calling the API must send the
  JSON content type.
- Hooks are installed with the event named on the command line
  (`crossthreads hook claude-code Stop`) and a longer timeout for Stop. Run
  `crossthreads hooks install` again to pick this up.
- Schema v10 (rooms, read positions, claims); v9 indexes upgrade in place.

### Not yet (see the spec)
- Rooms across devices; claims are absolute paths on one machine.
- Linking the sessions of an exchange needs hooks installed, since that is how
  the daemon learns which session an agent is.
- Tested with simulated agents driving the real CLI, hooks and MCP server, not
  with live Claude Code and Codex sessions.

## [0.17.0] - 2026-11-15

### Added
- **Live session state.** The board now shows which sessions are **running**
  and which **need you** (blocked on a permission prompt), and pins anything
  waiting on you to the top. It refreshes itself every ten seconds while open.
  - `crossthreads hooks install` adds a hook to Claude Code and Codex that
    reports session events to the local daemon; `hooks uninstall` and
    `hooks status` do what they say. The installer only touches its own
    entries, keeps your key order, and backs each file up once.
  - The hook (`crossthreads hook <tool>`) prints nothing, waits at most 400 ms,
    and exits 0 whatever happens, including with no daemon running. It takes a
    few milliseconds per event.
  - Without hooks, a session whose transcript was written in the last 45
    seconds is shown as running and labelled *inferred*. Inference cannot see
    a permission prompt.
  - State is held in memory only. A hook that said "running" and then went
    quiet for 20 minutes is presumed idle; entries are dropped after 12 hours.
- **The board spans your devices.** With cross-device search set up, the thread
  list is fanned out to approved peers and merged by recency. Because
  repository keys are machine-independent, work on the same repository groups
  together whichever machine it is on. Each remote thread carries a device
  chip, opens read-only, and its sessions' transcripts open through the same
  token-gated path search uses. A device that doesn't answer is named in a
  banner rather than silently missing. Serve-scope applies: a thread with any
  session outside it is not served.
- Daemon ops `hook_event`, `peer_threads`, `peer_thread`; `threads` takes
  `devices` and returns `unreachable`; `thread` takes `device`. CLI
  `crossthreads threads --remote` asks the daemon (live state + other devices).

### Changed
- `serde_json` now preserves key order workspace-wide, so rewriting a user's
  settings file never reorders it. JSON output keys now appear in declaration
  order rather than alphabetically.

### Not yet (see the spec)
- Opening the board from a phone: the HTTP bridge is still loopback-only and
  unauthenticated, and needs its own security work first.
- Codex reports only "working" and "turn ended"; its permission prompts are not
  visible to hooks we could verify.
- The same piece of work on two machines is two threads under one repository;
  linking them needs thread metadata exchange (ADR-011).
- The hooks were exercised with simulated payloads, not a live Claude Code or
  Codex session.

## [0.16.0] - 2026-11-15

### Added
- **Threads — one identity for a piece of work across tools.** The first
  milestone of the new [product spec](docs/SPEC.md). Every session now belongs
  to a thread; a session nobody has linked is a thread of one. A Claude Code
  session continued in Codex is one thread with two sessions. Threads have a
  status (`active` / `parked` / `done`), can be renamed, merged and split, and
  live in durable tables keyed by `session_key`, so rebuilding the index never
  loses them (schema v9; v8 indexes upgrade in place).
- **Repository identity that is the same on every machine.** Sessions get a
  `repo_key`: the normalised `origin` remote (`github.com/owner/repo`), else the
  root commit, else the path. Worktrees share their repository's key.
  Credentials in a remote URL are never kept.
- **Automatic and suggested links.** After each index pass a deterministic
  detector (no model) links a session that opens with the tail of an earlier
  one in the same repository — a transcript transfer or a resume — and
  *suggests* a session started in another tool around the same time. Unlinking
  or rejecting is remembered, so a pair is never proposed twice.
- **The board is the web app's home screen.** Threads grouped by repository,
  most recently active first, with tool chips, branch, session count and
  status tabs. Opening a thread shows one timeline of its sessions across
  tools, with rename, status, remove, merge, and accept/dismiss for
  suggestions. Search stays at the top; "Back to threads" returns from results.
  Usable at phone width.
- **Everywhere else:** daemon ops `threads`, `thread`, `link_sessions`,
  `unlink_session`, `reject_link`, `set_thread`; CLI `crossthreads threads` and
  `crossthreads thread …`; MCP tools `crossthreads_threads` and
  `crossthreads_thread` (MCP now exposes 26 tools).
- **[docs/SPEC.md](docs/SPEC.md)** — the product spec for threads, the board,
  rooms, workspaces and dispatch.

### Changed
- The web app says "sessions" for single conversations, so "thread" can mean
  the cross-tool unit. The stored `kind = 'thread'` value is unchanged.

### Fixed
- **Crossthreads no longer indexes its own model calls.** The weekly review,
  insights and theme naming run the Claude / Codex / Gemini CLI, which records
  each run as a session; those were being indexed as if they were your work.
  They now run from a dedicated directory and the indexer skips it. Helper
  sessions indexed before this release stay until you forget them.
- A test in `ct-cli` that newer stable Rust rejects under `-D warnings`.

### Not yet (see the spec)
- Live session state, the board across devices, and phone access over the
  tailnet. The board shows this device's threads and when each was last active.
- Detecting a native resume by session id, and using `/codex:transfer` markers
  directly; both currently rely on copied content.

## [0.15.0] - 2026-07-12

### Added
- **Resume — one canonical "pick up where you left off."** A new deterministic
  `ct-daemon::resume` produces the native way back into a session: a runnable
  command for tools that have one — Claude Code (`claude --resume <id>`) and
  Codex (`codex resume <uuid>`), each prefixed with `cd <project>` so it lands in
  the right workspace — and a reveal-the-transcript fallback for everything else
  (never a command we can't stand behind). Exposed as a daemon `resume` op, a new
  `crossthreads resume <ID> [--run]` CLI, and the web UI's resume button, which
  now reads this op instead of its own ad-hoc string — **one source of truth**
  across every surface.
- **`crossthreads glance`** — the menu-bar popover's usage × insight bundle, now
  on the command line too (`--json` for the raw shape).

### Changed
- **Live quota is now real, not reserved.** The glance's per-tool `quota` slot
  reads a **documented sidecar** — `~/.crossthreads/quota/<tool>.json` (or
  `$CROSSTHREADS_QUOTA_DIR`) shaped like `QuotaWindow` — that any on-device helper
  can write (e.g. from `anthropic-ratelimit-*` response headers). Crossthreads
  still **never scrapes** undocumented provider internals: missing, malformed, or
  stale (>24 h past `resets_at`) files show nothing, so a window appears only when
  one genuinely exists. This ships the *contract and reader*; an on-device writer
  that populates it is the remaining piece.

## [0.14.0] - 2026-07-12

### Added
- **Scheduled weekly-review delivery — the review pushes itself.** The proactive
  weekly review was pull-only (you had to open the app or run `crossthreads
  weekly`). Now it can arrive on its own:
  - **`crossthreads schedule install|uninstall|status`** installs a platform
    scheduler entry that runs delivery weekly — a **launchd** user agent on
    macOS, a **managed crontab** block elsewhere. Pick the cadence with
    `--day <Mon…Sun|0-6>` and `--hour <0-23>` (default Monday 09:00). The
    manifest and crontab-block logic (`ct-daemon::schedule`) is pure and
    unit-tested; the crontab block is marked and idempotent, so reinstalling
    updates in place and uninstall restores your other lines untouched.
  - **`crossthreads weekly --deliver [--to <DIR>]`** ensures a current review,
    writes it to `weekly-<period>.md` (default `<data dir>/crossthreads/reviews`),
    and fires a desktop notification (`osascript` on macOS, `notify-send`
    elsewhere; silently skipped when neither exists — the file is the durable
    artifact). A per-period marker means a scheduler that fires more than once
    won't re-notify for the same week.
  - Delivery and the CLI/op/MCP surfaces now share one `weekly::ensure_current`
    entry point, so they can never drift.

## [0.13.0] - 2026-07-12

### Added
- **Menu-bar / tray app.** The native desktop shell (Tauri) becomes a
  **menu-bar app** (macOS uses `Accessory` activation — no dock icon, like
  CodexBar): the tray icon toggles a compact **glance** popover, the tray menu
  opens the full window, and closing the main window hides it back to the tray.
  Cross-platform — Mac *and* Windows (and Linux).
- **The glance — usage × insight in one look.** A new deterministic daemon op
  (`ct-daemon::glance`) fuses two things no other tool combines:
  - **Usage** — per-tool session volume *today vs. your own baseline* and this
    week (CodexBar-style meters, but from your local index — no quota scraping).
    A `QuotaWindow` slot is reserved for live rate-limit/reset countdowns that an
    on-device reader fills; the shipping build shows usage-vs-baseline and never
    fabricates quota numbers.
  - **Insight** — your fluency score (+ weakest dimension), how many recent
    threads are unresolved, and the optimize loop's current focus.
  Surfaced in the tray popover (`index.html?view=glance`).

## [0.12.0] - 2026-07-11

### Added
- **Your AI-fluency — a frame, and a reflective mode.** A new 🧭 view (and
  `crossthreads fluency` / `crossthreads_fluency`) that organizes the behavioral
  metrics around the **4D AI-Fluency framework** — Delegation, Description,
  Discernment, Diligence — scoring each dimension 0–100 with its movement vs. the
  previous window. Where *optimize* is prescriptive ("do X, measured"), fluency is
  the **understanding** half:
  - each dimension shows a score, a meter, the exact metrics that fed it, and a
    plain-language read of what's driving it;
  - it ends on a **reflective question, not a to-do** — an open prompt about what
    you might want to keep doing yourself even if the agent could do it faster —
    which you can talk through via *Ask* (inspired by Anthropic's *Reflect with
    Claude*). Deterministic — no model.

### Notes
- MCP now exposes **24 tools**.

## [0.11.0] - 2026-06-25

### Added
- **Optimize how you work — a closed loop.** A new 🎯 view (and `crossthreads
  optimize` / `crossthreads_optimize`) that turns the behavioral metrics from
  *descriptive* into *prescriptive and verified*:
  - computes the **single highest-impact change** to make now (a concrete,
    testable prescription grounded in your metrics — e.g. front-load constraints,
    close loops, ask for tests, batch by project);
  - **measures whether the last change worked** — a week later it grades the
    targeted metric against its baseline (a validation-gated verdict: improved /
    regressed / no clear change), like SkillOpt but for a person;
  - keeps a running **optimization log** of what you changed and what it did.
  Deterministic — no model. State survives re-indexing.
- **Trends over time** — every metric compared to the previous window with a
  direction (↑ better / ↓ worse). New `crossthreads trends` and
  `crossthreads_trends`; also embedded in the optimize view.
- **Sharper diagnostics** — three new signals from data we already store:
  **time-to-first-response** (from per-message timestamps), **error-recovery
  rate** (of sessions with a pasted error, the fraction resolved), and
  **context-switching cost** (distinct projects per active day). They flow into
  `metrics`, Work DNA, and the levers above.

### Notes
- MCP now exposes **23 tools**.

## [0.10.0] - 2026-06-25

### Added
- **"What's indexed" tool chips.** The header now shows a chip per detected tool
  with its conversation count (e.g. *Claude Code 142 · Codex 31*), so you can see
  at a glance what Crossthreads found — and spot a tool that's under-counting.
  `Status` carries per-tool counts; `crossthreads status` and the
  `crossthreads_status` MCP tool report them too.

### Changed
- **Activity is now insightful.** The temporal view gained a summary (total
  sessions, active periods, busiest period, average per period), a per-tool
  **legend**, and **stacked bars colored by tool** — so each bar shows *which*
  tools you used that week, not just a count.
- **Ask draws on more of your history.** Default retrieval went from 6 → 12
  sessions, with the context budget scaled to match, for richer cited answers.

## [0.9.1] - 2026-06-24

### Fixed
- **Codex / OpenAI model auth (401).** Insights failed for Codex users with
  `OpenAI (Codex) via reused login token: status code 401`. A ChatGPT/Codex
  *subscription* token isn't an API key and can't call the public OpenAI API, so
  Crossthreads no longer offers it by default — the **`codex` CLI** is the real
  reuse path. It also now finds the `codex` (and `claude`/`gemini`) CLI even when
  the daemon is launched from the desktop app with a stripped `PATH`, searching
  Homebrew, `~/.local/bin`, and npm/nvm/bun/volta global bins. With no usable
  OpenAI credential the error is now actionable (install the CLI, paste a key, or
  use Claude/Gemini) instead of a raw 401. Power users can opt the login token
  back in with `CROSSTHREADS_OPENAI_USE_LOGIN_TOKEN=1` (e.g. behind a gateway).

## [0.9.0] - 2026-06-24

### Added
- **Proactive weekly review** — "what you worked on, how you worked, and one
  thing to try next week," generated from the last 7 days. The web app shows a
  banner (and a 📋 badge) when a fresh review you haven't seen is ready; the
  daemon regenerates it in the background on startup once it's a week stale.
  Also `crossthreads weekly [--force]` (cron/launchd-friendly) and the
  `crossthreads_weekly_review` MCP tool. Cached in the index; degrades to the
  week's metrics with no model.

### Changed
- **Index the full width of the data.** Connectors already parsed agent
  **tool-calls** (Edit/Bash/Read…), but the store dropped them on insert —
  schema **v8** now persists per-message agent-action names (re-index to
  backfill). The behavioral metrics now leverage that plus fields that were
  stored-but-unused: **delegation** (agent actions per session + your top
  actions), **languages** you work in (from fenced code blocks), your **model
  mix**, and **median session length** (from `started_at`→`ended_at`). These
  flow into `crossthreads metrics`, the metric strip, and every behavioral
  insight, so "how you work" is grounded in much more signal.

### Notes
- MCP now exposes **21 tools**. After upgrading, run a re-index so the new
  agent-action column backfills from your existing session files.

## [0.8.0] - 2026-06-24

### Added
- **Behavioral metrics — "how you work"** — a deterministic, full-history pass
  (no model) over your *own* messages and the shape of each interaction: turns
  per task, correction/rework rate, first-prompt-miss rate, abandonment,
  opening-prompt specificity, code/error paste rates, test/commit mention rates,
  tempo by hour, and your highest-friction projects. New `crossthreads metrics`
  CLI, the `crossthreads_metrics` MCP tool, and a metrics strip in the Insights
  panel. This is the quantitative backbone the behavioral insights are grounded
  in — so they cite real numbers instead of guessing.
- **Work DNA** — a quantified, evidence-backed profile of how you actually work
  (rhythm, where friction concentrates and why, how you open tasks, signature
  habits). The 💡 button now opens this by default. (`work_dna`)
- **Gaps & blind spots** — practices largely missing (e.g. tests, commits), work
  you start but don't finish, and friction you keep absorbing — plus a
  personalized draft `CLAUDE.md` to close them. (`gaps`)
- **Prompt coach** — contrasts your smoothest vs roughest sessions (by friction)
  and teaches you with concrete before→after rewrites drawn from your own
  prompts. (`prompt_coach`)
- **Process miner** — mines procedures you keep re-deriving and turns the
  strongest into draft, installable `SKILL.md` artifacts. (`process_miner`)

All four behavioral insights are exposed as Insights tabs, `crossthreads insight
<kind>`, and MCP tools (`crossthreads_work_dna` / `_gaps` / `_prompt_coach` /
`_process_miner`). They reuse the model-auth layer; the raw metrics never need a
model. MCP now exposes **20 tools**.

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
