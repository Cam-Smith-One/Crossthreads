# Contributing to Crossthreads

Thanks for your interest! Crossthreads is a local-first, privacy-focused indexer
and memory layer for AI coding tools. Contributions of all kinds are welcome —
bug reports, connectors, retrieval improvements, UI, and docs.

## Ground rules

- Be respectful — see [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
- Keep it **local-first and private by default**: no telemetry, no required
  network calls. Any network feature must be opt-in and clearly disclosed.
- Discuss large or architectural changes in an issue first. The product
  direction lives in [docs/PRD.md](docs/PRD.md) and decisions in
  [docs/DECISIONS.md](docs/DECISIONS.md).

## Development setup

You need **Rust** (stable) and, for the UI, **Node**. Then:

```sh
scripts/check.sh        # fmt + clippy + tests + UI build (what CI runs)
scripts/demo.sh         # run the whole stack against a sample corpus
```

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for the crate layout, the `onnx`
feature (real embeddings), and how to run the daemon, MCP server, and UI.

## Before you open a PR

1. `cargo fmt --all` — the CI checks formatting.
2. `cargo clippy --workspace --all-targets -- -D warnings` — no warnings.
3. `cargo test --workspace` — all green.
4. Add or update tests. New connector? Add a fixture and a case to
   `crates/ct-connectors/tests/regression.rs`.
5. Update docs if behavior or interfaces change.

## Adding a connector

Connectors implement the `Connector` trait in `ct-core`. A good one:

- detects its tool's data locations per-OS (with an env override for tests),
- parses defensively — skip unknown shapes, never panic on bad input,
- stamps a versioned `tool/vN` fingerprint,
- ships a committed fixture + a regression case.

Split format/JSON-shape logic into pure, unit-testable functions, kept separate
from filesystem/SQLite access (see the Cursor and Codex connectors for the
pattern).

## Commit & PR style

- Small, focused commits with descriptive messages.
- PRs should describe the change and how you verified it.
- By contributing, you agree your work is licensed under the project's
  [Apache-2.0](LICENSE) license.
