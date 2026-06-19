//! Optional HTTP bridge for the desktop/web UI.
//!
//! The native Tauri shell talks to the daemon through this same JSON API (and a
//! browser can too): `POST /api/rpc` with a [`Request`] body returns a
//! [`Response`]; everything else serves the built UI's static files. Kept on a
//! separate port from the raw TCP protocol so the two surfaces don't interfere.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tiny_http::{Header, Method, Response, Server};

use crate::{Daemon, Request};

impl Daemon {
    /// Serve the HTTP API (+ optional static UI dir) on `addr` until stopped.
    /// Blocking; run on its own thread.
    pub fn serve_http(&self, addr: &str, ui_dir: Option<PathBuf>) -> Result<()> {
        let server = Server::http(addr).map_err(|e| anyhow::anyhow!("binding http {addr}: {e}"))?;
        for mut request in server.incoming_requests() {
            let response = self.route(&mut request, ui_dir.as_deref());
            if let Err(e) = request.respond(response) {
                eprintln!("http respond error: {e}");
            }
        }
        Ok(())
    }

    fn route(
        &self,
        request: &mut tiny_http::Request,
        ui_dir: Option<&Path>,
    ) -> Response<std::io::Cursor<Vec<u8>>> {
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("/");

        match (request.method(), path) {
            (Method::Post, "/api/rpc") => self.handle_rpc(request),
            (Method::Get, "/api/rpc") => json_response(
                400,
                &serde_json::json!({ "type": "error", "message": "POST a Request body" }),
            ),
            (Method::Get, _) => serve_static(path, ui_dir),
            _ => text_response(405, "method not allowed"),
        }
    }

    fn handle_rpc(&self, request: &mut tiny_http::Request) -> Response<std::io::Cursor<Vec<u8>>> {
        let mut body = String::new();
        if request.as_reader().read_to_string(&mut body).is_err() {
            return json_response(
                400,
                &serde_json::json!({ "type": "error", "message": "unreadable body" }),
            );
        }
        let req: Request = match serde_json::from_str(&body) {
            Ok(r) => r,
            Err(e) => {
                return json_response(
                    400,
                    &serde_json::json!({ "type": "error", "message": format!("bad request: {e}") }),
                )
            }
        };
        json_response(200, &self.handle(req))
    }
}

fn serve_static(path: &str, ui_dir: Option<&Path>) -> Response<std::io::Cursor<Vec<u8>>> {
    let Some(dir) = ui_dir else {
        return text_response(404, "no UI bundled; run with --ui <dir>");
    };
    // Resolve, guarding against path traversal; SPA fallback to index.html.
    let rel = path.trim_start_matches('/');
    let candidate = dir.join(rel);
    let file = if rel.is_empty() || !safe_under(dir, &candidate) || !candidate.is_file() {
        dir.join("index.html")
    } else {
        candidate
    };
    match std::fs::read(&file) {
        Ok(bytes) => {
            let mut resp = Response::from_data(bytes).with_status_code(200);
            if let Ok(h) =
                Header::from_bytes(b"Content-Type".as_ref(), content_type(&file).as_bytes())
            {
                resp = resp.with_header(h);
            }
            resp
        }
        Err(_) => text_response(404, "not found"),
    }
}

/// True if `candidate` stays within `dir` (no `..` escape).
fn safe_under(dir: &Path, candidate: &Path) -> bool {
    match (dir.canonicalize(), candidate.canonicalize()) {
        (Ok(d), Ok(c)) => c.starts_with(d),
        // Candidate may not exist yet; fall back to a lexical check.
        _ => !candidate.components().any(|c| c.as_os_str() == ".."),
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn json_response<T: serde::Serialize>(
    status: u16,
    value: &T,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let body = serde_json::to_vec(value).unwrap_or_default();
    let mut resp = Response::from_data(body).with_status_code(status);
    if let Ok(h) = Header::from_bytes(b"Content-Type".as_ref(), b"application/json".as_ref()) {
        resp = resp.with_header(h);
    }
    resp
}

fn text_response(status: u16, msg: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(msg).with_status_code(status)
}
