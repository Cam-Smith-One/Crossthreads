//! The security boundaries, end to end: the local key, the peer-only network
//! listener, tokens for peers, signing the web app in, response headers, and
//! size limits.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use ct_daemon::{Daemon, Federation, Request, Response};
use ct_embed::HashEmbedder;
use ct_store::Store;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn daemon() -> Daemon {
    Daemon::new(
        Store::open_in_memory().unwrap(),
        Box::new(HashEmbedder::default()),
    )
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Send raw protocol lines on one connection; return each reply line.
fn lines(addr: &str, requests: &[&str]) -> Vec<String> {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut w = s.try_clone().unwrap();
    let mut r = BufReader::new(s);
    requests
        .iter()
        .map(|req| {
            w.write_all(format!("{req}\n").as_bytes()).unwrap();
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            line
        })
        .collect()
}

fn serve(d: Daemon, peers_only: bool) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    thread::spawn(move || {
        let _ = if peers_only {
            d.serve_peers(l)
        } else {
            d.serve(l)
        };
    });
    addr
}

#[test]
fn local_clients_must_present_the_local_key() {
    let addr = serve(daemon().with_local_key(KEY.into()), false);
    let hello = format!(r#"{{"op":"hello","key":"{KEY}"}}"#);
    let replies = lines(
        &addr,
        &[
            r#"{"op":"ping"}"#,
            r#"{"op":"status"}"#,
            r#"{"op":"hello","key":"wrong"}"#,
            r#"{"op":"status"}"#,
            &hello,
            r#"{"op":"status"}"#,
        ],
    );
    assert!(replies[0].contains("pong"), "ping is open: {}", replies[0]);
    assert!(replies[1].contains("unauthorized"), "{}", replies[1]);
    assert!(replies[2].contains("wrong local key"), "{}", replies[2]);
    assert!(replies[3].contains("unauthorized"), "{}", replies[3]);
    assert!(replies[4].contains(r#""ok":true"#), "{}", replies[4]);
    assert!(replies[5].contains("conversations"), "{}", replies[5]);

    // The client presents the key on its own when it has it.
    std::env::set_var("CROSSTHREADS_LOCAL_KEY", KEY);
    let status = ct_daemon::Client::new(addr.clone()).call(&Request::Status);
    std::env::remove_var("CROSSTHREADS_LOCAL_KEY");
    assert!(matches!(status, Ok(Response::Status { .. })), "{status:?}");
}

#[test]
fn the_network_listener_serves_only_the_peer_protocol() {
    let addr = serve(daemon(), true);
    let replies = lines(
        &addr,
        &[
            r#"{"op":"ping"}"#,
            r#"{"op":"status"}"#,
            r#"{"op":"phone_pair"}"#,
            r#"{"op":"set_federation_config","token":"x"}"#,
        ],
    );
    assert!(replies[0].contains("pong"));
    for r in &replies[1..] {
        assert!(r.contains("only the peer protocol"), "{r}");
    }
}

#[test]
fn a_device_without_a_token_serves_no_peers() {
    let d = daemon().with_federation(Federation::new(
        "mac".into(),
        None,
        vec![],
        Duration::from_millis(500),
    ));
    match d.handle(Request::PeerThreads {
        token: None,
        filters: Default::default(),
    }) {
        Response::Error { message } => assert!(message.contains("no shared token"), "{message}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn weak_tokens_are_refused() {
    let d = daemon().with_federation(Federation::new(
        "mac".into(),
        None,
        vec![],
        Duration::from_millis(500),
    ));
    let set = |t: &str| {
        d.handle(Request::SetFederationConfig {
            device: None,
            token: Some(t.into()),
            exclude_tools: None,
            exclude_projects: None,
        })
    };
    assert!(matches!(set("hunter2"), Response::Error { .. }));
    assert!(matches!(
        set("aaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        Response::Error { .. }
    ));
}

/// One HTTP request; returns (status, head, body).
fn http(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    let status = head.split(' ').nth(1).unwrap_or("0").parse().unwrap_or(0);
    (status, head.to_string(), body.to_string())
}

#[test]
fn the_web_app_signs_in_with_a_one_time_ticket() {
    let http_addr = format!("127.0.0.1:{}", free_port());
    let d = daemon()
        .with_local_key(KEY.into())
        .with_http_addr(Some(http_addr.clone()));
    let proto = serve(d.clone(), false);
    {
        let d = d.clone();
        let a = http_addr.clone();
        thread::spawn(move || d.serve_http(&a, None));
    }
    thread::sleep(Duration::from_millis(200));
    let json = ("Content-Type", "application/json");

    // Not signed in: nothing, and every response carries the security headers.
    let (status, head, body) = http(
        &http_addr,
        "POST",
        "/api/rpc",
        &[json],
        r#"{"op":"status"}"#,
    );
    assert_eq!(status, 401, "{body}");
    assert!(body.contains(r#""login":true"#));
    for h in [
        "Content-Security-Policy",
        "X-Frame-Options: DENY",
        "X-Content-Type-Options: nosniff",
        "Referrer-Policy: no-referrer",
    ] {
        assert!(head.contains(h), "missing {h} in {head}");
    }

    // A ticket comes only from a client holding the key.
    let hello = format!(r#"{{"op":"hello","key":"{KEY}"}}"#);
    let replies = lines(
        &proto,
        &[
            r#"{"op":"login_ticket"}"#,
            &hello,
            r#"{"op":"login_ticket"}"#,
        ],
    );
    assert!(replies[0].contains("unauthorized"));
    let v: serde_json::Value = serde_json::from_str(&replies[2]).unwrap();
    let ticket = v["ticket"].as_str().unwrap().to_string();
    assert!(v["url"]
        .as_str()
        .unwrap()
        .ends_with(&format!("/login#{ticket}")));
    // …and never from the web bridge.
    assert!(matches!(
        d.handle(Request::LoginTicket),
        Response::Error { .. }
    ));

    let login = |t: &str| {
        http(
            &http_addr,
            "POST",
            "/api/login",
            &[json],
            &format!(r#"{{"ticket":"{t}"}}"#),
        )
    };
    assert_eq!(login("made-up").0, 403);
    let (status, head, _) = login(&ticket);
    assert_eq!(status, 200);
    let cookie_line = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("set-cookie"))
        .unwrap()
        .to_string();
    assert!(cookie_line.contains("HttpOnly") && cookie_line.contains("SameSite=Strict"));
    assert!(
        !cookie_line.contains(KEY),
        "the key itself never reaches the browser"
    );
    let session = cookie_line
        .split("ct_local=")
        .nth(1)
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    assert_eq!(login(&ticket).0, 403, "a ticket works once");

    let cookie = format!("ct_local={session}");
    let (status, _, body) = http(
        &http_addr,
        "POST",
        "/api/rpc",
        &[json, ("Cookie", &cookie)],
        r#"{"op":"status"}"#,
    );
    assert_eq!(status, 200, "{body}");
    let (status, _, _) = http(
        &http_addr,
        "POST",
        "/api/rpc",
        &[json, ("Cookie", "ct_local=forged")],
        r#"{"op":"status"}"#,
    );
    assert_eq!(status, 401);

    // An oversized body is refused before it's read.
    let big = "x".repeat(3 * 1024 * 1024);
    let (status, _, _) = http(
        &http_addr,
        "POST",
        "/api/rpc",
        &[json, ("Cookie", &cookie)],
        &big,
    );
    assert_eq!(status, 400);
}

#[test]
fn the_web_app_never_listens_beyond_this_machine() {
    let err = daemon().serve_http("0.0.0.0:0", None).unwrap_err();
    assert!(format!("{err}").contains("loopback"), "{err}");
}

#[test]
fn room_messages_have_a_size_limit() {
    let d = daemon();
    let huge = "x".repeat(ct_daemon::rooms::MAX_TEXT + 1);
    match d.handle(Request::RoomSend {
        me: "claude".into(),
        text: huge.clone(),
        room: "demo".into(),
        to: None,
        wait: false,
        timeout: 0,
    }) {
        Response::Text { text } => assert!(text.contains("too long"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        d.handle(Request::RoomPost {
            room: "demo".into(),
            text: huge,
            to: None,
            author: None
        }),
        Response::Error { .. }
    ));
}

#[test]
fn an_endless_request_line_is_cut_off() {
    let addr = serve(daemon(), false);
    let mut s = TcpStream::connect(&addr).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
    let chunk = vec![b'x'; 1024 * 1024];
    // Writing well past the 8 MB limit must fail once the daemon hangs up.
    let mut failed = false;
    for _ in 0..64 {
        if s.write_all(&chunk).is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed, "the daemon kept reading an endless line");
}

#[test]
fn reports_its_version() {
    match daemon().handle(Request::Version) {
        Response::Version { version } => assert_eq!(version, env!("CARGO_PKG_VERSION")),
        other => panic!("{other:?}"),
    }
}
