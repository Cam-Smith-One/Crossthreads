//! Crossthreads native desktop shell (Tauri v2).
//!
//! The window loads the same `ui/` frontend as the web build. Instead of HTTP,
//! the frontend's `rpc()` calls the `rpc` Tauri command below, which forwards
//! to a running `crossthreadsd` over the local protocol — so the desktop app
//! shares the one index with the CLI and MCP server (ADR-005).

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![rpc])
        .run(tauri::generate_context!())
        .expect("error while running Crossthreads desktop");
}
