#!/usr/bin/env bash
# Run everything CI runs: format check, clippy, tests, and the UI build.
# This is the one-command answer to "how do we test?".
set -euo pipefail
cd "$(dirname "$0")/.."

echo "▸ rustfmt --check"
cargo fmt --all -- --check

echo "▸ clippy (warnings = errors)"
cargo clippy --workspace --all-targets -- -D warnings

echo "▸ cargo test"
cargo test --workspace

if command -v npm >/dev/null; then
  echo "▸ UI build"
  ( cd ui && npm ci && npm run build )
else
  echo "▸ skipping UI build (npm not found)"
fi

echo "✓ all checks passed"
