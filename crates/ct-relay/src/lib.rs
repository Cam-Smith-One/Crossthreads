//! The Crossthreads relay: a small, self-hostable forwarder between a
//! user's devices. It matches connections up and copies bytes; everything
//! it forwards is end-to-end encrypted between the devices (Noise KK, or
//! SPAKE2 for pairing), so it can't read or change it undetected. See
//! `docs/RELAY.md`.
//!
//! It does four things:
//! - **register**: a device keeps a control connection open, after proving
//!   it holds the secret for the public key it registers (Noise KN);
//! - **join**: another device asks for a registered key; the relay tells
//!   that device to dial back, then splices the two connections;
//! - **pair**: two connections naming the same slot are spliced, once;
//! - **limits**: connections per IP, new connections per minute, table
//!   sizes, idle timeouts, bandwidth and total bytes per tunnel.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ct_tunnel::wire::{
    self, code, Ctl, Hello, Op, PairRole, Reply, PROTOCOL_VERSION, REGISTER_NOISE,
    REGISTER_PROLOGUE,
};
use ct_tunnel::{
    hs_read, hs_write, is_valid_public, is_valid_slot, key_from_hex, key_to_hex, Channel,
};

/// Relay limits. The defaults suit a small public relay.
#[derive(Debug, Clone)]
pub struct Config {
    /// Open connections, in total.
    pub max_conns: usize,
    /// Open connections from one IP address.
    pub max_conns_per_ip: usize,
    /// New connections from one IP address per minute.
    pub max_new_conns_per_ip_per_min: u32,
    /// Registered devices.
    pub max_registrations: usize,
    /// Pairing slots (open and recently used).
    pub max_slots: usize,
    /// Joins waiting for the device to dial back, in total and per device.
    pub max_pending_joins: usize,
    pub max_pending_joins_per_key: usize,
    /// Longest a pairing code may stay open.
    pub max_pair_ttl: Duration,
    /// A tunnel with no traffic either way for this long is closed.
    pub idle_timeout: Duration,
    /// A control connection silent for this long is dropped (devices ping
    /// every 30 s).
    pub control_timeout: Duration,
    /// Time allowed for the first frame and for handshakes.
    pub hello_timeout: Duration,
    /// How long a join waits for the device to dial back.
    pub join_timeout: Duration,
    /// Bytes per second in each direction of a tunnel (0 = unlimited).
    pub rate_bytes_per_sec: u64,
    /// Bytes in each direction of one tunnel before it is cut (0 = unlimited).
    pub max_bytes_per_conn: u64,
    /// Log connection events (never content) to stderr.
    pub log: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_conns: 4096,
            max_conns_per_ip: 64,
            max_new_conns_per_ip_per_min: 600,
            max_registrations: 10_000,
            max_slots: 4096,
            max_pending_joins: 4096,
            max_pending_joins_per_key: 16,
            max_pair_ttl: Duration::from_secs(15 * 60),
            idle_timeout: Duration::from_secs(10 * 60),
            control_timeout: Duration::from_secs(120),
            hello_timeout: Duration::from_secs(10),
            join_timeout: Duration::from_secs(15),
            rate_bytes_per_sec: 8 * 1024 * 1024,
            max_bytes_per_conn: 4 * 1024 * 1024 * 1024,
            log: true,
        }
    }
}

/// Pairing tunnels carry a few small messages; cap them hard.
const PAIR_MAX_BYTES: u64 = 64 * 1024;
const PAIR_IDLE: Duration = Duration::from_secs(60);
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_HELLO: usize = 1024;

struct Reg {
    id: u64,
    ch: Arc<Channel>,
}

enum Answer {
    Conn(TcpStream, ConnGuard),
    Declined,
}

struct Pending {
    target: [u8; 32],
    tx: SyncSender<Answer>,
}

struct Slot {
    expires: Instant,
    /// `Some` while the host waits; `None` once used (kept until expiry so
    /// the code can't be tried again).
    waiting: Option<(u64, SyncSender<Answer>)>,
}

struct IpStat {
    conns: usize,
    window_start: Instant,
    window_count: u32,
}

#[derive(Default)]
struct State {
    regs: HashMap<[u8; 32], Reg>,
    pending: HashMap<String, Pending>,
    slots: HashMap<String, Slot>,
    ips: HashMap<IpAddr, IpStat>,
    conns: usize,
}

struct Shared {
    config: Config,
    state: Mutex<State>,
    stop: AtomicBool,
    next_id: AtomicU64,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn log(&self, msg: impl AsRef<str>) {
        if self.config.log {
            let t = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            eprintln!("{t} {}", msg.as_ref());
        }
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Count a new connection, or refuse it.
    fn admit(self: &Arc<Self>, ip: IpAddr) -> Result<ConnGuard, &'static str> {
        let c = &self.config;
        let now = Instant::now();
        let mut st = self.state();
        if st.conns >= c.max_conns {
            return Err("relay full");
        }
        let e = st.ips.entry(ip).or_insert(IpStat {
            conns: 0,
            window_start: now,
            window_count: 0,
        });
        if now.duration_since(e.window_start) > Duration::from_secs(60) {
            e.window_start = now;
            e.window_count = 0;
        }
        if e.window_count >= c.max_new_conns_per_ip_per_min {
            return Err("too many new connections from this address");
        }
        if e.conns >= c.max_conns_per_ip {
            return Err("too many connections from this address");
        }
        e.window_count += 1;
        e.conns += 1;
        st.conns += 1;
        Ok(ConnGuard {
            shared: Arc::clone(self),
            ip,
        })
    }
}

/// Holds one connection's place in the counters; released on drop.
struct ConnGuard {
    shared: Arc<Shared>,
    ip: IpAddr,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        let mut st = self.shared.state();
        st.conns = st.conns.saturating_sub(1);
        let now = Instant::now();
        if let Some(e) = st.ips.get_mut(&self.ip) {
            e.conns = e.conns.saturating_sub(1);
            if e.conns == 0 && now.duration_since(e.window_start) > Duration::from_secs(60) {
                st.ips.remove(&self.ip);
            }
        }
        if st.ips.len() > 100_000 {
            st.ips.retain(|_, e| {
                e.conns > 0 || now.duration_since(e.window_start) <= Duration::from_secs(60)
            });
        }
    }
}

/// A bound relay, not yet serving.
pub struct Relay {
    listener: TcpListener,
    shared: Arc<Shared>,
}

/// A relay serving on a background thread.
pub struct RelayHandle {
    addr: SocketAddr,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Relay {
    /// Bind the listening socket.
    pub fn bind(addr: impl ToSocketAddrs, config: Config) -> io::Result<Relay> {
        Ok(Relay {
            listener: TcpListener::bind(addr)?,
            shared: Arc::new(Shared {
                config,
                state: Mutex::new(State::default()),
                stop: AtomicBool::new(false),
                next_id: AtomicU64::new(1),
            }),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Serve until stopped (blocks).
    pub fn run(self) {
        let shared = self.shared;
        shared.log(format!(
            "crossthreads-relay listening on {}",
            self.listener
                .local_addr()
                .map(|a| a.to_string())
                .unwrap_or_default()
        ));
        for conn in self.listener.incoming() {
            if shared.stop.load(Ordering::SeqCst) {
                break;
            }
            let tcp = match conn {
                Ok(t) => t,
                Err(e) => {
                    shared.log(format!("accept failed: {e}"));
                    // E.g. out of file descriptors: don't spin.
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }
            };
            let Ok(peer) = tcp.peer_addr() else { continue };
            match shared.admit(peer.ip()) {
                Ok(guard) => {
                    let s = Arc::clone(&shared);
                    let spawned = thread::Builder::new()
                        .name("relay-conn".into())
                        .spawn(move || handle(s, tcp, peer, guard));
                    if spawned.is_err() {
                        shared.log("could not start a thread; connection dropped");
                    }
                }
                Err(why) => {
                    shared.log(format!("{} refused: {why}", peer.ip()));
                    let _ = tcp.set_write_timeout(Some(Duration::from_secs(1)));
                    let mut tcp = tcp;
                    let _ = wire::write_json(&mut tcp, &Reply::err(code::LIMIT, why));
                }
            }
        }
    }

    /// Serve on a background thread.
    pub fn spawn(self) -> io::Result<RelayHandle> {
        let addr = self.local_addr()?;
        let shared = Arc::clone(&self.shared);
        let thread = thread::Builder::new()
            .name("relay-accept".into())
            .spawn(move || self.run())?;
        Ok(RelayHandle {
            addr,
            shared,
            thread: Some(thread),
        })
    }
}

impl RelayHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stop accepting and drop every registration.
    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let mut wake = self.addr;
        if wake.ip().is_unspecified() {
            wake.set_ip(match wake.ip() {
                IpAddr::V4(_) => IpAddr::from([127, 0, 0, 1]),
                IpAddr::V6(_) => IpAddr::from([0u16, 0, 0, 0, 0, 0, 0, 1]),
            });
        }
        let _ = TcpStream::connect_timeout(&wake, Duration::from_secs(1));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let regs: Vec<_> = self.shared.state().regs.drain().map(|(_, r)| r).collect();
        for r in regs {
            let _ = r.ch.tcp().shutdown(Shutdown::Both);
        }
    }
}

impl Drop for RelayHandle {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.stop_inner();
        }
    }
}

/// Short key label for logs (public keys only; never content).
fn short(key: &[u8; 32]) -> String {
    key_to_hex(key)[..12].to_string()
}

fn reply(tcp: &mut TcpStream, r: &Reply) {
    let _ = wire::write_json(tcp, r);
}

fn handle(shared: Arc<Shared>, mut tcp: TcpStream, peer: SocketAddr, guard: ConnGuard) {
    let _ = tcp.set_nodelay(true);
    let _ = tcp.set_read_timeout(Some(shared.config.hello_timeout));
    let _ = tcp.set_write_timeout(Some(WRITE_TIMEOUT));
    let hello: Hello = match wire::read_json(&mut tcp, MAX_HELLO) {
        Ok(h) => h,
        Err(e) => {
            shared.log(format!("{} bad hello: {e}", peer.ip()));
            return reply(&mut tcp, &Reply::err(code::BAD_REQUEST, "bad hello"));
        }
    };
    if hello.v != PROTOCOL_VERSION {
        return reply(
            &mut tcp,
            &Reply::err(
                code::BAD_REQUEST,
                format!("unsupported protocol version {}", hello.v),
            ),
        );
    }
    match hello.op {
        Op::Register { key } => register(&shared, tcp, peer, &key),
        Op::Join { peer: target, from } => join(&shared, tcp, peer, &target, &from),
        Op::Accept { token } => accept(&shared, tcp, &token, guard),
        Op::Pair {
            slot,
            role,
            ttl_secs,
        } => match role {
            PairRole::Host => pair_host(&shared, tcp, peer, &slot, ttl_secs),
            PairRole::Join => pair_join(&shared, tcp, peer, &slot, guard),
        },
    }
}

fn parse_key(hex: &str) -> Option<[u8; 32]> {
    key_from_hex(hex).filter(is_valid_public)
}

fn register(shared: &Arc<Shared>, mut tcp: TcpStream, peer: SocketAddr, key_hex: &str) {
    let Some(key) = parse_key(key_hex) else {
        return reply(&mut tcp, &Reply::err(code::BAD_REQUEST, "invalid key"));
    };
    // Noise KN, relay as responder, with the claimed key as the initiator's
    // static key. Our second message mixes DH(our ephemeral, claimed key);
    // only the holder of its secret can derive the transport keys, so the
    // device's first transport message is the proof.
    let hs = REGISTER_NOISE.parse().map_err(|_| ()).and_then(|p| {
        snow::Builder::new(p)
            .remote_public_key(&key)
            .and_then(|b| b.prologue(REGISTER_PROLOGUE))
            .and_then(|b| b.build_responder())
            .map_err(|_| ())
    });
    let Ok(mut hs) = hs else { return };
    if hs_read(&mut tcp, &mut hs).is_err() || hs_write(&mut tcp, &mut hs, &[]).is_err() {
        return;
    }
    let ch = match Channel::new(hs, tcp) {
        Ok(ch) => Arc::new(ch),
        Err(_) => return,
    };
    match ch.recv_ctl() {
        Ok(Some(Ctl::Ready)) => {}
        _ => {
            shared.log(format!(
                "{} registration proof failed for {}",
                peer.ip(),
                short(&key)
            ));
            return;
        }
    }
    let id = shared.id();
    {
        let mut st = shared.state();
        if !st.regs.contains_key(&key) && st.regs.len() >= shared.config.max_registrations {
            drop(st);
            let _ = ch.send_ctl(&Ctl::Refused {
                code: code::LIMIT.into(),
                message: "relay has too many devices registered".into(),
            });
            return;
        }
        if let Some(old) = st.regs.insert(
            key,
            Reg {
                id,
                ch: Arc::clone(&ch),
            },
        ) {
            // The same device reconnected; the old connection is stale.
            let _ = old.ch.tcp().shutdown(Shutdown::Both);
        }
    }
    shared.log(format!("{} registered {}", peer.ip(), short(&key)));
    let _ = ch
        .tcp()
        .set_read_timeout(Some(shared.config.control_timeout));
    if ch.send_ctl(&Ctl::Registered).is_ok() {
        loop {
            match ch.recv_ctl() {
                Ok(Some(Ctl::Ping)) => {
                    if ch.send_ctl(&Ctl::Pong).is_err() {
                        break;
                    }
                }
                Ok(Some(Ctl::Pong)) => {}
                Ok(Some(Ctl::Decline { token })) => {
                    let mut st = shared.state();
                    if st.pending.get(&token).is_some_and(|p| p.target == key) {
                        if let Some(p) = st.pending.remove(&token) {
                            let _ = p.tx.try_send(Answer::Declined);
                        }
                    }
                }
                _ => break,
            }
        }
    }
    let mut st = shared.state();
    if st.regs.get(&key).is_some_and(|r| r.id == id) {
        st.regs.remove(&key);
    }
    drop(st);
    shared.log(format!("{} unregistered {}", peer.ip(), short(&key)));
}

fn join(
    shared: &Arc<Shared>,
    mut tcp: TcpStream,
    peer: SocketAddr,
    target_hex: &str,
    from_hex: &str,
) {
    let (Some(target), Some(from)) = (parse_key(target_hex), parse_key(from_hex)) else {
        return reply(&mut tcp, &Reply::err(code::BAD_REQUEST, "invalid key"));
    };
    let Ok(token_bytes) = wire::random_bytes::<16>() else {
        return;
    };
    let token: String = token_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let (tx, rx) = mpsc::sync_channel(1);
    let ch = {
        let mut st = shared.state();
        let Some(reg) = st.regs.get(&target) else {
            drop(st);
            return reply(&mut tcp, &Reply::err(code::OFFLINE, "device not connected"));
        };
        let ch = Arc::clone(&reg.ch);
        let for_key = st.pending.values().filter(|p| p.target == target).count();
        if st.pending.len() >= shared.config.max_pending_joins
            || for_key >= shared.config.max_pending_joins_per_key
        {
            drop(st);
            return reply(
                &mut tcp,
                &Reply::err(code::LIMIT, "too many connections waiting"),
            );
        }
        st.pending.insert(token.clone(), Pending { target, tx });
        ch
    };
    let notified = ch
        .send_ctl(&Ctl::Incoming {
            token: token.clone(),
            from: key_to_hex(&from),
        })
        .is_ok();
    let answer = if notified {
        rx.recv_timeout(shared.config.join_timeout).ok()
    } else {
        None
    };
    shared.state().pending.remove(&token);
    match answer {
        Some(Answer::Conn(mut other, _other_guard)) => {
            let ok = Reply::ok(code::SPLICED);
            if wire::write_json(&mut tcp, &ok).is_err()
                || wire::write_json(&mut other, &ok).is_err()
            {
                return;
            }
            shared.log(format!(
                "{} tunnel {} -> {} opened",
                peer.ip(),
                short(&from),
                short(&target)
            ));
            let c = &shared.config;
            let (up, down) = splice(
                tcp,
                other,
                Limits {
                    idle: c.idle_timeout,
                    rate: c.rate_bytes_per_sec,
                    max_bytes: c.max_bytes_per_conn,
                },
            );
            shared.log(format!(
                "tunnel {} -> {} closed ({up} bytes up, {down} down)",
                short(&from),
                short(&target)
            ));
        }
        Some(Answer::Declined) => reply(&mut tcp, &Reply::err(code::REFUSED, "device declined")),
        None if notified => reply(&mut tcp, &Reply::err(code::TIMEOUT, "device didn't answer")),
        None => reply(&mut tcp, &Reply::err(code::OFFLINE, "device not connected")),
    }
}

fn accept(shared: &Arc<Shared>, mut tcp: TcpStream, token: &str, guard: ConnGuard) {
    let pending = shared.state().pending.remove(token);
    let Some(p) = pending else {
        return reply(
            &mut tcp,
            &Reply::err(code::UNKNOWN, "unknown or expired token"),
        );
    };
    // The joining thread takes over this connection (and its guard).
    if let Err(
        TrySendError::Full(Answer::Conn(mut tcp, _))
        | TrySendError::Disconnected(Answer::Conn(mut tcp, _)),
    ) = p.tx.try_send(Answer::Conn(tcp, guard))
    {
        reply(&mut tcp, &Reply::err(code::TIMEOUT, "the caller gave up"));
    }
}

fn pair_host(
    shared: &Arc<Shared>,
    mut tcp: TcpStream,
    peer: SocketAddr,
    slot: &str,
    ttl_secs: Option<u64>,
) {
    if !is_valid_slot(slot) {
        return reply(&mut tcp, &Reply::err(code::BAD_REQUEST, "invalid slot"));
    }
    let ttl = Duration::from_secs(ttl_secs.unwrap_or(600).max(1)).min(shared.config.max_pair_ttl);
    let id = shared.id();
    let (tx, rx) = mpsc::sync_channel(1);
    let expires = Instant::now() + ttl;
    {
        let mut st = shared.state();
        let now = Instant::now();
        st.slots.retain(|_, s| s.expires > now);
        if st.slots.contains_key(slot) {
            drop(st);
            return reply(&mut tcp, &Reply::err(code::BUSY, "slot in use"));
        }
        if st.slots.len() >= shared.config.max_slots {
            drop(st);
            return reply(
                &mut tcp,
                &Reply::err(code::LIMIT, "too many pairings in progress"),
            );
        }
        st.slots.insert(
            slot.to_string(),
            Slot {
                expires,
                waiting: Some((id, tx)),
            },
        );
    }
    // Slots are logged by count only: the slot is part of the code.
    shared.log(format!("{} pairing opened ({}s)", peer.ip(), ttl.as_secs()));
    if wire::write_json(&mut tcp, &Reply::ok(code::WAITING)).is_err() {
        return remove_waiting(shared, slot, id);
    }
    let mut probe = [0u8; 1];
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Answer::Conn(mut other, _guard)) => {
                let ok = Reply::ok(code::PAIRED);
                if wire::write_json(&mut tcp, &ok).is_err()
                    || wire::write_json(&mut other, &ok).is_err()
                {
                    return;
                }
                shared.log(format!("{} pairing joined", peer.ip()));
                splice(
                    tcp,
                    other,
                    Limits {
                        idle: PAIR_IDLE,
                        rate: 0,
                        max_bytes: PAIR_MAX_BYTES,
                    },
                );
                return;
            }
            Ok(Answer::Declined) => return remove_waiting(shared, slot, id),
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if Instant::now() >= expires || shared.stop.load(Ordering::SeqCst) {
            remove_waiting(shared, slot, id);
            // A joiner may have taken the slot just now.
            if let Ok(Answer::Conn(other, _)) = rx.try_recv() {
                drop(other);
            }
            return reply(
                &mut tcp,
                &Reply::err(code::EXPIRED, "nobody joined in time"),
            );
        }
        // Has the host gone away (or sent something it shouldn't)?
        let gone = tcp.set_nonblocking(true).is_err()
            || match tcp.peek(&mut probe) {
                Ok(_) => true,
                Err(e) => e.kind() != io::ErrorKind::WouldBlock,
            };
        if gone || tcp.set_nonblocking(false).is_err() {
            return remove_waiting(shared, slot, id);
        }
    }
}

fn remove_waiting(shared: &Arc<Shared>, slot: &str, id: u64) {
    let mut st = shared.state();
    if st
        .slots
        .get(slot)
        .is_some_and(|s| s.waiting.as_ref().is_some_and(|(w, _)| *w == id))
    {
        st.slots.remove(slot);
    }
}

fn pair_join(
    shared: &Arc<Shared>,
    mut tcp: TcpStream,
    peer: SocketAddr,
    slot: &str,
    guard: ConnGuard,
) {
    if !is_valid_slot(slot) {
        return reply(&mut tcp, &Reply::err(code::BAD_REQUEST, "invalid slot"));
    }
    let taken = {
        let mut st = shared.state();
        match st.slots.get_mut(slot) {
            Some(s) if s.expires <= Instant::now() => Err(code::EXPIRED),
            // One attempt per code: the slot is burned whatever happens next.
            Some(s) => s.waiting.take().map(|(_, tx)| tx).ok_or(code::USED),
            None => Err(code::UNKNOWN),
        }
    };
    match taken {
        Ok(tx) => {
            shared.log(format!("{} joining a pairing", peer.ip()));
            if let Err(
                TrySendError::Full(Answer::Conn(mut tcp, _))
                | TrySendError::Disconnected(Answer::Conn(mut tcp, _)),
            ) = tx.try_send(Answer::Conn(tcp, guard))
            {
                reply(
                    &mut tcp,
                    &Reply::err(code::UNKNOWN, "the other device left"),
                );
            }
        }
        Err(c) => reply(&mut tcp, &Reply::err(c, "code not available")),
    }
}

#[derive(Clone, Copy)]
struct Limits {
    idle: Duration,
    rate: u64,
    max_bytes: u64,
}

/// Copy bytes both ways until both sides finish (or a limit trips).
/// Returns bytes copied a→b and b→a.
fn splice(a: TcpStream, b: TcpStream, lim: Limits) -> (u64, u64) {
    let started = Instant::now();
    let activity = Arc::new(AtomicU64::new(0));
    let poll = lim.idle.min(Duration::from_secs(5));
    for s in [&a, &b] {
        let _ = s.set_read_timeout(Some(poll));
        let _ = s.set_write_timeout(Some(lim.idle));
    }
    let (Ok(a2), Ok(b2)) = (a.try_clone(), b.try_clone()) else {
        return (0, 0);
    };
    let back_activity = Arc::clone(&activity);
    let lim2 = lim;
    let back = thread::Builder::new()
        .name("relay-splice".into())
        .spawn(move || pump(b2, a2, &lim2, started, &back_activity));
    let up = pump(a, b, &lim, started, &activity);
    let down = back.ok().and_then(|t| t.join().ok()).unwrap_or(0);
    (up, down)
}

fn pump(
    mut src: TcpStream,
    mut dst: TcpStream,
    lim: &Limits,
    started: Instant,
    activity: &AtomicU64,
) -> u64 {
    let mut buf = vec![0u8; 32 * 1024];
    let mut total: u64 = 0;
    let burst = (lim.rate as f64).max(64.0 * 1024.0);
    let mut tokens = burst;
    let mut last = Instant::now();
    let kill = |src: &TcpStream, dst: &TcpStream| {
        let _ = src.shutdown(Shutdown::Both);
        let _ = dst.shutdown(Shutdown::Both);
    };
    loop {
        match src.read(&mut buf) {
            Ok(0) => {
                // Half-close: the other direction may still be talking.
                let _ = dst.shutdown(Shutdown::Write);
                break;
            }
            Ok(n) => {
                total += n as u64;
                activity.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                if lim.max_bytes > 0 && total > lim.max_bytes {
                    kill(&src, &dst);
                    break;
                }
                if dst.write_all(&buf[..n]).is_err() {
                    kill(&src, &dst);
                    break;
                }
                if lim.rate > 0 {
                    let now = Instant::now();
                    tokens = (tokens + now.duration_since(last).as_secs_f64() * lim.rate as f64)
                        .min(burst);
                    last = now;
                    tokens -= n as f64;
                    if tokens < 0.0 {
                        thread::sleep(Duration::from_secs_f64(-tokens / lim.rate as f64));
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                let last_seen = Duration::from_millis(activity.load(Ordering::Relaxed));
                if started.elapsed().saturating_sub(last_seen) >= lim.idle {
                    kill(&src, &dst);
                    break;
                }
            }
            Err(_) => {
                kill(&src, &dst);
                break;
            }
        }
    }
    let _ = dst.flush();
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_ip_limits() {
        let relay = Relay::bind(
            "127.0.0.1:0",
            Config {
                max_conns_per_ip: 2,
                log: false,
                ..Config::default()
            },
        )
        .unwrap();
        let shared = Arc::clone(&relay.shared);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let g1 = shared.admit(ip).unwrap();
        let _g2 = shared.admit(ip).unwrap();
        assert!(shared.admit(ip).is_err());
        drop(g1);
        assert!(shared.admit(ip).is_ok());
        assert!(shared.admit("10.0.0.2".parse().unwrap()).is_ok());
    }
}
