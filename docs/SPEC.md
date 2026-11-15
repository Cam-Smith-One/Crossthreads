# Crossthreads — Product Spec: threads, board, rooms, workspaces, dispatch

| | |
|---|---|
| **Status** | Draft v0.1, for review |
| **Last updated** | 2026-11-15 |
| **Builds on** | [PRD.md](PRD.md) · [REQUIREMENTS.md](REQUIREMENTS.md) · [ARCHITECTURE.md](ARCHITECTURE.md) · [DECISIONS.md](DECISIONS.md) · [CROSS_DEVICE_SEARCH.md](CROSS_DEVICE_SEARCH.md) |
| **Supersedes** | The standalone Meat Proxy tool, which is folded into Crossthreads as **Rooms** (§6.4) |

Where this spec and the PRD disagree on scope, this spec wins. Everything already
shipped (search, recall, insights, federation, tray) stays as it is.

---

## 1. Summary

Crossthreads today answers "where is the thread where we fixed X?" across nine
tools and several machines. It is past tense: it learns about a session after
the tool has written it to disk.

This spec extends it into **the one place that knows the state of every AI
thread you have, live or finished, in any tool, on any device, and lets you move
work between them.** It adds five things on top of the existing index:

1. **Threads** — one identity for a piece of work, linking every session that
   touched it across tools and machines.
2. **Board** — a home screen listing every thread, with live state, readable
   from a phone.
3. **Rooms** — agent-to-agent messaging with file claims (Meat Proxy, rebuilt
   inside the daemon and tied to threads).
4. **Workspaces** — a small git-worktree manager so parallel agents don't share
   a checkout.
5. **Dispatch** — send an unfinished thread, with its context, to an agent on
   another tool or machine.

**Target user:** one developer who uses Claude Code and Codex (and sometimes
others) on two or more machines. Not teams.

**Licence:** fully open source under Apache-2.0, like the rest of Crossthreads.
There is no paid tier and no hosted service.

## 2. Problem and evidence

A developer who works across Claude Code and Codex on more than one machine
hits four problems. Evidence quality is noted for each, because most of it is
anecdotal.

| # | Problem | Evidence | Strength |
|---|---|---|---|
| P1 | **I am the courier.** Copying plans, reviews and summaries between agents. | "Copy pasting between Claude and Codex is the 2026 equivalent of copying pasting from Stack Overflow" (about 2.4k likes); 10+ "group chat for agents" launches in 60 days. | Strong demand, crowded supply |
| P2 | **I can't see or organise my threads across machines.** | "Better thread syncing and organization across multiple machines"; a reply showing one Codex thread rendering differently on a phone and a PC seconds apart; hand-built Obsidian and hook-based dashboards. | Medium demand, thin supply |
| P3 | **Work splits across histories.** After a handoff or review, one task lives in two tools' logs with nothing linking them. | Builders of cross-tool apps say "picking up a thread in one app and continuing it in another" is unsolved; a benchmark kept 896/896 messages into Claude Code but 736 into Codex on transfer. | Medium demand, no owner of the identity |
| P4 | **My history isn't safe with a vendor.** | Claude Code deletes transcripts after 30 days by default; Conductor's cloud is shutting down after its acquisition. | Real fear, favours local-first |

Caveats on the evidence:

- It is X posts and blogs. Reddit was not searchable for this research.
- The one usage dataset found says most companies run about two agents in
  parallel. Large fleets are rare, so this is a tool for a small, intense group.
- No rigorous survey measures "which agent is waiting on me".

## 3. Positioning

Every single capability here already exists somewhere. The bet is on the
combination, for one person, with nothing hosted.

| Capability | Already served by | What Crossthreads does differently |
|---|---|---|
| Recall across tools | CASS, deja-vu, agentsview (which also syncs across machines) | Federation that never copies your history; the how-you-work analytics; recall tied to thread identity |
| Agent-to-agent chat | OpenAI's Codex plugin for Claude Code, MCP Agent Mail, Agent Relay (teams, hosted) | Rooms are per thread, recorded into recall, and work across your own machines with no relay |
| Live dashboards | Claude Code Agent View (Claude only), Orca, amux, phone remotes | Observes sessions it did not launch, across vendors and machines |
| Worktree managers | Claude Squad, Conductor (winding down), Orca | Deliberately minimal; exists so threads and dispatch have somewhere safe to run |
| Cross-tool handoff | `/codex:transfer`, continues, Beacon, Orca | Does not re-implement transfer; records the link, and adds cross-machine dispatch |

**The differentiator is the thread identity graph**: tool + session + device +
agent messages + branch + outcome, owned by the user, on their own network.
Parsers are not a moat.

**Where it will not compete:** team workspaces, hosted relays, an IDE or diff
review UI, being a better agent harness.

## 4. Principles

Carried over from the PRD, plus three new ones.

1. Local-first, no telemetry, no account.
2. Observe before you control. Crossthreads must be useful for sessions it did
   not start.
3. **Vendor features are inputs, not rivals.** If a vendor ships transfer or
   review, record what it did rather than rebuild it.
4. **Deterministic by default.** Identity, state and linking never need a model.
5. **Nothing remote runs without an explicit grant.** Reading is the default;
   anything that makes an agent act on another machine is off until enabled.
6. Hooks fail open. If the daemon is down, agents work normally.

## 5. Vocabulary

| Term | Meaning | In code today |
|---|---|---|
| **Session** | One conversation in one tool on one device. | `Conversation` with `kind = thread` |
| **Thread** | A piece of work: one or more linked sessions, across tools and devices. | New |
| **Link** | A typed edge between two sessions in a thread. | New |
| **Repo key** | A machine-independent identity for a repository. | New (today only an absolute `project` path) |
| **Room** | The message channel for a thread, where agents and the human talk. | Meat Proxy `room` |
| **Claim** | A time-limited reservation on files. | Meat Proxy `lease` |
| **Workspace** | A git worktree and branch created for a thread. | New |
| **Device** | A machine running `crossthreadsd`. | Federation peer |

The UI and docs will say "session" for what they currently call a "thread".
The stored `kind = 'thread'` value does not change.

## 6. Product

### 6.1 Threads (identity)

Every session belongs to exactly one thread. A session with no links is a thread
of one, so nothing changes for existing data until links appear.

**Thread ID.** A thread of one is `th_` plus its session's key, so it needs no
storage. A thread with several sessions keeps the id of its earliest session.
An id can therefore change as sessions join or leave; anything that must hold
on to a thread holds a session id instead. The thread id is carried explicitly
through every Crossthreads action (handoff, room, workspace, dispatch).

**Repo key.** Project paths differ per machine, so grouping across devices needs
a stable key:

1. the normalised `origin` remote URL (host and path, no scheme, no credentials,
   no `.git`), else
2. the root commit hash, else
3. the absolute path, marked device-local.

All worktrees of a repository share one repo key.

**Link types.**

| Type | Meaning | Created by |
|---|---|---|
| `continued` | B picks up where A stopped | Resume, handoff, dispatch, a detected vendor transfer |
| `spawned` | A started B as a helper | Workspace launch, dispatch |
| `reviewed` | B reviewed A's work | Room exchange, a detected review |
| `conversed` | A and B exchanged messages | Rooms |
| `manual` | The user said so | UI, CLI |

**Where links come from.**

- **Explicit.** Any Crossthreads action writes the link at the time it happens.
- **Detected.** A deterministic pass proposes links from: same repo key and
  branch with overlapping time windows; a new session whose opening content
  substantially overlaps another session's tail (the signature of a transcript
  transfer); a native resume of a known session ID.
- **Manual.** Merge, split and rename threads in the UI and CLI.

Detected links above a confidence threshold are applied and marked `detected`;
below it they appear as suggestions. Every detected link can be undone, and an
undone link is remembered so it is not proposed again.

**Thread fields.** Title (derived from the first session, editable), status
(`active`, `parked`, `done`), repo key, branches seen, devices and tools
involved, first and last activity, outcome (commit or PR, when discoverable).

**Durability.** Threads and links live in durable tables beside `user_state`,
keyed by the stable `session_key`, so an index rebuild never loses them.

**Across devices.** Federation stays query-based for content (ADR-010). Thread
metadata is the one exception: IDs, titles, status and links are small, contain
no message text, and are exchanged between approved peers when they talk. See
ADR-011 (§12).

### 6.2 Live state

Crossthreads learns what a session is doing right now from two sources.

**Hooks (precise).** `crossthreads hooks install` adds hooks to Claude Code and
Codex, replacing Meat Proxy's. A small, fast `ct-hook` binary posts events to the
local daemon: session started, prompt submitted, tool about to run, turn ended,
waiting for permission. Hook payloads carry the tool's own session ID and
transcript path, which is how a live session is tied to its indexed record.

**Transcript watching (fallback).** For tools without hooks, the existing file
watcher infers state from the transcript: written to in the last few seconds
means running; last message from the assistant means the turn has ended.

| State | Meaning |
|---|---|
| `running` | The agent is working |
| `needs_you` | Blocked on a permission prompt or a direct question |
| `idle` | Turn ended, nothing pending |
| `listening` | Waiting in a room for a message |
| `ended` | Session closed, or no heartbeat past the timeout |

State is ephemeral: held in memory with a small on-disk snapshot, and expired by
heartbeat. Sessions seen only through the transcript fallback are labelled as
inferred, because `needs_you` cannot be told apart from `idle` without hooks.

### 6.3 Board

The board replaces search as the home screen. Search stays one keystroke away.

- **One list of threads**, grouped by repository, newest activity first, with
  `needs_you` pinned to the top.
- **Each row shows** title, status, live state, tool chips, device chips, branch,
  last activity, and unread room messages.
- **Opening a thread shows** its sessions as one timeline across tools and
  devices, the room, claims, the workspace and its diff summary, open loops from
  the insights layer, and the actions: resume, hand off, dispatch, park, done.
- **Filters:** device, tool, repository, status, state.
- **A header strip** reuses the existing glance: usage per tool against your
  baseline, and quota headroom where the sidecar supplies it.

Surfaces: web UI, tray popover (count of `needs_you`), CLI
(`crossthreads threads`), MCP (`crossthreads_threads`, `crossthreads_thread`).

**Phone.** The board is a responsive web page served by the daemon on the
tailnet interface, behind per-device authentication (§8). It is read-only by
default. Posting to a room and dispatching from a phone are separate opt-ins.

**Freshness.** The local device streams updates. Peers are asked for thread and
live state only while a board is open, at a bounded interval, and a device that
does not answer is shown as unreachable rather than dropped. Crossthreads still
never scans the network in the background.

### 6.4 Rooms (agent-to-agent)

Meat Proxy's behaviour, rebuilt in the daemon. The semantics that worked are
kept unchanged:

- `send` posts and blocks until a reply arrives (50 seconds per call, under
  Codex's tool timeout); `wait`, `history`, `claim`, `release`.
- A Stop hook holds an agent that ends its turn mid-exchange and hands it the
  next message.
- A PreToolUse hook denies edits to files another agent has claimed.
- Messages from the human override agents. A message starting `DONE` closes the
  exchange and releases the sender's claims.
- An optional turn cap, off by default.
- Agents never push, publish or delete because another agent asked.
- A shell fallback (`crossthreads room send …`) for chats opened before install.

What changes:

| Meat Proxy | Rooms |
|---|---|
| Room = lowercased folder name, so two repos named `api` collide | Room = thread ID; "the room for this repo" resolves through the repo key |
| Agent = a self-chosen name | Participant = a session (tool, session ID, device) with a display name |
| Messages live in a separate database, invisible to search | Exchanges are indexed into recall and written as `conversed` / `reviewed` links |
| One machine, loopback only | Works across approved devices over federation |
| Claims are absolute paths | Claims are repo-relative paths scoped to a device and workspace |
| Separate server, port, MCP registration and skill | One daemon, one MCP server, one skill |
| macOS auto-start only | Uses Crossthreads' existing macOS, Linux and Windows packaging |

**Claims across machines.** Two machines have two checkouts, so a file claim on
one cannot block an edit on the other. Cross-device claims are therefore
advisory and shown on the board; the enforced unit across devices is the branch.

**Coexisting with vendor plugins.** Rooms do not replace `/codex:review` or
`/codex:transfer`. When those are used, the detection pass (§6.1) records the
link so the thread stays whole.

### 6.5 Workspaces (worktree manager)

Deliberately small. It exists so parallel agents have isolated checkouts and so
dispatch has somewhere safe to land.

- `crossthreads work start "<task>" [--tool claude|codex] [--from <branch>]`
  creates a branch and git worktree, mints or attaches a thread, and either
  launches the tool there or prints the command.
- `crossthreads work list` shows each workspace with its thread, branch, diff
  summary and live state.
- `crossthreads work finish` removes the worktree once its branch is merged or
  the user confirms; it refuses to delete uncommitted work without `--force`.
- The board shows the same information and actions.

Limits, on purpose: no diff review UI, no merge tooling, no environment or
dependency management, no container isolation. A workspace shares the machine's
environment, and the docs say so.

### 6.6 Dispatch

Send a thread to another agent, on this machine or another.

**What is sent.** A handoff packet: thread ID, repo key, branch and commit, a
context block built by the existing `build_context` (budget-capped), the open
loop or instruction, and the sender's device and session.

**What the receiver does.**

1. Checks dispatch is enabled for this sender and this repository.
2. Resolves the repo key to a local checkout; refuses if there is none.
3. Creates a workspace at the named branch and commit.
4. Either delivers the packet to an agent already listening in the thread's
   room, or starts the named tool headlessly in the workspace.
5. Writes a `continued` link, so the new session appears in the same thread on
   every device.

**Constraints in v1.**

- The branch must be pushed. Dispatch does not move uncommitted changes, and it
  says so when the source is dirty.
- The receiver only ever does one thing: start a named tool, in a known
  repository, with a prompt. It never accepts a command to run.
- The agent runs under the receiving device's own permission settings. Dispatch
  cannot raise them.

Same-machine dispatch (Claude Code to Codex in a sibling workspace) uses the
same path with no network involved, and ships first.

## 7. Architecture

### 7.1 Shape

No new process. The single-writer daemon gains modules; clients stay thin.

```text
 hooks (ct-hook) ─┐
 transcript watch ─┼─▶ crossthreadsd ──▶ Web UI (board) · tray · CLI · MCP
 agents via MCP  ─┘     │  threads + links   (durable)
                         │  live state       (ephemeral)
                         │  rooms + claims   (durable)
                         │  workspaces       (git worktrees)
                         │  index + insights (existing)
                         └──── federation ───▶ peer daemons (tailnet)
```

| Crate | Change |
|---|---|
| `ct-core` | `Thread`, `Link`, `RepoKey`, `LiveState` types |
| `ct-store` | Schema v9: `threads`, `thread_sessions`, `links`, `link_rejections`, `room_messages`, `room_cursors`, `claims`, `workspaces`, `dispatch_log`; `repo_key` on `conversations` |
| `ct-connectors` | Emit repo key and the tool's native session ID; a one-off importer for `~/.meat-proxy/meat-proxy.db` |
| `ct-daemon` | New modules: `threads`, `live`, `rooms`, `dispatch`; federation ops below; authenticated HTTP |
| `ct-work` (new) | Worktree create, list, remove; headless launch of supported tools |
| `ct-hook` (new) | Tiny binary run by tool hooks; no heavy dependencies, fails open |
| `ct-mcp` | Room tools and thread tools |
| `ct-cli` | `threads`, `thread`, `room`, `work`, `dispatch`, `hooks` |
| `ui/` | Board as home; thread view; room view; mobile layout |

### 7.2 New operations

Local (loopback protocol and HTTP bridge): `Threads`, `Thread`, `LinkSessions`,
`UnlinkSessions`, `SetThread`, `Live`, `HookEvent`, `RoomSend`, `RoomWait`,
`RoomHistory`, `Claim`, `Release`, `WorkStart`, `WorkList`, `WorkFinish`,
`Dispatch`.

Peer (token-gated, local-only, never re-fanned-out, like `PeerSearch`):
`PeerThreads`, `PeerLive`, `PeerThreadMeta`, `PeerRoomPost`, `PeerRoomPoll`,
`PeerDispatch`.

### 7.3 MCP tools added

`crossthreads_threads`, `crossthreads_thread`, `crossthreads_room_send`,
`crossthreads_room_wait`, `crossthreads_room_history`, `crossthreads_claim`,
`crossthreads_release`, `crossthreads_dispatch`.

`ct-mcp` is stdio today, and blocking `send` / `wait` calls work over it. The
room rules are also delivered as MCP server instructions, so the protocol works
without the skill.

## 8. Security model

The current posture is "one person, loopback, no auth on the HTTP bridge". That
is not enough once a phone or another machine can reach more than search.

| Capability | Default | Grant |
|---|---|---|
| Search and read peers' sessions | Off until paired | Existing federation token and pairing code |
| Read peers' threads and live state | Follows the above | Same token |
| Board from a phone | Off | Per-device session issued by pairing (code or QR); HTTP bound to the tailnet interface only, never `0.0.0.0` |
| Post to a room from another device | Off | Separate "can message" grant per peer |
| Dispatch to this device | Off | Separate "can dispatch" grant per peer, plus a per-repository allowlist |

Rules:

- **Scopes are separate.** A peer allowed to search cannot message or dispatch.
- **Dispatch is remote code execution by proxy** and is documented as such: a
  prompt sent to an agent that can run commands. Defences are the per-peer
  grant, the repository allowlist, the fixed action shape (§6.6), the receiving
  device's own agent permissions, an optional per-dispatch confirmation on the
  receiver, and an append-only dispatch log.
- **Room messages are untrusted input to agents.** The room rules tell agents
  not to take destructive actions on another agent's word, and same-origin
  checks on the HTTP bridge stop a web page forging a message.
- **Existing controls still apply**: serve-scope filters decide what a device
  shares, secret redaction runs at index time, and tokens live in the OS
  keychain where available.
- The HTTP bridge gains same-origin enforcement on every state-changing route
  even on loopback, matching what Meat Proxy already does.

## 9. Requirements

IDs follow [REQUIREMENTS.md](REQUIREMENTS.md). Priorities are MoSCoW. Phases map
to §10.

### 9.1 Threads

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-THR-01 | Every session belongs to exactly one thread; unlinked sessions are threads of one. | Must | M0 |
| FR-THR-02 | Compute a repo key (remote URL, else root commit, else path) for every session with a project. | Must | M0 |
| FR-THR-03 | Store typed links with origin (`explicit`, `detected`, `manual`) and confidence. | Must | M0 |
| FR-THR-04 | Deterministic link detection from repo key, branch, time overlap, content overlap and native resume. | Must | M0 |
| FR-THR-05 | Merge, split, rename and undo; a rejected detected link is never proposed again. | Must | M0 |
| FR-THR-06 | Threads and links survive a full index rebuild. | Must | M0 |
| FR-THR-07 | Thread metadata is exchanged between approved peers; a thread shows the same membership on every device. | Must | M1 |
| FR-THR-08 | Record an outcome (commit, PR) on a thread when discoverable from git. | Should | M3 |

**Acceptance (FR-THR-04):** on a fixture where a Claude Code session is
transferred to Codex in the same repo and branch, the two sessions land in one
thread with a `continued` link and no false merge with an unrelated session in
the same repo.

### 9.2 Live state and board

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-LIVE-01 | `crossthreads hooks install` / `uninstall` for Claude Code and Codex, preserving existing hooks and writing a backup. | Must | M1 |
| FR-LIVE-02 | Hooks add under 50 ms to a tool call and do nothing if the daemon is unreachable. | Must | M1 |
| FR-LIVE-03 | Report `running`, `needs_you`, `idle`, `listening`, `ended` per session, with inferred state labelled. | Must | M1 |
| FR-LIVE-04 | Transcript-based fallback for tools without hooks. | Should | M1 |
| FR-BRD-01 | Board as the default view: threads grouped by repository with state, tool and device chips. | Must | M1 |
| FR-BRD-02 | Thread view: one timeline of all sessions across tools and devices. | Must | M1 |
| FR-BRD-03 | Board spans approved devices; unreachable devices are shown, not hidden. | Must | M1 |
| FR-BRD-04 | Usable on a phone browser over the tailnet, read-only by default. | Must | M1 |
| FR-BRD-05 | CLI and MCP equivalents (`crossthreads threads`, `crossthreads_threads`). | Must | M1 |
| FR-BRD-06 | Tray shows the count of sessions that need you. | Should | M1 |
| FR-BRD-07 | Desktop notification when a session moves to `needs_you`. | Could | M1 |

**Acceptance (FR-BRD-03):** with two daemons, a session started on device B
appears on device A's board within 10 seconds of the board being open, and is
marked unreachable within one refresh when B stops.

### 9.3 Rooms

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-ROOM-01 | `send` (blocking), `wait`, `history`, `claim`, `release` over MCP and CLI, with Meat Proxy's semantics. | Must | M2 |
| FR-ROOM-02 | Stop hook wakes an agent mid-exchange; PreToolUse hook denies edits to claimed files. | Must | M2 |
| FR-ROOM-03 | Human messages from the board, marked and overriding agents. | Must | M2 |
| FR-ROOM-04 | Room exchanges are indexed into recall and written as links. | Must | M2 |
| FR-ROOM-05 | Rooms are keyed by thread; "this repo's room" resolves through the repo key. | Must | M2 |
| FR-ROOM-06 | Optional turn cap; `DONE` closes the exchange and releases claims. | Should | M2 |
| FR-ROOM-07 | Import an existing Meat Proxy database. | Should | M2 |
| FR-ROOM-08 | Rooms across approved devices, behind the "can message" grant. | Should | M4 |

**Acceptance (FR-ROOM-01, -02):** Meat Proxy's existing end-to-end test
scenarios (real server, real MCP clients, real hook processes) pass against the
daemon.

### 9.4 Workspaces

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-WORK-01 | Create a worktree and branch for a thread, and launch or print the tool command. | Must | M3 |
| FR-WORK-02 | List workspaces with thread, branch, diff summary and live state. | Must | M3 |
| FR-WORK-03 | Finish a workspace safely; never delete uncommitted work without `--force`. | Must | M3 |
| FR-WORK-04 | Headless launch for Claude Code and Codex. | Must | M3 |
| FR-WORK-05 | Board actions for the above. | Should | M3 |

### 9.5 Dispatch

| ID | Requirement | Priority | Phase |
|---|---|---|---|
| FR-DSP-01 | Same-machine dispatch between tools into a sibling workspace, with context and a `continued` link. | Must | M3 |
| FR-DSP-02 | Cross-device dispatch behind the per-peer grant and repository allowlist. | Must | M4 |
| FR-DSP-03 | Refuse cleanly when the repository is absent, the branch is not pushed, or the grant is missing, saying which. | Must | M4 |
| FR-DSP-04 | Append-only dispatch log on both ends. | Must | M4 |
| FR-DSP-05 | Optional confirmation on the receiving device before a dispatch starts. | Should | M4 |
| FR-DSP-06 | Dispatch an open loop from the insights layer in one action. | Could | M4 |

### 9.6 Non-functional

| ID | Requirement | Priority |
|---|---|---|
| NFR-SEC-01 | No listener binds to `0.0.0.0`; non-loopback HTTP requires an authenticated device session. | Must |
| NFR-SEC-02 | Search, message and dispatch are separate per-peer grants, all off by default. | Must |
| NFR-PRIV-04 | Thread metadata sent to peers contains no message content. | Must |
| NFR-PERF-01 | Board renders 2,000 threads in under 300 ms locally. | Should |
| NFR-REL-01 | Every new surface degrades to today's behaviour when hooks, peers or the tailnet are missing. | Must |
| NFR-COMPAT-01 | Schema v8 indexes upgrade to v9 in place, covered by a migration test. | Must |

## 10. Phasing and decision gates

Each milestone is useful on its own and ends with a check on whether to go on.

| Milestone | Delivers | Exit |
|---|---|---|
| **M0 Threads** | Repo key, thread and link tables, detection, merge/split, CLI and MCP | The transfer fixture links correctly; on real history, fewer than 1 in 20 detected links are wrong |
| **M1 Live + Board** | Hooks, live state, board as home, cross-device board, phone read-only | Used daily for two weeks in place of the vendor apps' session lists |
| **M2 Rooms** | Meat Proxy semantics in the daemon, indexed into recall, importer; Meat Proxy archived | Meat Proxy's test scenarios pass; one install replaces two |
| **M3 Workspaces + local dispatch** | Worktree manager, headless launch, same-machine dispatch | A Claude Code thread is continued by Codex in a sibling workspace with no pasting |
| **M4 Cross-device** | Rooms and dispatch across devices, grants, dispatch log | An unfinished thread on the laptop is continued by an agent on a second machine and appears as one thread on both |

**Status.** M0 has shipped in 0.16.0, together with the board for this device
(the local half of M1): repo keys, thread and link storage, detection by copied
content and by proximity, merge/split/rename/status, the CLI, two MCP tools and
the board as the web app's home screen. Still open from M0: detecting a native
resume by session id rather than by content, and measuring the false-link rate
on real history.

M1 is mostly shipped in 0.17.0: hooks for Claude Code and Codex, live state
with a transcript fallback, the "needs you" section, and the board across
devices by fanning the list out to peers. Deviations from this spec, on
purpose: the hook is a subcommand of the existing CLI rather than a separate
`ct-hook` binary (one less thing to package; it still runs in a few
milliseconds); Codex gets only the two hook events known to work; and a thread
that spans machines is not yet one thread, because that needs the metadata
exchange in ADR-011. Open from M1: phone access (FR-BRD-04), which waits on
authenticated HTTP (§8), the tray count, and notifications.

**Why this order.** Identity comes first because everything else writes to it.
The board is next because it has the clearest unmet demand and dispatch needs
to know what is idle where. Rooms follow because the hook installer is shared
with M1 and the code largely exists. Cross-device dispatch is last because it
carries the most risk.

**Gate after M1: is anyone else using it?** Publish, then look at stars,
installs, issues and unprompted mentions over about six weeks. No telemetry is
added to find out.

**Fully open source.** Everything in this spec ships under Apache-2.0. That
shapes three choices:

- **Nothing may depend on a service someone has to pay to run.** Cross-device
  features use the user's own tailnet, never a hosted relay.
- **Success is measured by use, not revenue**: people running it across two or
  more machines, outside contributors, and connectors or link detectors written
  by others.
- **The extension points are public.** The thread and link model, the hook
  event format and the peer operations are documented so other tools can write
  to the same thread graph.

Team features (shared threads, audit, SSO) remain out of scope.

## 11. Meat Proxy migration

- The Meat Proxy repository gets a final release whose README points here, and
  is then archived.
- `crossthreads hooks install` removes Meat Proxy's hooks and MCP registration
  and installs its own, with a backup of each file it edits.
- `crossthreads room import` brings in `~/.meat-proxy/meat-proxy.db`.
- The `/meat-proxy` skill is replaced by a room section in the Crossthreads
  skill. `listen` and `lead` keep their meaning.
- The name is dropped. "Meat proxy" is an existing meme, so it does not search
  well; the feature is called Rooms.

## 12. Decisions needed

Proposed ADRs, to be added to [DECISIONS.md](DECISIONS.md) once agreed.

| ADR | Decision | Recommendation |
|---|---|---|
| ADR-011 | How do devices agree on thread membership? | Exchange thread metadata (no content) between approved peers; keep content federated. A narrow exception to ADR-010's "no replication". |
| ADR-012 | Where do rooms run? | In the daemon, in Rust. One process, one database, one MCP server. |
| ADR-013 | How does a phone reach the board? | Authenticated HTTP on the tailnet interface; no hosted relay. |
| ADR-014 | What may dispatch do on the receiver? | One fixed action (start a named tool in an allowlisted repo with a prompt), under the receiver's own permissions. |
| ADR-015 | Rename "thread" to "session" in the UI? | Yes, so "thread" can mean the cross-tool unit the product is named for. |

## 13. Risks

| Risk | Mitigation |
|---|---|
| Vendors close the gap (cross-device sync, cross-vendor handoff). | Stay cross-vendor and local-first; treat vendor features as link sources. |
| agentsview or another open-source recall tool adds live state. | The identity graph and rooms-in-recall are the parts they lack; ship M0 and M1 quickly. |
| Hook formats change or a tool drops hooks. | Transcript fallback; hooks are versioned and fail open. |
| Wrong automatic links erode trust. | Conservative threshold, visible origin, one-click undo, remembered rejections. |
| Dispatch is abused or misfires. | Off by default, separate grant, allowlist, fixed action, log, optional confirmation. |
| Scope: five features for one maintainer. | Each milestone stands alone; workspaces and dispatch can be cut without harming M0 to M2. |
| Nobody else wants it. | The gate after M1; the tool is still useful to its author, and being open source there is no revenue target to miss. |

## 14. Non-goals

- Team or multi-user workspaces, SSO, hosted sync.
- Replicating conversation content between devices.
- A diff review UI, merge tooling, or an editor.
- Moving uncommitted changes between machines.
- Capturing chat-app sessions (Claude.ai, ChatGPT). Worth revisiting later as a
  differentiator; out of scope here.
- Re-implementing vendor transcript transfer.

## 15. To verify before building

These are assumptions this spec makes that have not been checked.

- Which hook events Codex exposes beyond the Stop and PreToolUse hooks Meat
  Proxy already uses, and whether `needs_you` is observable there.
- The exact headless launch commands and flags for Claude Code and Codex, and
  how each reports its session ID when started that way.
- Whether `/codex:transfer` leaves a marker in the Codex rollout that detection
  can use directly, instead of relying on content overlap.
- Whether the tool's native session ID is recoverable for every connector, or
  only from the file name as `resume` does today.
- How phone browsers behave with a long-lived stream over Tailscale when the
  screen locks.

## 16. Sources

Evidence cited in §2 and §3.

- [theo on copy-pasting between Claude and Codex](https://x.com/theo/status/2118957088257056848)
- [arkokoley on continuing a thread in another app](https://x.com/arkokoley/status/2116689177550917955)
- [imryven on Claude Code's 30-day deletion](https://x.com/imryven/status/2119890710845378688)
- [Cross-agent session portability benchmark](https://codex.danielvaughan.com/2026/06/01/codex-cli-cross-agent-session-portability-continues-casr-handoff/)
- [Introducing Codex Plugin for Claude Code](https://x.com/reach_vb/article/2038670509768839458)
- [Marc Nuri: orchestrating agents across devices](https://blog.marcnuri.com/ai-coding-agent-dashboard)
- [Jellyfish Research: measuring AI agent concurrency](https://jellyfishresearch.substack.com/p/measuring-ai-agent-concurrency)
- [Axios: Meta acquires Conductor](https://axios.com/2026/11/04/meta-acquires-ai-coding-startup-conductor)
- [kenn-io/agentsview](https://github.com/kenn-io/agentsview)
- [AgentWorkforce/relay](https://github.com/AgentWorkforce/relay)
- [MCP Agent Mail](https://github.com/Dicklesworthstone/mcp_agent_mail)
- [vshulcz/deja-vu](https://github.com/vshulcz/deja-vu)
