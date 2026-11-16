//! Crossthreads for the desktop (Tauri v2): the web app in a native window,
//! plus a menu-bar icon with a compact glance.
//!
//! The app is self-contained. `crossthreadsd`, `crossthreads` and `ct-mcp` ship
//! inside the bundle (next to this executable), and on launch the app starts
//! the daemon unless one is already running, so installing the `.dmg` is the
//! whole setup. Both windows talk to the daemon through the `rpc` command, over
//! the same local protocol as the CLI and the MCP server, presenting this
//! account's local key like every other local client.
//!
//! This crate is excluded from the headless workspace (it needs the platform
//! webview); it is built by `.github/workflows/desktop.yml` on macOS.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, RunEvent, WindowEvent,
};

use ct_daemon::{Client, Request, Response, DEFAULT_ADDR};

/// The daemon this app started, if it started one; stopped when the app quits.
struct OwnedDaemon(Mutex<Option<Child>>);

/// A program shipped inside the bundle, next to this executable.
fn bundled(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    });
    path.is_file().then_some(path)
}

fn daemon_up() -> bool {
    matches!(
        Client::new(DEFAULT_ADDR).call_timeout(&Request::Ping, Duration::from_millis(400)),
        Ok(Response::Pong)
    )
}

/// Where the daemon's log goes: `~/Library/Logs/Crossthreads` on macOS.
fn log_file() -> Option<std::fs::File> {
    let dir = if cfg!(target_os = "macos") {
        dirs_home()?.join("Library/Logs/Crossthreads")
    } else {
        dirs_home()?.join(".crossthreads/logs")
    };
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("crossthreadsd.log"))
        .ok()
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Start the bundled daemon unless one is already answering. It serves the
/// web app on 127.0.0.1:47101 too (for `crossthreads open` and phones), using
/// the copy of the UI shipped in the bundle's resources.
fn start_daemon(ui_dir: Option<PathBuf>) -> Option<Child> {
    if daemon_up() {
        return None;
    }
    let daemon = bundled("crossthreadsd")?;
    let mut cmd = Command::new(daemon);
    cmd.args(["--addr", "auto", "--http", "127.0.0.1:47101"]);
    if let Some(ui) = ui_dir.filter(|d| d.join("index.html").is_file()) {
        cmd.arg("--ui").arg(ui);
    }
    let log = log_file();
    cmd.stdin(Stdio::null())
        .stdout(
            log.as_ref()
                .and_then(|f| f.try_clone().ok())
                .map_or(Stdio::null(), Stdio::from),
        )
        .stderr(log.map_or(Stdio::null(), Stdio::from));
    cmd.spawn().ok()
}

/// Forward a JSON-encoded daemon `Request` and return the JSON `Response`.
/// Runs off the main thread, so a slow call never freezes the window, and
/// waits up to 15 seconds for a daemon that is still starting.
#[tauri::command]
async fn rpc(request: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let req: Request =
            serde_json::from_str(&request).map_err(|e| format!("bad request: {e}"))?;
        let client = Client::from_env();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match client.call(&req) {
                Ok(response) => return serde_json::to_string(&response).map_err(|e| e.to_string()),
                Err(e) if Instant::now() >= deadline => return Err(format!("{e:#}")),
                Err(_) => std::thread::sleep(Duration::from_millis(300)),
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Paths the Get started checklist shows, so its commands work as pasted.
#[tauri::command]
fn app_paths() -> serde_json::Value {
    let p = |n: &str| bundled(n).map(|p| p.to_string_lossy().into_owned());
    serde_json::json!({ "ct_mcp": p("ct-mcp"), "crossthreads": p("crossthreads") })
}

/// Run one Get started step for the user, with the bundled CLI.
/// `hooks`: install the Claude Code and Codex hooks. `skill`: install the agent
/// skill. `cli`: link the commands into `~/.local/bin`.
#[tauri::command]
async fn setup_step(step: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cli = bundled("crossthreads").ok_or("the crossthreads command isn't in this app")?;
        let args: &[&str] = match step.as_str() {
            "hooks" => &["hooks", "install"],
            "skill" => &["skill", "install"],
            "cli" => return link_cli(),
            other => return Err(format!("unknown setup step `{other}`")),
        };
        let out = Command::new(cli)
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        if out.status.success() {
            Ok(text)
        } else {
            Err(text)
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Link `crossthreads`, `crossthreadsd` and `ct-mcp` into `~/.local/bin`, the
/// same place the install script uses.
fn link_cli() -> Result<String, String> {
    let bin = dirs_home().ok_or("no home folder")?.join(".local/bin");
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let mut linked = Vec::new();
    for name in ["crossthreads", "crossthreadsd", "ct-mcp"] {
        let Some(src) = bundled(name) else { continue };
        let dst = bin.join(name);
        if dst.symlink_metadata().is_ok() {
            std::fs::remove_file(&dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        }
        symlink(&src, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        linked.push(name);
    }
    let on_path = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|p| Path::new(p) == bin);
    Ok(format!(
        "Linked {} into {}.{}",
        linked.join(", "),
        bin.display(),
        if on_path {
            ""
        } else {
            " Add it to your PATH: echo 'export PATH=\"$HOME/.local/bin:$PATH\"' >> ~/.zshrc"
        }
    ))
}

#[cfg(unix)]
fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(not(unix))]
fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::copy(src, dst).map(|_| ())
}

/// Bring up (show + focus) the full main window — invoked from the glance
/// popover's "open" button and the tray menu.
#[tauri::command]
fn open_main(app: tauri::AppHandle) {
    show_main(&app);
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Show the glance popover near the tray click, or hide it if already visible.
fn toggle_glance(app: &tauri::AppHandle, near: Option<(f64, f64)>) {
    let Some(w) = app.get_webview_window("glance") else {
        return;
    };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        return;
    }
    if let Some((x, y)) = near {
        let px = (x - 180.0).max(0.0);
        let _ = w.set_position(tauri::PhysicalPosition::new(px, y));
    }
    let _ = w.show();
    let _ = w.set_focus();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .manage(OwnedDaemon(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            rpc, open_main, app_paths, setup_step
        ])
        .on_window_event(|window, event| match event {
            // Closing the main window hides it; the app stays in the menu bar
            // (quit is on the menu).
            WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                api.prevent_close();
                let _ = window.hide();
            }
            // The glance popover dismisses itself when it loses focus.
            WindowEvent::Focused(false) if window.label() == "glance" => {
                let _ = window.hide();
            }
            _ => {}
        })
        .setup(|app| {
            // Start the bundled daemon in the background so the window opens
            // at once; `rpc` waits for it.
            let ui_dir = app.path().resource_dir().ok().map(|d| d.join("ui"));
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                if let Some(child) = start_daemon(ui_dir) {
                    *handle.state::<OwnedDaemon>().0.lock().unwrap() = Some(child);
                }
            });

            let handle = app.handle();
            let open = MenuItem::with_id(handle, "open", "Open Crossthreads", true, None::<&str>)?;
            let browser =
                MenuItem::with_id(handle, "browser", "Open in browser", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(handle)?;
            let quit = MenuItem::with_id(handle, "quit", "Quit Crossthreads", true, None::<&str>)?;
            let menu = Menu::with_items(handle, &[&open, &browser, &sep, &quit])?;

            TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().cloned().unwrap())
                .tooltip("Crossthreads")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main(app),
                    "browser" => {
                        // Signed in, through the same one-time link as
                        // `crossthreads open`.
                        if let Some(cli) = bundled("crossthreads") {
                            let _ = Command::new(cli).arg("open").spawn();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } = event
                    {
                        toggle_glance(&tray.app_handle().clone(), Some((position.x, position.y)));
                    }
                })
                .build(app)?;

            // Open the window on launch: a new install should see the app.
            show_main(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while starting Crossthreads");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // Stop the daemon only if this app started it.
            if let Some(mut child) = handle.state::<OwnedDaemon>().0.lock().unwrap().take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        // Clicking the Dock icon brings the window back.
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen { .. } = event {
            show_main(handle);
        }
    });
}
