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
        let bind_host = host_of(addr).to_string();
        for mut request in server.incoming_requests() {
            let response = self.route(&mut request, ui_dir.as_deref(), &bind_host);
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
        bind_host: &str,
    ) -> Response<std::io::Cursor<Vec<u8>>> {
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("/");

        match (request.method(), path) {
            (Method::Post, "/api/rpc") => {
                if !from_our_own_page(request, bind_host) {
                    return json_response(
                        403,
                        &serde_json::json!({ "type": "error", "message": "forbidden" }),
                    );
                }
                self.handle_rpc(request)
            }
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
        json_response(200, &self.handle(no_blocking(req)))
    }
}

/// This server answers one request at a time, so a call that parks (an agent
/// waiting in a room) would freeze the whole UI. Agents use the socket
/// protocol for those; over HTTP they return immediately.
fn no_blocking(req: Request) -> Request {
    match req {
        Request::RoomSend {
            me, text, room, to, ..
        } => Request::RoomSend {
            me,
            text,
            room,
            to,
            wait: false,
            timeout: 0,
        },
        Request::RoomWait { me, room, .. } => Request::RoomWait {
            me,
            room,
            timeout: 0,
        },
        Request::RoomStop {
            me,
            room,
            transcript_path,
            ..
        } => Request::RoomStop {
            me,
            room,
            wait: 0,
            transcript_path,
        },
        other => other,
    }
}

/// The host part of a `host:port` address (IPv6 brackets kept).
fn host_of(addr: &str) -> &str {
    match addr.rfind(':') {
        Some(i) if !addr.ends_with(']') => &addr[..i],
        _ => addr,
    }
}

fn header<'a>(request: &'a tiny_http::Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

/// Is this RPC call from the page this daemon serves (or a local script), and
/// not from some other website open in the user's browser?
///
/// It matters because the API can post a "human" message that agents obey.
/// Three checks, all cheap:
///
/// - **JSON only.** A cross-site form or a "simple" fetch can't send
///   `application/json` without a preflight, which we never approve.
/// - **Same origin.** A browser attaches `Origin` to every cross-site POST; if
///   present it must be this server.
/// - **A host we answer to.** The `Host` must be loopback or the address we
///   bound, so a hostile name re-pointed at 127.0.0.1 (DNS rebinding) is
///   refused even though its origin "matches".
fn from_our_own_page(request: &tiny_http::Request, bind_host: &str) -> bool {
    let json = header(request, "Content-Type")
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
    let Some(host) = header(request, "Host") else {
        return false;
    };
    let name = host_of(host);
    let known = matches!(name, "127.0.0.1" | "localhost" | "[::1]") || name == bind_host;
    let same_origin = header(request, "Origin").map_or(true, |origin| {
        origin
            .split_once("://")
            .is_some_and(|(_, rest)| rest.eq_ignore_ascii_case(host))
    });
    json && known && same_origin
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_host_from_port() {
        assert_eq!(host_of("127.0.0.1:47101"), "127.0.0.1");
        assert_eq!(host_of("localhost:47101"), "localhost");
        assert_eq!(host_of("[::1]:47101"), "[::1]");
        assert_eq!(host_of("mac-mini"), "mac-mini");
    }

    #[test]
    fn calls_that_would_park_return_at_once_over_http() {
        let req = no_blocking(Request::RoomSend {
            me: "a".into(),
            text: "t".into(),
            room: "r".into(),
            to: None,
            wait: true,
            timeout: 50,
        });
        assert!(matches!(
            req,
            Request::RoomSend {
                wait: false,
                timeout: 0,
                ..
            }
        ));
        let req = no_blocking(Request::RoomWait {
            me: "a".into(),
            room: "r".into(),
            timeout: 600,
        });
        assert!(matches!(req, Request::RoomWait { timeout: 0, .. }));
        let req = no_blocking(Request::RoomStop {
            me: "a".into(),
            room: "r".into(),
            wait: 240,
            transcript_path: None,
        });
        assert!(matches!(req, Request::RoomStop { wait: 0, .. }));
        assert!(matches!(no_blocking(Request::Ping), Request::Ping));
    }
}
