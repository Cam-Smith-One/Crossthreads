//! Crossthreads for the desktop (Tauri v2): the web app in a native window,
//! plus a menu-bar icon with a compact glance.
//!
//! The app is self-contained. `crossthreadsd`, `crossthreads` and `ct-mcp` ship
//! inside the bundle (next to this executable), and on launch the app starts
//! the daemon unless one is already running, so installing the `.dmg` (or, on
//! Windows, running the installer, which puts the same programs side by side
//! in the install folder) is the whole setup. Both windows talk to the daemon
//! through the `rpc` command, over the same local protocol as the CLI and the MCP server, presenting this
//! account's local key like every other local client.
//!
//! This crate is excluded from the headless workspace (it needs the platform
//! webview); it is built by `.github/workflows/desktop.yml` on macOS and Windows.

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

/// A `Command` for a program this app runs in the background. On Windows the
/// bundled programs are console programs, so without `CREATE_NO_WINDOW` each
/// one would flash (or, for the daemon, keep open) a console window.
fn background_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn daemon_up() -> bool {
    matches!(
        Client::new(DEFAULT_ADDR).call_timeout(&Request::Ping, Duration::from_millis(400)),
        Ok(Response::Pong)
    )
}

/// Where the daemon's log goes: `~/Library/Logs/Crossthreads` on macOS,
/// `%LOCALAPPDATA%\Crossthreads\Logs` on Windows.
fn log_file() -> Option<std::fs::File> {
    let dir = if cfg!(target_os = "macos") {
        dirs_home()?.join("Library/Logs/Crossthreads")
    } else if cfg!(windows) {
        local_app_data()?.join("Crossthreads").join("Logs")
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
    // Windows has no `HOME` unless something (Git Bash) sets it.
    let fallback = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os(fallback))
        .map(PathBuf::from)
}

/// `%LOCALAPPDATA%` (used on Windows): per-user app data on this machine.
fn local_app_data() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| dirs_home().map(|h| h.join("AppData").join("Local")))
}

/// This app's version, which is also the version of the daemon it bundles.
const BUNDLED_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `major.minor.patch` as numbers, for comparing versions.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v
        .trim()
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

/// The running daemon's version. `None` means it is older than the `version`
/// request (before 0.22.1), so certainly older than this app.
fn running_version() -> Option<String> {
    match Client::new(DEFAULT_ADDR).call_timeout(&Request::Version, Duration::from_secs(2)) {
        Ok(Response::Version { version }) => Some(version),
        _ => None,
    }
}

/// Is the running daemon older than the one in this app? Then it may not
/// understand what this app asks of it, and must be replaced.
fn running_is_older() -> bool {
    match running_version().as_deref().and_then(parse_version) {
        Some(running) => parse_version(BUNDLED_VERSION).is_some_and(|ours| running < ours),
        None => true,
    }
}

/// Stop every `crossthreadsd` of this account (an older one left by a
/// terminal install, or started by an older MCP server), and wait for the
/// port to free up.
fn stop_running_daemon() {
    #[cfg(unix)]
    let _ = Command::new("pkill").args(["-x", "crossthreadsd"]).status();
    #[cfg(windows)]
    let _ = background_command("taskkill")
        .args(["/IM", "crossthreadsd.exe", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon_up() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Start the bundled daemon unless an up-to-date one is already answering. A
/// daemon older than this app is replaced: only one may own the index, and an
/// old one doesn't understand the newer app. It serves the web app on
/// 127.0.0.1:47101 too (for `crossthreads open` and phones), using the copy of
/// the UI shipped in the bundle's resources.
fn start_daemon(ui_dir: Option<PathBuf>) -> Option<Child> {
    let daemon = bundled("crossthreadsd")?;
    if daemon_up() {
        if !running_is_older() {
            return None;
        }
        eprintln!(
            "Crossthreads: replacing an older daemon ({}) with this app's {BUNDLED_VERSION}",
            running_version().unwrap_or_else(|| "before 0.22.1".into())
        );
        stop_running_daemon();
    }
    let mut cmd = background_command(daemon);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_as_numbers() {
        assert!(parse_version("0.9.0") < parse_version("0.22.1"));
        assert!(parse_version("0.22.1") < parse_version("0.22.10"));
        assert_eq!(parse_version("1.2.3-beta"), Some((1, 2, 3)));
        assert_eq!(parse_version("nonsense"), None);
    }
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
/// skill. `cli`: link the commands into `~/.local/bin` (on Windows: copy them
/// into `%LOCALAPPDATA%\Crossthreads\bin` and put that on the user's PATH).
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
        let out = background_command(cli)
            .args(args)
            .stdin(Stdio::null())
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
#[cfg(not(windows))]
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

#[cfg(all(not(unix), not(windows)))]
fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::copy(src, dst).map(|_| ())
}

/// The programs `link_cli` puts on the PATH on Windows.
#[cfg(windows)]
const CLI_PROGRAMS: [&str; 3] = ["crossthreads", "crossthreadsd", "ct-mcp"];

/// Where `link_cli` copies the commands on Windows: a folder of their own, so
/// the user's PATH doesn't depend on where the app is installed (or run from).
#[cfg(windows)]
fn cli_dir() -> Option<PathBuf> {
    Some(local_app_data()?.join("Crossthreads").join("bin"))
}

/// Windows: copy `crossthreads.exe`, `crossthreadsd.exe` and `ct-mcp.exe` into
/// `%LOCALAPPDATA%\Crossthreads\bin` and add that folder to the user's PATH
/// (HKCU\Environment, so no administrator is needed). Symlinks on Windows need
/// developer mode or an administrator, hence copies; the app refreshes them on
/// each launch (`refresh_cli_copies`) so they follow app updates. Running it
/// again is harmless.
#[cfg(windows)]
fn link_cli() -> Result<String, String> {
    let bin = cli_dir().ok_or("no %LOCALAPPDATA% folder")?;
    std::fs::create_dir_all(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    let mut copied = Vec::new();
    for name in CLI_PROGRAMS {
        let Some(src) = bundled(name) else { continue };
        let dst = bin.join(format!("{name}.exe"));
        copy_replacing(&src, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        copied.push(name);
    }
    let path_note = match add_to_user_path(&bin) {
        Ok(true) => " Added it to your PATH; open a new terminal to use them.".to_string(),
        Ok(false) => " It was already on your PATH.".to_string(),
        Err(e) => format!(" Couldn't add it to your PATH ({e}); add it yourself."),
    };
    Ok(format!(
        "Copied {} into {}.{path_note}",
        copied.join(", "),
        bin.display()
    ))
}

/// Copy `src` over `dst` even while `dst` is running: Windows won't overwrite
/// or delete a running program, but it will rename one, so the old copy is
/// moved aside first (and cleaned up next time).
#[cfg(windows)]
fn copy_replacing(src: &Path, dst: &Path) -> std::io::Result<()> {
    let new = dst.with_extension("exe.new");
    let old = dst.with_extension("exe.old");
    std::fs::copy(src, &new)?;
    let _ = std::fs::remove_file(&old);
    if dst.exists() {
        if let Err(e) = std::fs::remove_file(dst) {
            std::fs::rename(dst, &old).map_err(|_| e)?;
        }
    }
    std::fs::rename(&new, dst)
}

/// Keep the copies `link_cli` made in step with this app (after an update),
/// if the user made them. Best effort: nothing to report if it fails.
#[cfg(windows)]
fn refresh_cli_copies() {
    let Some(bin) = cli_dir() else { return };
    for name in CLI_PROGRAMS {
        let dst = bin.join(format!("{name}.exe"));
        let Some(src) = bundled(name) else { continue };
        if !dst.is_file() {
            continue;
        }
        // On Windows `fs::copy` keeps the modified time, so a copy is current
        // when it is the same size and no older than the bundled program.
        let current = match (std::fs::metadata(&src), std::fs::metadata(&dst)) {
            (Ok(a), Ok(b)) => {
                a.len() == b.len()
                    && matches!((a.modified(), b.modified()), (Ok(s), Ok(d)) if d >= s)
            }
            _ => false,
        };
        if !current {
            let _ = copy_replacing(&src, &dst);
        }
    }
}

/// Add `dir` to the user's PATH (HKCU\Environment\Path) unless it is there
/// already. Returns whether it changed anything. Writes REG_EXPAND_SZ, the
/// type Windows itself uses, so entries like `%USERPROFILE%\bin` still expand,
/// then tells running programs the environment changed, so a terminal opened
/// from the Start menu or Explorer sees the new PATH without signing out.
#[cfg(windows)]
fn add_to_user_path(dir: &Path) -> Result<bool, String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ};
    use winreg::{RegKey, RegValue};

    let (env, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        .map_err(|e| e.to_string())?;
    // The raw value, unexpanded. A missing `Path` is fine (we create it), but
    // one we can't read must not be overwritten.
    let current: String = match env.get_value("Path") {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("can't read your PATH: {e}")),
    };
    let normalize = |p: &str| {
        // Compare expanded, case-insensitively, without a trailing slash.
        let mut p = p.trim().to_string();
        if let Some(lad) = std::env::var_os("LOCALAPPDATA") {
            p = p.replace("%LOCALAPPDATA%", &lad.to_string_lossy());
        }
        p.trim_end_matches(['\\', '/']).to_lowercase()
    };
    let want = normalize(&dir.to_string_lossy());
    if current.split(';').any(|p| normalize(p) == want) {
        return Ok(false);
    }
    let updated = if current.trim().is_empty() {
        dir.to_string_lossy().into_owned()
    } else {
        format!("{};{}", current.trim_end_matches(';'), dir.display())
    };
    // REG_EXPAND_SZ is UTF-16LE, NUL-terminated.
    let bytes = updated
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    env.set_raw_value(
        "Path",
        &RegValue {
            bytes,
            vtype: REG_EXPAND_SZ,
        },
    )
    .map_err(|e| e.to_string())?;
    broadcast_environment_change();
    Ok(true)
}

/// Send WM_SETTINGCHANGE("Environment") to every top-level window, as System
/// Properties does after an environment change. Best effort.
#[cfg(windows)]
fn broadcast_environment_change() {
    #[link(name = "user32")]
    extern "system" {
        fn SendMessageTimeoutW(
            hwnd: isize,
            msg: u32,
            wparam: usize,
            lparam: isize,
            flags: u32,
            timeout: u32,
            result: *mut usize,
        ) -> isize;
    }
    const HWND_BROADCAST: isize = 0xffff;
    const WM_SETTINGCHANGE: u32 = 0x001A;
    const SMTO_ABORTIFHUNG: u32 = 0x0002;
    let area: Vec<u16> = "Environment".encode_utf16().chain([0]).collect();
    let mut result = 0usize;
    // SAFETY: `area` is a NUL-terminated UTF-16 string that outlives the call,
    // and `result` points to a valid usize for the reply.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            area.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            2000,
            &mut result,
        );
    }
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
                // After the daemon is settled, so an older copy isn't in use.
                #[cfg(windows)]
                refresh_cli_copies();
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
                            let _ = background_command(cli).arg("open").spawn();
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
            // Stop the daemon only if this app started it. (`kill` is
            // TerminateProcess on Windows, so this covers Windows too.)
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
