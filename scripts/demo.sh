#!/usr/bin/env bash
# Bring Crossthreads up against a sample 4-tool corpus, for a quick local demo.
#
#   scripts/demo.sh            # deterministic offline embedder (no model download)
#   CT_ONNX=1 scripts/demo.sh  # real semantic search (downloads all-MiniLM once)
#
# Builds the binaries + UI, stages fake Claude Code / Cursor / Aider / Codex
# sessions in a temp dir, and runs the daemon with the web UI. Ctrl-C to stop;
# the temp corpus is removed on exit.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
HTTP_ADDR="${CT_HTTP:-127.0.0.1:47101}"
PROTO_ADDR="${CT_ADDR:-127.0.0.1:47100}"

FEATURES=""
[[ "${CT_ONNX:-0}" == "1" ]] && FEATURES="--features onnx"

echo "▸ Building (release${FEATURES:+, $FEATURES})…"
# shellcheck disable=SC2086
cargo build --release $FEATURES -p ct-daemon -p ct-cli -p ct-mcp

if [[ ! -d ui/dist ]]; then
  echo "▸ Building UI…"
  ( cd ui && npm install && npm run build )
fi

DEMO="$(mktemp -d)"
trap 'rm -rf "$DEMO"' EXIT
echo "▸ Staging sample corpus in $DEMO"

# Claude Code
mkdir -p "$DEMO/claude/projects/acme-api"
cat > "$DEMO/claude/projects/acme-api/s1.jsonl" <<'JSONL'
{"cwd":"/home/dev/acme-api","gitBranch":"fix/oauth","type":"user","message":{"role":"user","content":"the OAuth token refresh keeps looping on a 401, can you fix it?"},"uuid":"a1","timestamp":"2026-05-14T09:12:00Z"}
{"cwd":"/home/dev/acme-api","type":"assistant","message":{"role":"assistant","model":"claude-opus-4-8","content":[{"type":"text","text":"The refresh has no backoff. Wrap it:\n\n```rust\nbackoff::retry(|| do_refresh());\n```"}]},"uuid":"a2","timestamp":"2026-05-14T09:12:30Z"}
JSONL

# Aider
mkdir -p "$DEMO/aiderproj"
cat > "$DEMO/aiderproj/.aider.chat.history.md" <<'MD'
# aider chat started at 2026-05-16 14:00:00

> Model: gpt-4o
#### add pagination to the results table on the dashboard

Add a `usePagination` hook and a page-size selector.
MD

# Codex
mkdir -p "$DEMO/codex/sessions/2026/05/18"
cat > "$DEMO/codex/sessions/2026/05/18/rollout-1.jsonl" <<'JSONL'
{"type":"session_meta","payload":{"id":"x","timestamp":"2026-05-18T08:00:00Z","cwd":"/home/dev/acme-api","model":"gpt-5-codex"}}
{"type":"response_item","timestamp":"2026-05-18T08:00:05Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"write a unit test for the rate limiter middleware"}]}}
{"type":"response_item","timestamp":"2026-05-18T08:00:20Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Here's a test using a fake clock."}]}}
JSONL

# Cursor (needs python3 to synthesize the SQLite db; skipped if unavailable)
CURSOR_DIR="$DEMO/none"
if command -v python3 >/dev/null; then
  WS="$DEMO/Cursor/User/workspaceStorage/w1"
  mkdir -p "$WS" "$DEMO/Cursor/User/globalStorage"
  echo '{"folder":"file:///home/dev/acme-web"}' > "$WS/workspace.json"
  python3 - "$WS/state.vscdb" <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
db.execute("CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value BLOB)")
db.execute("INSERT INTO cursorDiskKV VALUES (?,?)", ("composerData:c1",
  b'{"composerId":"c1","createdAt":1747315200000,"conversation":[{"bubbleId":"b1","type":1,"text":"add a dark mode theme switcher to the top navigation bar"}]}'))
db.commit(); db.close()
PY
  CURSOR_DIR="$DEMO/Cursor/User"
else
  echo "  (python3 not found — skipping the Cursor sample)"
fi

echo "▸ Starting crossthreadsd"
echo "  Web UI:   http://$HTTP_ADDR"
echo "  Protocol: $PROTO_ADDR   (point ct-mcp / 'crossthreads --remote' here)"
echo

exec env \
  CROSSTHREADS_DB="$DEMO/index.db" \
  CLAUDE_CONFIG_DIR="$DEMO/claude" \
  CURSOR_CONFIG_DIR="$CURSOR_DIR" \
  CROSSTHREADS_AIDER_ROOTS="$DEMO/aiderproj" \
  CODEX_HOME="$DEMO/codex" \
  CROSSTHREADS_ADDR="$PROTO_ADDR" \
  "$ROOT/target/release/crossthreadsd" --http "$HTTP_ADDR" --ui ui/dist
