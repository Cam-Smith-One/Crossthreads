//! Optional HTTP bridge for the desktop/web UI.
//!
//! The native Tauri shell talks to the daemon through this same JSON API (and a
//! browser can too): `POST /api/rpc` with a [`Request`] body returns a
//! [`Response`]; everything else serves the built UI's static files. Kept on a
//! separate port from the raw TCP protocol so the two surfaces don't interfere.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tiny_http::{Header, Method, Response, Server};

use crate::{Daemon, Request};

impl Daemon {
    /// Serve the HTTP API (+ optional static UI dir) on `addr` until stopped.
    /// Blocking; run on its own thread.
    pub fn serve_http(&self, addr: &str, ui_dir: Option<PathBuf>) -> Result<()> {
        // The local web app is for this computer only; phones have their own
        // listener (see `phone`).
        if !crate::local_auth::is_loopback(addr) {
            anyhow::bail!(
                "--http must be a loopback address such as 127.0.0.1:47101; to reach the board \
                 from a phone, use phone access"
            );
        }
        let server = Server::http(addr).map_err(|e| anyhow::anyhow!("binding http {addr}: {e}"))?;
        let bind_host = host_of(addr).to_string();
        for mut request in server.incoming_requests() {
            let response = secure(self.route(&mut request, ui_dir.as_deref(), &bind_host));
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
            (Method::Post, "/api/login") => {
                if !from_our_own_page(request, bind_host) {
                    return error_json(403, "forbidden");
                }
                let Some(key) = self.local_key.clone() else {
                    return json_response(200, &serde_json::json!({ "ok": true }));
                };
                let ticket = read_body(request)
                    .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
                    .and_then(|v| v.get("ticket").and_then(|t| t.as_str()).map(String::from));
                match ticket {
                    Some(t) if self.login_tickets.redeem(&t) => {
                        let cookie = format!(
                            "{}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000",
                            crate::local_auth::COOKIE,
                            crate::local_auth::http_session(&key)
                        );
                        let mut resp = json_response(200, &serde_json::json!({ "ok": true }));
                        if let Ok(h) = Header::from_bytes(b"Set-Cookie".as_ref(), cookie.as_bytes()) {
                            resp = resp.with_header(h);
                        }
                        resp
                    }
                    _ => error_json(
                        403,
                        "that sign-in link has expired or was already used; run `crossthreads open`",
                    ),
                }
            }
            (Method::Post, "/api/rpc") => {
                if !from_our_own_page(request, bind_host) {
                    return error_json(403, "forbidden");
                }
                if let Some(key) = &self.local_key {
                    let expected = crate::local_auth::http_session(key);
                    let ok = header(request, "Cookie")
                        .and_then(|c| crate::phone::cookie(c, crate::local_auth::COOKIE))
                        .is_some_and(|v| crate::local_auth::same(v, &expected));
                    if !ok {
                        return json_response(
                            401,
                            &serde_json::json!({
                                "type": "error",
                                "message": "sign in first: run `crossthreads open` on this computer",
                                "login": true,
                            }),
                        );
                    }
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
        let Some(body) = read_body(request) else {
            return error_json(400, "unreadable or oversized body");
        };
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

// ---- the phone listener (SPEC FR-BRD-04) -------------------------------------

#[derive(serde::Deserialize)]
struct PairBody {
    code: String,
    #[serde(default)]
    name: String,
}

impl Daemon {
    /// Where the phone listener should bind: `CROSSTHREADS_PHONE_ADDR`, the
    /// address saved in `phone.json`, or this device's Tailscale address.
    fn phone_addr(&self) -> std::result::Result<String, String> {
        let addr = std::env::var("CROSSTHREADS_PHONE_ADDR")
            .ok()
            .filter(|a| !a.trim().is_empty())
            .or_else(|| self.phone.saved_addr())
            .or_else(|| {
                crate::federation::detect_self_tailnet_ip()
                    .map(|ip| format!("{ip}:{}", crate::phone::PHONE_PORT))
            })
            .ok_or_else(|| {
                "Tailscale isn't connected on this device. Phone access only listens on your \
                 tailnet; install Tailscale on this computer and your phone, sign both in, \
                 then try again."
                    .to_string()
            })?;
        crate::phone::safe_bind(&addr)?;
        Ok(addr)
    }

    /// Start the phone listener if phone access is on and it isn't running.
    /// A failure is kept for Settings to show, not returned.
    pub fn start_phone_if_enabled(&self) {
        if !self.phone.enabled() || self.phone_server.lock().expect("poisoned").is_some() {
            return;
        }
        let result = self.phone_addr().and_then(|addr| {
            tiny_http::Server::http(&addr)
                .map(|server| (Arc::new(server), addr.clone()))
                .map_err(|e| format!("couldn't listen on {addr}: {e}"))
        });
        match result {
            Ok((server, addr)) => {
                eprintln!("crossthreadsd: phone access on http://{addr}/");
                *self.phone_error.lock().expect("poisoned") = None;
                *self.phone_server.lock().expect("poisoned") = Some((server.clone(), addr.clone()));
                let this = self.clone();
                let _ = std::thread::Builder::new()
                    .name("ct-phone".into())
                    .spawn(move || {
                        for mut request in server.incoming_requests() {
                            let response = secure(this.phone_route(&mut request, &addr));
                            if let Err(e) = request.respond(response) {
                                eprintln!("phone respond error: {e}");
                            }
                        }
                    });
            }
            Err(e) => {
                eprintln!("crossthreadsd: phone access not started: {e}");
                *self.phone_error.lock().expect("poisoned") = Some(e);
            }
        }
    }

    fn stop_phone(&self) {
        if let Some((server, _)) = self.phone_server.lock().expect("poisoned").take() {
            server.unblock();
        }
    }

    pub(crate) fn phone_status(&self) -> crate::Response {
        let running = self.phone_server.lock().expect("poisoned").clone();
        crate::Response::Phone {
            enabled: self.phone.enabled(),
            listening: running.is_some(),
            url: running.map(|(_, addr)| format!("http://{addr}/")),
            error: self.phone_error.lock().expect("poisoned").clone(),
            phones: self.phone.phones(),
        }
    }

    pub(crate) fn phone_set_enabled(&self, on: bool) -> crate::Response {
        self.phone.set_enabled(on);
        if on {
            self.start_phone_if_enabled();
        } else {
            self.stop_phone();
            *self.phone_error.lock().expect("poisoned") = None;
        }
        self.phone_status()
    }

    pub(crate) fn phone_pair(&self) -> crate::Response {
        let addr = self
            .phone_server
            .lock()
            .expect("poisoned")
            .clone()
            .map(|(_, a)| a);
        let Some(addr) = addr else {
            return crate::Response::Error {
                message: "turn phone access on first".into(),
            };
        };
        let code = self.phone.start_pairing();
        let url = format!("http://{addr}/pair#{code}");
        crate::Response::PhonePairing {
            qr_svg: crate::phone::qr_svg(&url),
            code,
            url,
            expires_secs: crate::phone::CODE_TTL.as_secs(),
        }
    }

    fn phone_route(
        &self,
        request: &mut tiny_http::Request,
        bind: &str,
    ) -> Response<std::io::Cursor<Vec<u8>>> {
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("/").to_string();
        let post = matches!(request.method(), Method::Post);
        match (post, path.as_str()) {
            (true, "/api/pair") => {
                if !from_phone_page(request) {
                    return error_json(403, "forbidden");
                }
                let Some(body) = read_body(request) else {
                    return error_json(400, "unreadable body");
                };
                let Ok(body) = serde_json::from_str::<PairBody>(&body) else {
                    return error_json(400, "send {\"code\": …, \"name\": …}");
                };
                match self.phone.redeem(&body.code, &body.name) {
                    Some(token) => {
                        let cookie = format!(
                            "{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000",
                            crate::phone::COOKIE
                        );
                        let mut resp = json_response(200, &serde_json::json!({ "ok": true }));
                        if let Ok(h) = Header::from_bytes(b"Set-Cookie".as_ref(), cookie.as_bytes())
                        {
                            resp = resp.with_header(h);
                        }
                        resp
                    }
                    None => error_json(
                        403,
                        "that code didn't work: it may have expired or been used. Make a new one \
                         on the desktop (Settings → Phone).",
                    ),
                }
            }
            (true, "/api/rpc") => {
                if !from_phone_page(request) {
                    return error_json(403, "forbidden");
                }
                let phone = header(request, "Cookie")
                    .and_then(|c| crate::phone::cookie(c, crate::phone::COOKIE))
                    .and_then(|t| self.phone.check(t));
                let Some(phone) = phone else {
                    return json_response(
                        401,
                        &serde_json::json!({
                            "type": "error",
                            "message": "pair this phone first",
                            "unpaired": true,
                        }),
                    );
                };
                let Some(body) = read_body(request) else {
                    return error_json(400, "unreadable body");
                };
                let req: Request = match serde_json::from_str(&body) {
                    Ok(r) => r,
                    Err(e) => return error_json(400, &format!("bad request: {e}")),
                };
                if let Err(why) = crate::phone::allowed(&req, &phone) {
                    return error_json(403, why);
                }
                let req = match req {
                    Request::Session => {
                        return json_response(
                            200,
                            &crate::Response::Session {
                                phone: true,
                                name: Some(phone.name),
                                can_post: phone.can_post,
                            },
                        )
                    }
                    // A phone's message is signed with the phone's name.
                    Request::RoomPost { room, text, to, .. } => Request::RoomPost {
                        room,
                        text,
                        to,
                        author: Some(phone.name.clone()),
                    },
                    other => other,
                };
                json_response(200, &self.handle(no_blocking(req)))
            }
            (false, _) if matches!(request.method(), Method::Get) => {
                let _ = bind;
                serve_static(&path, self.ui_dir.as_deref())
            }
            _ => text_response(405, "method not allowed"),
        }
    }
}

/// Largest request body accepted (requests are small; this is generous).
const MAX_BODY: u64 = 2 * 1024 * 1024;

fn read_body(request: &mut tiny_http::Request) -> Option<String> {
    use std::io::Read;
    if request.body_length().is_some_and(|n| n as u64 > MAX_BODY) {
        return None;
    }
    let mut body = String::new();
    request
        .as_reader()
        .take(MAX_BODY + 1)
        .read_to_string(&mut body)
        .ok()?;
    (body.len() as u64 <= MAX_BODY).then_some(body)
}

/// Headers every response carries: no framing (clickjacking), no MIME
/// sniffing, no referrer, nothing cached, and a content policy that allows
/// only this app's own scripts, styles, images and requests.
fn secure(mut resp: Response<std::io::Cursor<Vec<u8>>>) -> Response<std::io::Cursor<Vec<u8>>> {
    for (k, v) in [
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
        ("Referrer-Policy", "no-referrer"),
        ("Cross-Origin-Opener-Policy", "same-origin"),
        ("Cross-Origin-Resource-Policy", "same-origin"),
        (
            "Permissions-Policy",
            "camera=(), microphone=(), geolocation=()",
        ),
        ("Cache-Control", "no-store"),
        (
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; \
             form-action 'self'; frame-ancestors 'none'",
        ),
    ] {
        if let Ok(h) = Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp = resp.with_header(h);
        }
    }
    resp
}

fn error_json(status: u16, message: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    json_response(
        status,
        &serde_json::json!({ "type": "error", "message": message }),
    )
}

/// The phone listener's version of [`from_our_own_page`]. A phone reaches us
/// by tailnet IP or MagicDNS name, so the `Host` isn't pinned; what stops
/// another site is that it can't send JSON without a preflight, its `Origin`
/// won't match, and the phone's cookie is `SameSite=Strict` and bound to this
/// host, so a re-pointed name never carries it.
fn from_phone_page(request: &tiny_http::Request) -> bool {
    let json = header(request, "Content-Type")
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
    let Some(host) = header(request, "Host") else {
        return false;
    };
    // Browsers send Origin on every POST from a page, so one is required here:
    // a request without it isn't coming from the phone's browser.
    let same_origin = header(request, "Origin").is_some_and(|origin| {
        origin
            .split_once("://")
            .is_some_and(|(_, rest)| rest.eq_ignore_ascii_case(host))
    });
    json && same_origin
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
