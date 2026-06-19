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
| `ct-cli` | `crossthreads` CLI (`index`, `search`) |
| `ct-daemon` | `crossthreadsd` background daemon (scaffold) |
| `ct-mcp` | MCP server (scaffold) |
