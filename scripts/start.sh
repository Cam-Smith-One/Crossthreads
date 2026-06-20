#!/usr/bin/env bash
# Crossthreads — one command to index your real sessions and open the app.
#
#   scripts/start.sh             # fast deterministic search (no model download)
#   CT_ONNX=1 scripts/start.sh   # real semantic search (downloads all-MiniLM once)
#
# Builds the daemon + UI, then runs the daemon against the AI coding tools you
# already have installed (Claude Code, Codex, Cursor, Aider, Cline, Copilot,
# Gemini CLI, Windsurf, Antigravity). It indexes in the background and opens the
# web app in your browser. Ctrl-C to stop.
set -euo pipefail

cd "$(dirname "$0")/.."
HTTP_ADDR="${CT_HTTP:-127.0.0.1:47101}"
URL="http://${HTTP_ADDR}"

FEATURES=()
[[ "${CT_ONNX:-0}" == "1" ]] && FEATURES=(--features onnx)

echo "▸ Building Crossthreads${FEATURES:+ (${FEATURES[*]})}…"
cargo build --release "${FEATURES[@]}" -p ct-daemon

if [[ ! -d ui/dist ]]; then
  echo "▸ Building the web UI…"
  ( cd ui && npm install && npm run build )
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
exec ./target/release/crossthreadsd --http "$HTTP_ADDR" --ui ui/dist
