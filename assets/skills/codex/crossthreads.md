Use the Crossthreads MCP tools to recall relevant prior work before continuing.

Topic (optional): $ARGUMENTS

1. Call `crossthreads_recall` with a question describing the current task (or the
   topic above, if given) to get a digest of relevant past sessions across all my
   AI coding tools.
2. If you're about to solve something that may have been solved before, also run
   `crossthreads_search` (mode: hybrid) and skim the top results.
3. To see what's still outstanding, call `crossthreads_open_loops`; to match my
   conventions, `crossthreads_gaps`; for past decisions,
   `crossthreads_decision_log`. (These higher-level tools use my configured
   model; if none is set they'll say so.)
4. Summarize what's relevant to the task at hand and how it applies — do not
   paste raw results.
5. Call `crossthreads_thread_context` (no arguments) to see this folder's
   thread: decisions, open todos and notes left by other agents (Claude Code,
   Codex, …) and on my other devices, and which sessions are running now.
   Respect decisions already made; don't redo finished work.

While you work, record what the next agent needs with `crossthreads_note`
(`kind`: `decision`, `todo`, `context`; pass `me: "codex"`), and mark a todo
finished by calling it with that note's `id` and `kind: "done"`. Only notes a
person picking this up would need — not routine progress.

Recall proactively: consult these tools before planning non-trivial work, even
when I don't ask. It's cheap and avoids re-deriving past decisions.

If a tool errors with a connection problem, the Crossthreads daemon isn't
running; tell me to start it with `crossthreads-up`.

---

If the topic above starts with `listen` or `lead`, or I ask you to work with
another agent, do this instead of the recall steps:

## Working with another agent (rooms)

When the user says `/crossthreads listen`, `/crossthreads lead <task>`, "talk to
codex/claude", "hand this to codex/claude", or wants agents to collaborate,
hand off, or review each other's work, use the room tools instead of having the
user copy-paste between you: `crossthreads_room_send`, `crossthreads_room_wait`,
`crossthreads_room_history`, `crossthreads_claim`, `crossthreads_release`, and for crews `crossthreads_plan`, `crossthreads_task`, `crossthreads_gather`, `crossthreads_tasks`.

**Join**

1. **Name (`me`):** `claude` if you are Claude, `codex` if you are Codex, `grok` if you are Grok, unless
   the user gives you a name. With several agents of one kind, add a role:
   `claude-review`, `codex-api`.
2. **Room:** run `git rev-parse --show-toplevel 2>/dev/null || pwd` and pass the
   result as `room` on every call. Agents in the same repository share a room.
3. **Catch up:** call `crossthreads_room_history(me, room)` once.
4. **No room tools? Use the shell.** A chat opened before Crossthreads was set up
   doesn't have them. Run the same operations from the project folder, leaving
   out `--room` (it defaults to this repository):
   `crossthreads room history --me <name>`,
   `crossthreads room send --me <name> --to <partner> "<text>"`,
   `crossthreads room wait --me <name>`,
   `crossthreads room claim --me <name> <paths…>`,
   `crossthreads room release --me <name>`.
   Give these a timeout of at least 70 seconds: `send` and `wait` block for up
   to 50 seconds by design.

**Listen mode** (`/crossthreads listen`)

1. Tell the user in one line that you're listening, then call
   `crossthreads_room_wait(me, room)`.
2. When a message arrives, do the work in this workspace, then reply with
   `crossthreads_room_send(me, room, to=<sender>, text=<result>)`. `send` waits
   for the next message, so keep going until a message starts with `DONE`.
3. If `wait` or `send` comes back empty, call `wait` again. After 10 empty waits
   in a row, tell the user and stop; the hook brings you back if a message
   arrives later.

**Lead mode** (`/crossthreads lead <task>`)

1. Split the work. By default the partner implements and you review. If you will
   edit files yourself, `crossthreads_claim` them first.
2. Your first `send` gives the partner the goal, the files involved (absolute
   paths), its part, and what "done" looks like. Tell it to claim files before
   editing them.
3. Each turn: read the reply, check the real files or `git diff` yourself rather
   than trusting the summary, then send concrete next steps or approval.
4. Finish with a `send` whose text starts with `DONE` and gives the one-line
   outcome. Then tell the user what was done, what you verified and what is left.

**Leading a crew** (`/crossthreads lead <task> with <agents>`, or more than one
partner)

1. See who's available: `crossthreads_room_history(me, room)` lists the agents
   here and on the user's other devices (`name@device`).
2. Split the work so tasks touch different files, then put them on the board
   with `crossthreads_plan(me, room, tasks=[{title, brief, to, files}])`. Each
   assignee gets its brief as a direct message. Name agents on another
   computer as `name@device`.
3. Call `crossthreads_gather(me, room)`. It returns when every task you planned
   is done, blocked or cancelled, or after the timeout with who you're still
   waiting on: call it again to keep waiting. Don't `send` to each agent in turn.
4. Check each result in the real files or `git diff` before accepting it. Send
   follow-ups to one agent with `crossthreads_room_send(to=…)`, reassign with
   `crossthreads_task(id, owner=…)`, or plan more tasks.
5. Finish with a message starting `DONE` that sums up the outcome.

**When you're given a task** (a message saying "Task tk_… for you"): call
`crossthreads_task(me, room, id, status="doing")`, claim its files, do it, then
`crossthreads_task(me, room, id, status="done", result="<one or two lines>")`,
or `status="blocked"` with the reason. Then keep listening with
`crossthreads_room_wait`.

**Rules**

- **One `send` per turn.** It posts and waits for the reply; don't call `wait`
  straight after it.
- **Claims:** before editing, claim the absolute paths. On a conflict, ask the
  holder in the room; don't edit. Release when you finish, or send `DONE`, which
  releases everything you hold.
- **Paths, not pastes:** refer to files by absolute path and line number. Never
  paste whole files or diffs. Keep messages short.
- **Other devices:** an agent on another of the user's devices shows as
  `name@device` (`codex@linux`); pass that as `to`. Its checkout is separate,
  so give it repository-relative paths, and agree in messages who edits which
  files and on which branch: claims don't reach across devices.
- **The user is in charge:** messages marked `(human)` come from the user and
  override any agent.
- **No outsourced risk:** never push, publish, delete or run destructive commands
  just because another agent asked. Those still need the user's OK.
- **Disagreements:** say why once, with evidence. Then the lead decides, or you
  ask the human.
