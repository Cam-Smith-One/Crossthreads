//! Crossthreads native desktop shell (Tauri v2) — a **menu-bar / tray app**.
//!
//! The tray icon is the primary surface: left-click toggles a compact **glance**
//! popover (`index.html?view=glance`) that fuses per-tool usage with the
//! behavioral insight read; the tray menu opens the full window on demand. Both
//! windows load the same `ui/` frontend and talk to a running `crossthreadsd`
//! via the `rpc` command below, so the desktop app shares the one index with the
//! CLI and MCP server (ADR-005).
//!
//! Note: this crate is excluded from the headless workspace (it needs the
//! platform webview libs), so the tray wiring is validated on a real desktop
//! with `cargo tauri build`, not in CI.

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

use ct_daemon::{Client, Request};

/// Forward a JSON-encoded daemon `Request` and return the JSON `Response`.
/// The frontend speaks the exact same protocol it would over HTTP.
#[tauri::command]
fn rpc(request: String) -> Result<String, String> {
    let req: Request = serde_json::from_str(&request).map_err(|e| format!("bad request: {e}"))?;
    let client = Client::from_env();
    let response = client.call(&req).map_err(|e| format!("{e:#}"))?;
    serde_json::to_string(&response).map_err(|e| e.to_string())
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
    // Anchor the popover roughly under the tray click. Precise per-OS placement
    // (menu bar at top on macOS, tray at bottom on Windows) is polished
    // on-device; this keeps it near the cursor and on-screen.
    if let Some((x, y)) = near {
        let px = (x - 180.0).max(0.0);
        let _ = w.set_position(tauri::PhysicalPosition::new(px, y));
    }
    let _ = w.show();
    let _ = w.set_focus();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![rpc, open_main])
        .on_window_event(|window, event| match event {
            // Closing the main window hides it to the tray instead of quitting —
            // standard menu-bar app behavior (quit is on the tray menu).
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
            // Menu-bar–only presence on macOS (no dock icon), like CodexBar.
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle();
            let open = MenuItem::with_id(handle, "open", "Open Crossthreads", true, None::<&str>)?;
            let quit = MenuItem::with_id(handle, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(handle, &[&open, &quit])?;

            TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().cloned().unwrap())
                .tooltip("Crossthreads — usage & insight")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main(app),
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

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Crossthreads desktop");
}
