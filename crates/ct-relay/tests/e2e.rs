//! End-to-end: real relay on loopback, real devices.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use ct_relay::{Config, Relay, RelayHandle};
use ct_tunnel::wire::{
    self, code, Hello, Op, Reply, PROTOCOL_VERSION, REGISTER_NOISE, REGISTER_PROLOGUE, TUNNEL_NOISE,
};
use ct_tunnel::{
    connect, generate_code, key_to_hex, listen, pair_host, pair_join, Error, Identity, Listener,
};

fn relay() -> (RelayHandle, String) {
    let r = Relay::bind(
        "127.0.0.1:0",
        Config {
            log: false,
            ..Config::default()
        },
    )
    .unwrap()
    .spawn()
    .unwrap();
    let addr = r.addr().to_string();
    (r, addr)
}

fn id() -> Identity {
    Identity::generate().unwrap()
}

/// Join, retrying while the host hasn't opened the slot yet.
fn join_when_open(
    addr: &str,
    me: &Identity,
    code: &str,
    payload: &[u8],
) -> ct_tunnel::Result<ct_tunnel::Paired> {
    for _ in 0..100 {
        match pair_join(addr, me, code, payload) {
            Err(Error::CodeUnavailable(m)) if m.contains("no device is waiting") => {
                thread::sleep(Duration::from_millis(20))
            }
            other => return other,
        }
    }
    panic!("host never opened the code");
}

#[test]
fn pairing_with_a_code_swaps_keys_and_payloads() {
    let (_relay, addr) = relay();
    let (a, b) = (id(), id());
    let code = generate_code();
    let host = {
        let (addr, a, code) = (addr.clone(), a.clone(), code.clone());
        thread::spawn(move || pair_host(&addr, &a, &code, b"laptop|token-a"))
    };
    let joined = join_when_open(&addr, &b, &code.to_lowercase(), b"desktop|token-b").unwrap();
    let hosted = host.join().unwrap().unwrap();
    assert_eq!(joined.peer_public, a.public());
    assert_eq!(joined.peer_payload, b"laptop|token-a");
    assert_eq!(hosted.peer_public, b.public());
    assert_eq!(hosted.peer_payload, b"desktop|token-b");
}

#[test]
fn wrong_code_fails_both_sides_and_burns_the_slot() {
    let (_relay, addr) = relay();
    let (a, b) = (id(), id());
    let code = generate_code();
    // Same slot (first four characters), different secret part.
    let mut wrong: Vec<char> = code.chars().collect();
    let last = wrong.len() - 1;
    wrong[last] = if wrong[last] == 'A' { 'B' } else { 'A' };
    let wrong: String = wrong.into_iter().collect();
    let host = {
        let (addr, a, code) = (addr.clone(), a.clone(), code.clone());
        thread::spawn(move || pair_host(&addr, &a, &code, b"secret-host-payload"))
    };
    let joined = join_when_open(&addr, &b, &wrong, b"secret-join-payload");
    assert!(matches!(joined, Err(Error::WrongCode)), "{joined:?}");
    let hosted = host.join().unwrap();
    assert!(matches!(hosted, Err(Error::WrongCode)), "{hosted:?}");
    // The right code can't be tried again on that rendezvous.
    let retry = pair_join(&addr, &b, &code, b"x");
    assert!(matches!(retry, Err(Error::CodeUnavailable(_))), "{retry:?}");
}

/// Listen as `me`, allowing only `allowed`; the handler echoes messages and
/// then sends one of its own.
fn echo_listener(
    addr: &str,
    me: &Identity,
    allowed: [u8; 32],
    calls: Arc<AtomicUsize>,
) -> Listener {
    let l = listen(
        addr,
        me,
        move |k| *k == allowed,
        move |mut s, peer| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(peer, allowed);
            while let Ok(Some(msg)) = s.recv_message() {
                let mut reply = msg.clone();
                reply.reverse();
                if s.send_message(&reply).is_err() {
                    break;
                }
            }
        },
    );
    assert!(
        l.wait_registered(Duration::from_secs(10)),
        "{:?}",
        l.last_error()
    );
    l
}

#[test]
fn connect_and_listen_echo_large_messages_both_ways() {
    let (_relay, addr) = relay();
    let (a, b) = (id(), id());
    let calls = Arc::new(AtomicUsize::new(0));
    let _l = echo_listener(&addr, &b, a.public(), calls.clone());
    let mut s = connect(&addr, &a, &b.public()).unwrap();
    assert_eq!(s.peer_public(), b.public());
    for size in [250_000usize, 1, 70_000] {
        let msg: Vec<u8> = (0..size).map(|i| (i * 7 % 256) as u8).collect();
        s.send_message(&msg).unwrap();
        let mut expect = msg.clone();
        expect.reverse();
        assert_eq!(s.recv_message().unwrap().unwrap(), expect);
    }
    // The line protocol shape the daemon uses: write a line, read a line.
    let mut raw = connect(&addr, &a, &b.public()).unwrap();
    raw.send_message(b"second connection").unwrap();
    assert_eq!(raw.recv_message().unwrap().unwrap(), b"noitcennoc dnoces");
    drop(raw);
    drop(s);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn byte_stream_over_relay() {
    let (_relay, addr) = relay();
    let (a, b) = (id(), id());
    let ap = a.public();
    let _l = listen(
        &addr,
        &b,
        move |k| *k == ap,
        |s, _| {
            // Line echo, like the daemon's request loop.
            let mut w = s.try_clone().unwrap();
            let mut r = std::io::BufReader::new(s);
            let mut line = String::new();
            while std::io::BufRead::read_line(&mut r, &mut line).unwrap_or(0) > 0 {
                w.write_all(line.as_bytes()).unwrap();
                line.clear();
            }
        },
    );
    assert!(_l.wait_registered(Duration::from_secs(10)));
    let s = connect(&addr, &a, &b.public()).unwrap();
    let mut w = s.try_clone().unwrap();
    let mut r = std::io::BufReader::new(s);
    let big = "x".repeat(300_000);
    let writer = thread::spawn(move || {
        w.write_all(format!("{big}\nsmall\n").as_bytes()).unwrap();
        w.shutdown().unwrap();
    });
    let mut out = String::new();
    r.read_to_string(&mut out).unwrap();
    writer.join().unwrap();
    assert_eq!(out.len(), 300_000 + 1 + 6);
    assert!(out.ends_with("\nsmall\n"));
}

#[test]
fn unknown_device_is_declined() {
    let (_relay, addr) = relay();
    let (a, b, c) = (id(), id(), id());
    let calls = Arc::new(AtomicUsize::new(0));
    let _l = echo_listener(&addr, &b, a.public(), calls.clone());
    let r = connect(&addr, &c, &b.public());
    assert!(matches!(r, Err(Error::PeerRefused)), "{r:?}");
    // Nobody registered under this key.
    let r = connect(&addr, &a, &c.public());
    assert!(matches!(r, Err(Error::PeerOffline)), "{r:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn knowing_a_paired_public_key_is_not_enough() {
    let (_relay, addr) = relay();
    let (a, b, mallory) = (id(), id(), id());
    let calls = Arc::new(AtomicUsize::new(0));
    let _l = echo_listener(&addr, &b, a.public(), calls.clone());

    // Mallory claims to be A (whose public key she knows) but holds only her
    // own secret.
    let mut tcp = wire::dial(&addr).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    wire::write_json(
        &mut tcp,
        &Hello {
            v: PROTOCOL_VERSION,
            op: Op::Join {
                peer: key_to_hex(&b.public()),
                from: key_to_hex(&a.public()),
            },
        },
    )
    .unwrap();
    let reply: Reply = wire::read_json(&mut tcp, 4096).unwrap();
    assert!(reply.ok, "{reply:?}");
    let mal_secret = snow::Builder::new(TUNNEL_NOISE.parse().unwrap())
        .generate_keypair()
        .unwrap();
    let _ = mallory;
    let prologue = wire::tunnel_prologue(&a.public(), &b.public());
    let mut hs = snow::Builder::new(TUNNEL_NOISE.parse().unwrap())
        .local_private_key(&mal_secret.private)
        .unwrap()
        .remote_public_key(&b.public())
        .unwrap()
        .prologue(&prologue)
        .unwrap()
        .build_initiator()
        .unwrap();
    ct_tunnel::hs_write(&mut tcp, &mut hs, &[]).unwrap();
    let r = ct_tunnel::hs_read(&mut tcp, &mut hs);
    assert!(matches!(r, Err(Error::Handshake(_))), "{r:?}");
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "handler must not see the impostor"
    );

    // The real A still gets through.
    let mut s = connect(&addr, &a, &b.public()).unwrap();
    s.send_message(b"hi").unwrap();
    assert_eq!(s.recv_message().unwrap().unwrap(), b"ih");
}

#[test]
fn registering_a_key_without_its_secret_fails() {
    let (_relay, addr) = relay();
    let (victim, caller) = (id(), id());
    let forger = snow::Builder::new(REGISTER_NOISE.parse().unwrap())
        .generate_keypair()
        .unwrap();

    let mut tcp = wire::dial(&addr).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    wire::write_json(
        &mut tcp,
        &Hello {
            v: PROTOCOL_VERSION,
            op: Op::Register {
                key: key_to_hex(&victim.public()),
            },
        },
    )
    .unwrap();
    let mut hs = snow::Builder::new(REGISTER_NOISE.parse().unwrap())
        .local_private_key(&forger.private)
        .unwrap()
        .prologue(REGISTER_PROLOGUE)
        .unwrap()
        .build_initiator()
        .unwrap();
    ct_tunnel::hs_write(&mut tcp, &mut hs, &[]).unwrap();
    // The relay's reply is bound to the victim's key; the forger can't use it.
    let r = ct_tunnel::hs_read(&mut tcp, &mut hs);
    assert!(
        r.is_err(),
        "forger should not be able to complete the handshake"
    );
    // Send a made-up "proof" anyway; the relay must hang up.
    wire::write_frame(&mut tcp, &[0x42; 64]).unwrap();
    // (It may send an encrypted close first, which the forger can't read.)
    let mut rest = Vec::new();
    tcp.read_to_end(&mut rest)
        .expect("relay should close the connection");
    assert!(rest.len() < 64, "nothing but a close message");

    // And the victim's key is not reachable through the forger.
    let r = connect(&addr, &caller, &victim.public());
    assert!(matches!(r, Err(Error::PeerOffline)), "{r:?}");
}

/// A man in the middle that forwards everything but flips one byte once
/// `flip_at` bytes have gone from client to relay.
fn tampering_proxy(relay_addr: String, flip_at: usize) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    thread::spawn(move || {
        for client in l.incoming() {
            let Ok(client) = client else { continue };
            let upstream = TcpStream::connect(&relay_addr).unwrap();
            let (mut c_r, mut u_w) = (client.try_clone().unwrap(), upstream.try_clone().unwrap());
            let (mut u_r, mut c_w) = (upstream, client);
            thread::spawn(move || {
                let mut seen = 0usize;
                let mut buf = [0u8; 8192];
                loop {
                    let n = match c_r.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if seen <= flip_at && flip_at < seen + n {
                        buf[flip_at - seen] ^= 0x01;
                    }
                    seen += n;
                    if u_w.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                let _ = u_w.shutdown(std::net::Shutdown::Write);
            });
            thread::spawn(move || {
                let _ = std::io::copy(&mut u_r, &mut c_w);
                let _ = c_w.shutdown(std::net::Shutdown::Write);
            });
        }
    });
    addr
}

#[test]
fn tampering_is_detected_not_delivered() {
    let (_relay, addr) = relay();
    let (a, b) = (id(), id());
    let ap = a.public();
    let (tx, rx) = mpsc::channel();
    let _l = listen(
        &addr,
        &b,
        move |k| *k == ap,
        move |mut s, _| {
            let _ = tx.send(s.recv_message());
        },
    );
    assert!(_l.wait_registered(Duration::from_secs(10)));
    // Flip a byte well inside the 300 KB message, after the handshake.
    let proxy = tampering_proxy(addr.clone(), 150_000);
    let mut s = connect(&proxy, &a, &b.public()).unwrap();
    let msg = vec![0x5au8; 300_000];
    let _ = s.send_message(&msg);
    let got = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(
        matches!(got, Err(Error::Decrypt)),
        "expected a decrypt error, got {:?}",
        got.map(|m| m.map(|m| m.len()))
    );
}

#[test]
fn malformed_hellos_get_errors_not_crashes() {
    let (_relay, addr) = relay();
    for junk in [
        &b"not json"[..],
        b"{}",
        b"{\"v\":1,\"op\":\"join\",\"peer\":\"zz\",\"from\":\"zz\"}",
        b"{\"v\":9,\"op\":\"accept\",\"token\":\"x\"}",
    ] {
        let mut tcp = wire::dial(&addr).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        wire::write_frame(&mut tcp, junk).unwrap();
        let reply: Reply = wire::read_json(&mut tcp, 4096).unwrap();
        assert!(!reply.ok);
        assert_eq!(reply.code, code::BAD_REQUEST);
    }
    // An oversized hello is cut off.
    let mut tcp = wire::dial(&addr).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    wire::write_frame(&mut tcp, &vec![b'{'; 10_000]).unwrap();
    let reply: Reply = wire::read_json(&mut tcp, 4096).unwrap();
    assert!(!reply.ok);
    // The relay still works.
    let (a, b) = (id(), id());
    let code = generate_code();
    let host = {
        let (addr, a, code) = (addr.clone(), a.clone(), code.clone());
        thread::spawn(move || pair_host(&addr, &a, &code, b""))
    };
    join_when_open(&addr, &b, &code, b"").unwrap();
    host.join().unwrap().unwrap();
}

#[test]
fn listener_reconnects_after_relay_restart_and_stops() {
    let r = Relay::bind(
        "127.0.0.1:0",
        Config {
            log: false,
            ..Config::default()
        },
    )
    .unwrap();
    let port = r.local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{port}");
    let handle = r.spawn().unwrap();
    let (a, b) = (id(), id());
    let calls = Arc::new(AtomicUsize::new(0));
    let l = echo_listener(&addr, &b, a.public(), calls);
    handle.stop();
    thread::sleep(Duration::from_millis(300));
    assert!(!l.is_registered());
    let handle = Relay::bind(
        &addr,
        Config {
            log: false,
            ..Config::default()
        },
    )
    .unwrap()
    .spawn()
    .unwrap();
    assert!(
        l.wait_registered(Duration::from_secs(10)),
        "{:?}",
        l.last_error()
    );
    let mut s = connect(&addr, &a, &b.public()).unwrap();
    s.send_message(b"ok").unwrap();
    assert_eq!(s.recv_message().unwrap().unwrap(), b"ko");
    l.stop();
    thread::sleep(Duration::from_millis(300));
    assert!(matches!(
        connect(&addr, &a, &b.public()),
        Err(Error::PeerOffline)
    ));
    handle.stop();
}
