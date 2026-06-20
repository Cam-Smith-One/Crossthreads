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
# Antigravity (Markdown brain artifacts we index)
ls ~/.gemini/antigravity*/brain 2>/dev/null | head
```

Then click **re-index** in the UI (or restart `start.sh`). If a tool stores its
data somewhere not listed above, open an issue with the real path and we'll add
it. Most connectors honor an override env var (e.g. `CLAUDE_CONFIG_DIR`,
`CODEX_HOME`, `WINDSURF_CONFIG_DIR`, `ANTIGRAVITY_HOME`) if your install is
non-standard.

### Antigravity shows nothing
Antigravity's full conversation lives in an **undocumented protobuf** format;
Crossthreads indexes the readable Markdown task/plan artifacts under
`~/.gemini/antigravity*/brain/<uuid>/*.md`. If that directory is empty or your
install keeps them elsewhere, Antigravity won't appear. Paste the output of
`ls -d ~/.gemini/antigravity*/ ~/.antigravity* 2>/dev/null` in an issue and
we'll point the connector at the right place.

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
