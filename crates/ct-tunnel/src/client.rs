//! Connecting to a paired device through the relay, and staying reachable.

use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use snow::Builder;

use crate::channel::{hs_read, hs_write, Channel, SecureStream, HANDSHAKE_TIMEOUT};
use crate::error::{Error, Result};
use crate::identity::{is_valid_public, key_from_hex, key_to_hex, Identity};
use crate::wire::{
    self, code, Ctl, Hello, Op, Reply, MAX_CONTROL_FRAME, PROTOCOL_VERSION, REGISTER_NOISE,
    REGISTER_PROLOGUE, TUNNEL_NOISE,
};

/// How long `connect` waits for the relay to reach the other device.
pub const JOIN_TIMEOUT: Duration = Duration::from_secs(20);
/// How often a listening device pings the relay.
pub const PING_INTERVAL: Duration = Duration::from_secs(30);
/// Silence on the control connection after which the device reconnects.
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(90);
/// Most incoming handshakes in progress at once; more are declined.
const MAX_INFLIGHT: usize = 32;

pub(crate) fn send_hello(tcp: &mut TcpStream, op: Op) -> Result<()> {
    wire::write_json(
        tcp,
        &Hello {
            v: PROTOCOL_VERSION,
            op,
        },
    )
}

pub(crate) fn read_reply(tcp: &mut TcpStream) -> Result<Reply> {
    match wire::read_json::<Reply>(tcp, MAX_CONTROL_FRAME) {
        Err(Error::Truncated) => Err(Error::Relay {
            code: "closed".into(),
            message: "the relay closed the connection".into(),
        }),
        other => other,
    }
}

fn relay_error(reply: Reply) -> Error {
    Error::Relay {
        code: reply.code,
        message: reply.message,
    }
}

fn noise(pattern: &str) -> Result<snow::params::NoiseParams> {
    pattern
        .parse()
        .map_err(|e: snow::Error| Error::Handshake(e.to_string()))
}

/// Open an encrypted stream to the paired device `peer_public` through the
/// relay at `relay_addr` (`host:port`).
///
/// Runs Noise KK as initiator: the stream exists only if the other side
/// holds the secret for `peer_public` (and it accepted our key).
pub fn connect(
    relay_addr: &str,
    identity: &Identity,
    peer_public: &[u8; 32],
) -> Result<SecureStream> {
    if !is_valid_public(peer_public) {
        return Err(Error::InvalidKey);
    }
    let mut tcp = wire::dial(relay_addr)?;
    tcp.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    tcp.set_read_timeout(Some(JOIN_TIMEOUT + Duration::from_secs(5)))?;
    send_hello(
        &mut tcp,
        Op::Join {
            peer: key_to_hex(peer_public),
            from: key_to_hex(&identity.public()),
        },
    )?;
    let reply = read_reply(&mut tcp)?;
    if !reply.ok {
        return Err(match reply.code.as_str() {
            code::OFFLINE | code::TIMEOUT => Error::PeerOffline,
            code::REFUSED => Error::PeerRefused,
            _ => relay_error(reply),
        });
    }
    tcp.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let prologue = wire::tunnel_prologue(&identity.public(), peer_public);
    let mut hs = Builder::new(noise(TUNNEL_NOISE)?)
        .local_private_key(identity.secret())
        .and_then(|b| b.remote_public_key(peer_public))
        .and_then(|b| b.prologue(&prologue))
        .and_then(|b| b.build_initiator())
        .map_err(|e| Error::Handshake(e.to_string()))?;
    hs_write(&mut tcp, &mut hs, &[])?;
    hs_read(&mut tcp, &mut hs)?;
    tcp.set_read_timeout(None)?;
    tcp.set_write_timeout(None)?;
    Ok(SecureStream::new(Channel::new(hs, tcp)?, *peer_public))
}

/// Answer one incoming join: dial back with the token, run KK as responder
/// with `from` pinned.
fn accept_incoming(
    relay_addr: &str,
    identity: &Identity,
    token: &str,
    from: [u8; 32],
) -> Result<SecureStream> {
    let mut tcp = wire::dial(relay_addr)?;
    tcp.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    tcp.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    send_hello(
        &mut tcp,
        Op::Accept {
            token: token.into(),
        },
    )?;
    let reply = read_reply(&mut tcp)?;
    if !reply.ok {
        return Err(relay_error(reply));
    }
    let prologue = wire::tunnel_prologue(&from, &identity.public());
    let mut hs = Builder::new(noise(TUNNEL_NOISE)?)
        .local_private_key(identity.secret())
        .and_then(|b| b.remote_public_key(&from))
        .and_then(|b| b.prologue(&prologue))
        .and_then(|b| b.build_responder())
        .map_err(|e| Error::Handshake(e.to_string()))?;
    // Fails unless the initiator holds the secret for `from` (KK's `ss`).
    hs_read(&mut tcp, &mut hs)?;
    hs_write(&mut tcp, &mut hs, &[])?;
    tcp.set_read_timeout(None)?;
    tcp.set_write_timeout(None)?;
    Ok(SecureStream::new(Channel::new(hs, tcp)?, from))
}

/// Register `identity` with the relay: Noise KN with the relay, where only
/// the holder of the secret can produce the first transport message.
fn register(relay_addr: &str, identity: &Identity) -> Result<Channel> {
    let mut tcp = wire::dial(relay_addr)?;
    tcp.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    tcp.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    send_hello(
        &mut tcp,
        Op::Register {
            key: key_to_hex(&identity.public()),
        },
    )?;
    let mut hs = Builder::new(noise(REGISTER_NOISE)?)
        .local_private_key(identity.secret())
        .and_then(|b| b.prologue(REGISTER_PROLOGUE))
        .and_then(|b| b.build_initiator())
        .map_err(|e| Error::Handshake(e.to_string()))?;
    hs_write(&mut tcp, &mut hs, &[])?;
    hs_read(&mut tcp, &mut hs)?;
    let ch = Channel::new(hs, tcp)?;
    ch.send_ctl(&Ctl::Ready)?;
    match ch.recv_ctl()? {
        Some(Ctl::Registered) => Ok(ch),
        Some(Ctl::Refused { code, message }) => Err(Error::Relay { code, message }),
        Some(other) => Err(Error::protocol(format!("unexpected {other:?}"))),
        None => Err(Error::Relay {
            code: "closed".into(),
            message: "the relay closed the connection".into(),
        }),
    }
}

type Allow = dyn Fn(&[u8; 32]) -> bool + Send + Sync;
type Handler = dyn Fn(SecureStream, [u8; 32]) + Send + Sync;

struct ListenShared {
    relay_addr: String,
    identity: Identity,
    is_allowed: Box<Allow>,
    handler: Box<Handler>,
    stop: AtomicBool,
    registered: AtomicBool,
    inflight: AtomicUsize,
    control: Mutex<Option<TcpStream>>,
    last_error: Mutex<Option<String>>,
}

/// A running registration with the relay. Dropping it (or calling
/// [`Listener::stop`]) disconnects from the relay; streams already handed
/// to the handler keep working.
pub struct Listener {
    shared: Arc<ListenShared>,
    thread: Option<JoinHandle<()>>,
}

impl Listener {
    /// True while registered with the relay (reachable by paired devices).
    pub fn is_registered(&self) -> bool {
        self.shared.registered.load(Ordering::SeqCst)
    }

    /// The most recent connection problem, if any (for a status display).
    pub fn last_error(&self) -> Option<String> {
        self.shared
            .last_error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Wait until registered or `timeout` passes; returns whether registered.
    pub fn wait_registered(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.is_registered() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        self.is_registered()
    }

    /// Disconnect from the relay and wait for the background thread to end.
    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(tcp) = self
            .shared
            .control
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = tcp.shutdown(Shutdown::Both);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.shared.registered.store(false, Ordering::SeqCst);
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// Stay registered with the relay at `relay_addr` so paired devices can
/// [`connect`] to this one. Reconnects with backoff (1 s doubling to 60 s)
/// when the relay is unreachable.
///
/// For each incoming connection, `is_allowed` is asked about the caller's
/// key; refused keys are declined without a handshake. Allowed ones go
/// through Noise KK with that key pinned, so a caller that merely knows an
/// allowed public key fails. Each accepted stream is passed to `handler`
/// with the caller's public key, on its own thread.
pub fn listen(
    relay_addr: &str,
    identity: &Identity,
    is_allowed: impl Fn(&[u8; 32]) -> bool + Send + Sync + 'static,
    handler: impl Fn(SecureStream, [u8; 32]) + Send + Sync + 'static,
) -> Listener {
    let shared = Arc::new(ListenShared {
        relay_addr: relay_addr.to_string(),
        identity: identity.clone(),
        is_allowed: Box::new(is_allowed),
        handler: Box::new(handler),
        stop: AtomicBool::new(false),
        registered: AtomicBool::new(false),
        inflight: AtomicUsize::new(0),
        control: Mutex::new(None),
        last_error: Mutex::new(None),
    });
    let bg = Arc::clone(&shared);
    let thread = thread::Builder::new()
        .name("ct-tunnel-listen".into())
        .spawn(move || listen_loop(bg))
        .ok();
    if thread.is_none() {
        *shared.last_error.lock().unwrap_or_else(|p| p.into_inner()) =
            Some("could not start the listener thread".into());
    }
    Listener { shared, thread }
}

fn listen_loop(shared: Arc<ListenShared>) {
    let mut backoff = Duration::from_secs(1);
    while !shared.stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        let result = run_session(&shared);
        shared.registered.store(false, Ordering::SeqCst);
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        if let Err(e) = result {
            *shared.last_error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
        }
        // A session that lasted a while was healthy: start backoff over.
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        let until = Instant::now() + backoff;
        while Instant::now() < until && !shared.stop.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(50));
        }
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

fn run_session(shared: &Arc<ListenShared>) -> Result<()> {
    let ch = Arc::new(register(&shared.relay_addr, &shared.identity)?);
    {
        let mut control = shared.control.lock().unwrap_or_else(|p| p.into_inner());
        if shared.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        *control = ch.tcp().try_clone().ok();
    }
    ch.tcp().set_read_timeout(Some(CONTROL_TIMEOUT))?;
    ch.tcp().set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    shared.registered.store(true, Ordering::SeqCst);
    *shared.last_error.lock().unwrap_or_else(|p| p.into_inner()) = None;

    // Pinger: keeps NATs open and lets both ends notice a dead connection.
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let ping_ch = Arc::clone(&ch);
    let pinger = thread::spawn(move || {
        while let Err(mpsc::RecvTimeoutError::Timeout) = done_rx.recv_timeout(PING_INTERVAL) {
            if ping_ch.send_ctl(&Ctl::Ping).is_err() {
                let _ = ping_ch.tcp().shutdown(Shutdown::Both);
                break;
            }
        }
    });

    let result = loop {
        match ch.recv_ctl() {
            Ok(Some(Ctl::Pong)) | Ok(Some(Ctl::Ping)) => {}
            Ok(Some(Ctl::Incoming { token, from })) => incoming(shared, &ch, token, from),
            Ok(Some(other)) => break Err(Error::protocol(format!("unexpected {other:?}"))),
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        }
    };
    drop(done_tx);
    let _ = ch.tcp().shutdown(Shutdown::Both);
    let _ = pinger.join();
    shared
        .control
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take();
    result
}

fn incoming(shared: &Arc<ListenShared>, ch: &Channel, token: String, from: String) {
    let decline = |token: String| {
        let _ = ch.send_ctl(&Ctl::Decline { token });
    };
    let Some(from) = key_from_hex(&from).filter(is_valid_public) else {
        return decline(token);
    };
    if !(shared.is_allowed)(&from) {
        return decline(token);
    }
    if shared.inflight.fetch_add(1, Ordering::SeqCst) >= MAX_INFLIGHT {
        shared.inflight.fetch_sub(1, Ordering::SeqCst);
        return decline(token);
    }
    let counter = Arc::clone(shared);
    let shared = Arc::clone(shared);
    let spawned = thread::Builder::new()
        .name("ct-tunnel-accept".into())
        .spawn(move || {
            let result = accept_incoming(&shared.relay_addr, &shared.identity, &token, from);
            shared.inflight.fetch_sub(1, Ordering::SeqCst);
            if let Ok(stream) = result {
                (shared.handler)(stream, from);
            }
        });
    if spawned.is_err() {
        // The closure (and its counter decrement) never ran.
        counter.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}
