# Crossthreads for the desktop (Tauri)

`Crossthreads.app` is the web app in a native window, with everything it needs
inside the bundle:

```
Crossthreads.app/Contents/
  MacOS/Crossthreads      the app
  MacOS/crossthreadsd     the daemon (started by the app if none is running)
  MacOS/crossthreads      the CLI (used for one-click setup and "Open in browser")
  MacOS/ct-mcp            the MCP server, for Claude Code and Codex
  Resources/ui/           the web app, served by the daemon to browsers and phones
```

## What it does

- **On launch** it starts the bundled daemon unless one is already answering on
  `127.0.0.1:47100`, and opens the main window. A daemon it started is stopped
  when you quit; one it found running is left alone. The daemon's log is in
  `~/Library/Logs/Crossthreads/crossthreadsd.log`.
- **The window** loads the same frontend as the web app. Its `rpc` command
  forwards each request to the daemon over the local protocol (off the main
  thread), presenting this account's local key like every other local client.
- **The menu-bar icon**: left-click toggles a compact glance (per-tool usage
  against your baseline, with the current insight). The menu has *Open
  Crossthreads*, *Open in browser* (signed in, via `crossthreads open`) and *Quit*.
  Closing the window keeps the app in the menu bar; clicking the Dock icon
  brings it back.
- **Settings → Get started** gains buttons that run the bundled CLI: install the
  hooks, install the agent skill, and link the commands into `~/.local/bin`. The
  MCP command it shows uses the app's own path to `ct-mcp`.

## Building

CI builds it: `.github/workflows/desktop.yml` runs on macOS for pull requests
that touch this folder and for releases. It builds `crossthreadsd`,
`crossthreads` and `ct-mcp` for both architectures, joins each with `lipo` into
`binaries/<name>-universal-apple-darwin`, builds a universal app and `.dmg`, and
then checks the bundle: both architectures in every program, the code
signature, and a launch that brings up the daemon.

To build it yourself on a Mac:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
(cd ui && npm ci && npm run build)
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --target $t --bin crossthreadsd --bin crossthreads --bin ct-mcp
done
mkdir -p src-tauri/binaries
for b in crossthreadsd crossthreads ct-mcp; do
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cp target/$t/release/$b src-tauri/binaries/$b-$t   # each architecture's pass needs its own
  done
  lipo -create target/{aarch64,x86_64}-apple-darwin/release/$b \
    -output src-tauri/binaries/$b-universal-apple-darwin
done
cd src-tauri && npx @tauri-apps/cli@2 build --target universal-apple-darwin --bundles app,dmg
```

For a quick build for just your Mac, build the three programs without
`--target`, copy them to `src-tauri/binaries/<name>-<your target triple>` (see
`rustc -vV`), and run `npx @tauri-apps/cli@2 build`. `npx @tauri-apps/cli@2
dev` runs it against the Vite dev server.

## Signing

Without Apple credentials the bundle is ad-hoc signed (`signingIdentity: "-"`),
so macOS asks you to confirm the first time you open it (right-click → Open).
To produce a signed, notarized app, add the repository secrets
`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`APPLE_ID`, `APPLE_PASSWORD` (an app-specific password) and `APPLE_TEAM_ID`; the
workflow passes them to the Tauri bundler. That needs an Apple Developer
account.

This crate is excluded from the Rust workspace because it needs the platform
webview libraries to build.
