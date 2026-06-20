# Development

## Build & test

```sh
cargo build --workspace          # core build (deterministic embedder)
cargo test  --workspace          # all tests, no network/model required
cargo clippy --workspace --all-targets
```

The default build uses a **deterministic offline embedder** (`hash-bow-v1`) so
the workspace builds fast and CI needs no model or network. It exercises the
full vector-storage + RRF-fusion pipeline but does **not** capture true
semantics.

## Real semantic embeddings (`onnx` feature)

Genuine sentence embeddings use `fastembed` (all-MiniLM-L6-v2, 384-dim) over a
bundled ONNX runtime:

```sh
cargo build -p ct-cli --features onnx
crossthreads index
crossthreads search "login keeps failing" --mode semantic
```

On a normal machine `fastembed` downloads the model on first use.

### Locked-down environments (no rustls trust for the proxy)

If the model auto-download fails with a TLS `UnknownIssuer` error (an
intercepting proxy whose CA `curl` trusts but Rust's rustls does not), pre-seed
an hf-hub-style cache with `curl` and run offline:

```sh
CACHE=$HOME/.fastembed_cache
REPO="$CACHE/models--Qdrant--all-MiniLM-L6-v2-onnx"
REV=local
mkdir -p "$REPO/snapshots/$REV" "$REPO/refs"
printf '%s' "$REV" > "$REPO/refs/main"
BASE="https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/resolve/main"
for f in tokenizer.json config.json special_tokens_map.json tokenizer_config.json model.onnx; do
  curl -sSL "$BASE/$f" -o "$REPO/snapshots/$REV/$f"
done

export FASTEMBED_CACHE_DIR="$CACHE" HF_HUB_OFFLINE=1
crossthreads index && crossthreads search "..." --mode hybrid
```

## Workspace layout

| Crate | Role |
|---|---|
| `ct-core` | Normalized schema, `Connector` trait, content hashing |
| `ct-connectors` | Source-tool parsers (Claude Code, Cursor) |
| `ct-embed` | `Embedder` trait + hash (default) and ONNX (`onnx`) backends |
| `ct-store` | Single SQLite index: FTS5 lexical + vector + RRF hybrid search |
| `ct-index` | Indexing orchestration (connectors → store → embeddings), shared by CLI + daemon |
| `ct-daemon` | `crossthreadsd` single-writer daemon + loopback API + file watcher |
| `ct-cli` | `crossthreads` CLI (`index`, `search`, `status`; `--remote`) |
| `ct-index` orchestration is shared by both | |
| `ct-mcp` | MCP server: exposes search/status to MCP-capable agents |

## MCP server (agents)

`ct-mcp` speaks JSON-RPC 2.0 over stdio and forwards tool calls to a running
`crossthreadsd`. Register it with an MCP-capable agent (e.g. Claude Code):

```json
{
  "mcpServers": {
    "crossthreads": {
      "command": "/path/to/ct-mcp",
      "env": { "CROSSTHREADS_ADDR": "127.0.0.1:47100" }
    }
  }
}
```

Tools exposed: `crossthreads_search` (lexical/semantic/hybrid),
`crossthreads_recall` (answer-oriented digest), `crossthreads_build_context`
(paste-ready markdown for injecting prior work), and `crossthreads_status`.
`ct-mcp` **auto-starts `crossthreadsd`** on first use (and reuses one already
running), so you don't need to keep a daemon up yourself — just register the
absolute path to `ct-mcp`. See `docs/AGENT_API.md` §7.

## Running the daemon

```sh
cargo run -p ct-daemon --bin crossthreadsd     # indexes, watches, serves on 127.0.0.1:47100
crossthreads status --remote                   # ask the daemon
crossthreads search "..." --remote --mode hybrid
```

Override the address with `CROSSTHREADS_ADDR` (or `--addr`), the DB with
`CROSSTHREADS_DB` (or `--db`), and disable watching with `--no-watch`.

## Web UI / desktop frontend

The UI (`ui/`) is a React + Vite app. It is the same frontend the native Tauri
shell wraps; the daemon can also serve it directly over HTTP so it runs in a
browser without the native toolchain.

```sh
cd ui && npm install && npm run build      # -> ui/dist
# from the repo root:
crossthreadsd --http 127.0.0.1:47101 --ui ui/dist
# open http://127.0.0.1:47101
```

Dev mode with hot reload (proxies `/api` to a running daemon on :47101):

```sh
crossthreadsd --http 127.0.0.1:47101 &
cd ui && npm run dev
```

The HTTP bridge exposes `POST /api/rpc` (body = a daemon `Request`, e.g.
`{"op":"search","query":"…","mode":"hybrid"}`) and serves the built UI for all
other paths (SPA fallback).

### Native Tauri shell

The native window (ADR-002) wraps `ui/` with a Rust backend that calls the same
daemon. Building it needs a Node toolchain **and** platform webview libraries
(e.g. `webkit2gtk` on Linux) plus a display — so it can't be built/run in a
headless CI container. The web-served path above is the verifiable equivalent
during development.

## Releasing prebuilt binaries

Users install without a Rust/Node toolchain via prebuilt release tarballs
(`scripts/install.sh`). To cut a release, push a `v*` tag:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

`.github/workflows/release.yml` then builds `crossthreadsd`, `crossthreads`, and
`ct-mcp` plus the web UI for Linux x64 and macOS (arm64 + x64), packages each as
`crossthreads-<tag>-<target>.tar.gz` (binaries under `bin/`, the built UI under
`ui/`, and a `crossthreads-up` launcher), and attaches them to the GitHub
Release. You can also run the workflow manually (`workflow_dispatch`) with a tag
input. Release binaries use the offline embedder; ONNX/all-MiniLM semantic
search is a source build (`--features onnx`).
