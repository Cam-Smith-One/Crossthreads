# Crossthreads desktop shell (Tauri)

Native **menu-bar / tray app** (ADR-002) that wraps the `ui/` frontend. The same
UI runs as a web app (served by `crossthreadsd --http --ui`) or here as a native
app; in the native shell the frontend's `rpc()` calls the `rpc` Tauri command,
which forwards to a running `crossthreadsd` over the local protocol.

## Tray behavior

The tray icon is the primary surface (macOS uses `Accessory` activation, so
there's no dock icon — like CodexBar):

- **Left-click the tray icon** → toggles the compact **glance** popover
  (`index.html?view=glance`): per-tool usage (today vs. your baseline, this
  week) fused with the insight read (fluency score, unresolved threads, the
  optimize focus). It dismisses on blur.
- **Tray menu** → *Open Crossthreads* (full window) · *Quit*.
- **Closing the main window** hides it back to the tray (quit is on the menu).

The glance data comes from the daemon's `glance` op (`ct-daemon::glance`), which
is deterministic and computed under a read lock. Live provider quota/reset
windows are a reserved slot (`QuotaWindow`) that an on-device reader fills; the
headless build ships usage-vs-baseline meters and no fabricated quota numbers.

> ⚠️ **Excluded from the Rust workspace on purpose.** Building Tauri needs
> platform webview libraries (e.g. `webkit2gtk-4.1` + `libsoup` on Linux,
> WebView2 on Windows, WKWebView on macOS) and, on Linux, a display. It cannot
> build or run in a headless CI container, so it is **not** part of
> `cargo build --workspace`. This directory is therefore scaffolding that has
> **not** been compiled in CI — build it on a real dev machine.

## Prerequisites

- Node toolchain (for the `ui/` frontend).
- Rust + the Tauri CLI: `cargo install tauri-cli --version '^2'`.
- Platform webview deps — see https://v2.tauri.app/start/prerequisites/.
- App icons: `cargo tauri icon path/to/logo.png` (generates `icons/`).

## Run / build

```sh
# from src-tauri/
cargo tauri dev      # launches the window against the dev frontend
cargo tauri build    # produces a native installer

# the desktop app needs a daemon to talk to:
crossthreadsd        # (started separately; CROSSTHREADS_ADDR if non-default)
```

## How it connects

`src/lib.rs` exposes two commands:

```rust
#[tauri::command]
fn rpc(request: String) -> Result<String, String> // Request JSON -> Response JSON
#[tauri::command]
fn open_main(app: tauri::AppHandle)               // show + focus the full window
```

`ui/src/api.ts` detects the Tauri runtime (`window.__TAURI__`) and routes
through `invoke("rpc", …)`; in a browser it POSTs to `/api/rpc` instead. The
glance popover's "open" button calls `open_main` (and falls back to navigating
to `/` in the browser).
