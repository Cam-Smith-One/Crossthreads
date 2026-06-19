<!-- Thanks for contributing! Keep PRs focused; discuss large changes in an issue first. -->

## What & why

<!-- What does this change, and what problem does it solve? Link any issue. -->

## How I verified

<!-- Commands run, manual testing, screenshots for UI. -->

- [ ] `cargo fmt --all` (CI checks formatting)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Added/updated tests (and a regression case for connector changes)
- [ ] Updated docs if behavior/interfaces changed
- [ ] Keeps Crossthreads local-first (no required network / telemetry)
