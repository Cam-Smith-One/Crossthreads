# Crossthreads for the desktop (Tauri)

`Crossthreads.app` is the web app in a native window, with everything it needs
inside the bundle:

```
Crossthreads.app/Contents/
  MacOS/ct-desktop        the app (named after its package: a "Crossthreads" binary would
                          clash with the `crossthreads` CLI, since Mac disks ignore case)
  MacOS/crossthreadsd     the daemon (started by the app if none is running)
  MacOS/crossthreads      the CLI (used for one-click setup and "Open in browser")
  MacOS/ct-mcp            the MCP server, for Claude Code and Codex
  Resources/ui/           the web app, served by the daemon to browsers and phones
```

On Windows the same programs are installed side by side, per user, by an NSIS
installer (`Crossthreads-<version>-windows-x64-setup.exe`), by default into
`%LOCALAPPDATA%\Crossthreads`:

```
ct-desktop.exe      the app
crossthreadsd.exe   the daemon
crossthreads.exe    the CLI
ct-mcp.exe          the MCP server
ui\                 the web app
```

## What it does

- **On launch** it starts the bundled daemon unless one is already answering on
  `127.0.0.1:47100`, and opens the main window. A daemon it started is stopped
  when you quit; one it found running is left alone, unless it is older than
  the app, in which case it is replaced. The daemon's log is in
  `~/Library/Logs/Crossthreads/crossthreadsd.log` (Windows:
  `%LOCALAPPDATA%\Crossthreads\Logs\crossthreadsd.log`). On Windows every
  program the app starts runs without a console window.
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
  MCP command it shows uses the app's own path to `ct-mcp`. On Windows the
  commands are copied into `%LOCALAPPDATA%\Crossthreads\bin` instead, and that
  folder is added to your user PATH (no administrator needed; open a new
  terminal to pick it up). The app refreshes those copies when it is updated.
  The tray icon lives in the notification area rather than the menu bar.

## Building

CI builds it: `.github/workflows/desktop.yml` runs on macOS and Windows for
pull requests that touch this folder and for releases. It builds `crossthreadsd`,
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

On Windows (x64), CI does the same with one architecture and an NSIS
installer, then launches the built `ct-desktop.exe` and checks that it replaces
an older daemon with its own. To build it yourself (in Git Bash, with Rust,
Node and the Visual Studio C++ build tools; WebView2 ships with Windows 10/11):

```sh
(cd ui && npm ci && npm run build)
cargo build --release --target x86_64-pc-windows-msvc --bin crossthreadsd --bin crossthreads --bin ct-mcp
mkdir -p src-tauri/binaries
for b in crossthreadsd crossthreads ct-mcp; do
  cp target/x86_64-pc-windows-msvc/release/$b.exe src-tauri/binaries/$b-x86_64-pc-windows-msvc.exe
done
cd src-tauri && npx @tauri-apps/cli@2 build --target x86_64-pc-windows-msvc --bundles nsis
# -> target/x86_64-pc-windows-msvc/release/bundle/nsis/Crossthreads_<version>_x64-setup.exe
```

The installer is per user (`installMode: currentUser`), so it needs no
administrator, and it downloads the WebView2 runtime only on the rare machine
without it. Uninstall from **Settings → Apps**.

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

On Windows the installer and programs are unsigned unless the repository has
the secrets `WINDOWS_CERTIFICATE` (a base64-encoded `.pfx` code-signing
certificate) and `WINDOWS_CERTIFICATE_PASSWORD`; the workflow imports the
certificate and the bundler signs with it. Unsigned, **Microsoft Defender
SmartScreen** warns the first time you run the installer ("Windows protected
your PC"): click **More info → Run anyway**.

This crate is excluded from the Rust workspace because it needs the platform
webview libraries to build.
