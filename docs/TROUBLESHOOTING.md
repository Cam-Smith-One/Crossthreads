# Troubleshooting

Common issues and fixes. Most have a one-line cause. If something here doesn't
cover it, open an issue with the terminal output (and the `warn:` lines the
daemon prints).

---

## Install & build

### `scripts/start.sh: FEATURES[@]: unbound variable` (macOS)
macOS ships **bash 3.2**, which errors on empty-array expansion under `set -u`.
Fixed in current `start.sh`/`demo.sh` — `git pull` to get it. If you're on an
old checkout, update, or run the daemon directly (see *Run without the script*).

### `zsh: parse error near ')'`
You pasted lines containing `# … (…)` into interactive **zsh**, which (unlike
bash) doesn't treat `#` as a comment by default — so the `()` is parsed as code.
Paste **comment-free** commands, one per line.

### `error: package requires rustc 1.88` / `rustup` toolchain too old
Some dependencies need a recent compiler. Update:
```sh
rustup update stable
```

### Build fails on the UI / `vite: command not found`
The web UI needs **Node** (18+). Install Node, then `scripts/start.sh` builds
`ui/` for you (it runs `npm install` only when `node_modules` is missing).

### `error: linker 'cc' not found` (Linux)
Install a C toolchain: `sudo apt-get install build-essential` (Debian/Ubuntu).

---

## Running the app

### The browser shows nothing / "can't be reached" at `127.0.0.1:47101`
The **daemon stopped**. `start.sh` runs the server in the **foreground** of its
terminal, so any command you type there (or closing the tab) stops it.

- Keep the `start.sh` terminal running and untouched.
- Use a **second terminal tab** (Cmd+T) for other commands.
- To restart: `scripts/start.sh` again (the index is already built, so it's fast).

### `Address already in use` / port 47101 busy
A daemon is already running. Either use it, or stop the old one and restart.
Pick another port:
```sh
CT_HTTP=127.0.0.1:47105 scripts/start.sh
```

### Run the daemon without the script (e.g. in the background)
```sh
cargo build --release -p ct-daemon
./target/release/crossthreadsd --http 127.0.0.1:47101 --ui ui/dist
```

### First load is slow / the page is empty for a minute
A large first index (hundreds of sessions + embeddings) runs in the
**background**; the server is reachable immediately and results fill in as it
indexes. The terminal prints `initial index complete` with per-tool counts when
it's done.

---

## The UI looks old (no logo / theme toggle / tags)
You're being served a **stale `ui/dist`**. It's git-ignored, so `git pull`
doesn't refresh it. Current `start.sh` rebuilds it every run; if you're not
using the script, rebuild and hard-refresh:
```sh
cd ui && npm run build && cd ..
```
Then **Cmd+Shift+R** (hard refresh) in the browser — a normal refresh can serve
the cached old bundle.

---

## Indexing & connectors

### A tool I use isn't in the "all tools" dropdown
A tool only appears once it has **indexed conversations**. If it's missing, the
connector found no data at the expected path. Check what's actually on disk and
re-index. Useful probes:

```sh
# Claude Code
ls ~/.claude/projects 2>/dev/null | head
# Codex
ls ~/.codex/sessions 2>/dev/null | head
# Cursor (macOS)
ls ~/Library/Application\ Support/Cursor/User/globalStorage/state.vscdb 2>/dev/null
# Gemini CLI
ls ~/.gemini/tmp/*/chats 2>/dev/null | head
# Antigravity (Markdown artifacts we index, anywhere under the home)
ls ~/.gemini/antigravity* 2>/dev/null | head
```

Then click **re-index** in the UI (or restart `start.sh`). If a tool stores its
data somewhere not listed above, open an issue with the real path and we'll add
it. Most connectors honor an override env var (e.g. `CLAUDE_CONFIG_DIR`,
`CODEX_HOME`, `WINDSURF_CONFIG_DIR`, `ANTIGRAVITY_HOME`) if your install is
non-standard.

### Antigravity shows nothing
Antigravity's full conversation lives in an **undocumented protobuf** format;
Crossthreads indexes the readable Markdown task/plan artifacts that Antigravity
writes under its home (`~/.gemini/antigravity/`). The connector scans the whole
home and treats any folder holding `.md` files as a conversation, so it doesn't
matter whether they sit in `brain/<uuid>/`, `conversations/<uuid>/`, or directly
under the home. If nothing shows, there are likely no Markdown artifacts yet (a
brand-new install, or a version that only keeps `.pb`). Point the connector at a
non-standard location with `ANTIGRAVITY_HOME=/path/to/antigravity`.

### Skill search returns nothing
Skills/prompts aren't a separate **tool** — they're indexed under their tool
(`claude-code` / `codex`) as `kind = skill`. Search them via the **kind**
dropdown (set it to *skills*). They come from `~/.claude/skills/**/SKILL.md` and
`~/.codex/prompts/*.md`; if you have none of those files, there's nothing to
find.

### `warn: cursor — skipped 143 session(s): tab had no usable messages`
Benign. Editors accumulate empty/placeholder chat tabs; the connector skips
them and aggregates the reasons rather than flooding the log. The count of
**indexed** conversations is what matters.

### Cursor / Windsurf are open — is that a problem?
No. Connectors open `state.vscdb` **read-only**, so running editors are fine.
Occasionally a locked DB is skipped for that pass and picked up on the next one.

---

## Search
### Semantic search feels weak / too literal
Prebuilt binaries and the default build use a **deterministic offline embedder**
(fast, no download) — good for keyword/hybrid, weaker on pure meaning. For real
semantic search, build with ONNX/all-MiniLM:
```sh
CT_ONNX=1 scripts/start.sh
```
The model downloads once (~90 MB) and needs network for that first run.

### `build_context` / "Build context" looks truncated
Intentional — the context block is capped (default ~6,000 chars) so it fits an
agent's window. Raise it with `--max-chars` on the CLI or the `max_chars`
argument on the MCP/HTTP `context` call.

---

## Models & insights

### macOS keeps asking to use the "crossthreads" Keychain item
macOS grants Keychain access **per binary**. After you install a new version, the
freshly downloaded `crossthreadsd` isn't the same binary you previously trusted,
so it asks again. Click **Always Allow** (not just *Allow*) the first time each
stored secret is read — there are only a few (the model key and, if you paired
devices, the federation token). After that it stays quiet for that version. The
daemon caches each secret in memory for its lifetime, so a single insight no
longer triggers a burst of prompts.

### Insights / "Name with AI" say a model is needed
The 💡 insights and the theme map's **✨ Name with AI** call a model. Set one up
in **Settings → Models** (paste a key, or reuse your Claude Code / Codex / Gemini
login), or check `crossthreads llm-auth` from the terminal. Search, recall, and
the offline theme labels work with no model at all.

### An insight is slow or feels thin
Insights synthesize your **whole history** — every tool plus your skills/prompts
(up to a few hundred records). That's a larger model call than search, so it
takes a few seconds. Scope it down with `--limit N` on the CLI
(`crossthreads insight open_loops --limit 50`) if you want a faster, narrower
pass.

---

## MCP server (agents)

### The MCP tools fail / "connecting to daemon … connection refused"
`ct-mcp` **auto-starts `crossthreadsd`** on the first tool call, so normally you
don't need to keep a daemon running. For that to work, `crossthreadsd` must be
findable: either next to `ct-mcp` (it is after `install.sh`, or in
`target/release` from source) or on your `PATH`. If the tools still error:
- Make sure you registered the **absolute path** to `ct-mcp` (so it can find its
  sibling `crossthreadsd`).
- It connects on `CROSSTHREADS_ADDR` (default `127.0.0.1:47100`); set that env
  var if you point the daemon elsewhere.
- You can always start one yourself (`crossthreads-up` / `scripts/start.sh`); the
  MCP server will reuse it. There is only ever **one** daemon per index.

### Registering it with an agent
Point your MCP client at the `ct-mcp` binary (after `install.sh` it's on your
PATH; from source it's `target/release/ct-mcp`). For example:
```jsonc
{ "mcpServers": { "crossthreads": { "command": "ct-mcp" } } }
```
Tools exposed: `crossthreads_search`, `crossthreads_recall` (takes a `question`),
`crossthreads_build_context`, and `crossthreads_status`. See
[AGENT_API](AGENT_API.md) for the full schemas.

### The agent has the tools but never uses them
Install the **agent skill**, which tells the agent *when* to reach for
Crossthreads (recall at task start, search before re-solving):
```sh
crossthreads skill install
```
It writes a Claude Code `SKILL.md` and a Codex `/crossthreads` prompt. The skill
still needs the MCP server configured and the daemon running to do anything.

---

## Searching across devices

### A search only returns hits from this machine
Cross-device search needs each machine set up and connected first — install
Crossthreads on each, join them to one Tailscale network, bind each daemon to
that network with a name and shared token (`--addr 100.x.y.z:47100
--device-name <name> --fed-token <secret>`), then on your main device open
**Settings → Devices → Discover my devices** and approve them (or list peers
with `--peer NAME=ADDR`). The full walkthrough (and a per-step troubleshooting
list) is in [MULTI_DEVICE_SETUP.md](MULTI_DEVICE_SETUP.md). If a device is found
but searches skip it, its daemon is likely down or the shared `--fed-token`
doesn't match — offline peers are skipped by design, never fatal.

---

## Data, privacy & reset

### Where is the index stored?
One SQLite file under your platform data dir:

- macOS: `~/Library/Application Support/crossthreads/index.db`
- Linux: `~/.local/share/crossthreads/index.db`
- Windows: `%APPDATA%\crossthreads\index.db`

Override with `CROSSTHREADS_DB=/path/to/index.db`.

### Wipe the index and start clean
Stop the daemon, delete the DB, restart:
```sh
rm -f ~/Library/Application\ Support/crossthreads/index.db*   # macOS
scripts/start.sh
```
Bookmarks, pins, notes, and tags live in the same file, so deleting it clears
them too. "Forgotten" tombstones are also in that file.

### Does anything leave my machine?
No telemetry, no account. The only optional network call is the one-time
embedding-model download when you opt into `CT_ONNX=1`. Secrets in your
conversations (API keys, tokens, private keys) are **redacted before indexing**,
so they never reach the index.

### I forgot a thread but it came back
"Forget" tombstones a conversation so re-indexing won't re-add it — but if you
wiped the DB, the tombstone went with it. Forget it again.

---

## Diagnostics to include in a bug report

```sh
# Per-tool / kind counts (run in a SECOND terminal while the daemon is up)
curl -s -X POST http://127.0.0.1:47101/api/rpc \
  -H 'content-type: application/json' -d '{"op":"status"}'

# Versions
rustc --version; node --version; uname -a
```

Plus the `warn:` lines and the `initial index complete` summary from the
terminal running `start.sh`.
