#!/usr/bin/env bash
# Crossthreads — one command to index your real sessions and open the app.
#
#   scripts/start.sh             # fast deterministic search (no model download)
#   CT_ONNX=1 scripts/start.sh   # real semantic search (downloads all-MiniLM once)
#
# Builds the daemon + CLI + MCP server + UI, runs the daemon against the AI
# coding tools you already have (Claude Code, Codex, Cursor, Aider, Cline,
# Copilot, Gemini CLI, Windsurf, Antigravity), installs the agent skill, and
# opens the web app. Ctrl-C to stop.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
PROTO_ADDR="${CT_ADDR:-127.0.0.1:47100}"
HTTP_ADDR="${CT_HTTP:-127.0.0.1:47101}"
URL="http://${HTTP_ADDR}"

# Plain string (not an array) so an empty value is safe under `set -u` on the
# bash 3.2 that ships with macOS. The flag has no spaces, so word-splitting the
# unquoted expansion below is intentional.
FEATURES=""
[[ "${CT_ONNX:-0}" == "1" ]] && FEATURES="--features onnx"

echo "▸ Building Crossthreads${FEATURES:+ ($FEATURES)}…"
# shellcheck disable=SC2086
cargo build --release $FEATURES -p ct-daemon -p ct-cli -p ct-mcp

if [[ ! -d ui/dist ]]; then
  echo "▸ Building the web UI…"
  ( cd ui && npm install && npm run build )
else
  # Always refresh the built UI so it matches the current source (dist is
  # git-ignored, so a `git pull` won't update it). Fast; installs only if needed.
  echo "▸ Refreshing the web UI…"
  ( cd ui && { [[ -d node_modules ]] || npm install; } && npm run build )
fi

# Install the agent skill (idempotent — skips if already present).
./target/release/crossthreads skill install || true

# Finishing step the user does once: register the MCP server with their agent.
echo
echo "▸ To let your agents query Crossthreads, register the MCP server once:"
echo "    claude mcp add crossthreads -s user -- \"$ROOT/target/release/ct-mcp\""
echo "  (the MCP server auto-starts the daemon, so nothing else to keep running)"
echo

# If a daemon already owns the protocol port (e.g. one the MCP server started),
# reuse it — there must be only one writer on the index. Just open the browser.
host="${PROTO_ADDR%%:*}"; port="${PROTO_ADDR##*:}"
if (exec 3<>"/dev/tcp/${host}/${port}") 2>/dev/null; then
  exec 3>&- 2>/dev/null || true
  echo "▸ A Crossthreads daemon is already running on ${PROTO_ADDR} — reusing it."
  case "$(uname -s)" in
    Darwin) open "$URL" ;;
    Linux)  command -v xdg-open >/dev/null && xdg-open "$URL" >/dev/null 2>&1 || true ;;
  esac
  echo "  Opened ${URL}"
  exit 0
fi

# Open the browser once the server is reachable (best effort, in the background).
(
  for _ in $(seq 1 60); do
    if curl -fsS "${URL}/" >/dev/null 2>&1; then
      case "$(uname -s)" in
        Darwin) open "$URL" ;;
        Linux)  command -v xdg-open >/dev/null && xdg-open "$URL" >/dev/null 2>&1 || true ;;
      esac
      break
    fi
    sleep 0.5
  done
) &

echo "▸ Starting Crossthreads — the app will open at ${URL}"
echo "  (first index of a large history can take a minute; search works as it fills in)"
exec ./target/release/crossthreadsd --addr "$PROTO_ADDR" --http "$HTTP_ADDR" --ui ui/dist
