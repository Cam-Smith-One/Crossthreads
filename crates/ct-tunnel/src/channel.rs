//! Noise transport over a TCP stream: [`Channel`] (message-oriented, shared
//! by the relay's control connections) and [`SecureStream`] (a byte stream
//! for the daemon's line protocol).
//!
//! Each Noise transport message travels in one frame (2-byte length, at most
//! 65535 bytes). Its plaintext starts with a type byte: `0` data, `1` close.
//! The close message lets a reader tell a finished conversation from one cut
//! short by someone in the middle.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use snow::{HandshakeState, StatelessTransportState};

use crate::error::{Error, Result};
use crate::wire::{self, Ctl, MAX_CONTROL_FRAME, MAX_FRAME};

const TAG_LEN: usize = 16;
/// Largest plaintext in one Noise message after the type byte.
pub const MAX_CHUNK: usize = MAX_FRAME - TAG_LEN - 1;
/// Default limit for [`SecureStream::recv_message`]: 1 MiB.
pub const MAX_MESSAGE: usize = 1024 * 1024;

const KIND_DATA: u8 = 0;
const KIND_CLOSE: u8 = 1;

/// How long a single handshake step may take.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Send one handshake message as a frame.
pub fn hs_write(tcp: &mut TcpStream, hs: &mut HandshakeState, payload: &[u8]) -> Result<()> {
    let mut buf = vec![0u8; MAX_FRAME];
    let n = hs
        .write_message(payload, &mut buf)
        .map_err(|e| Error::Handshake(e.to_string()))?;
    wire::write_frame(tcp, &buf[..n])
}

/// Read one handshake message; a peer that hangs up mid-handshake is a
/// handshake failure (that's what a device that rejects us looks like).
pub fn hs_read(tcp: &mut TcpStream, hs: &mut HandshakeState) -> Result<Vec<u8>> {
    let frame = match wire::read_frame_opt(tcp, wire::MAX_CONTROL_FRAME) {
        Ok(Some(f)) => f,
        Ok(None) | Err(Error::Truncated) => {
            return Err(Error::Handshake(
                "the other side closed the connection".into(),
            ))
        }
        Err(Error::Io(e))
            if matches!(
                e.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
            ) =>
        {
            return Err(Error::Handshake(
                "the other side closed the connection".into(),
            ))
        }
        Err(e) => return Err(e),
    };
    let mut payload = vec![0u8; frame.len()];
    let n = hs
        .read_message(&frame, &mut payload)
        .map_err(|_| Error::Handshake("the other side failed to authenticate".into()))?;
    payload.truncate(n);
    Ok(payload)
}

struct Tx {
    nonce: u64,
    tcp: TcpStream,
    broken: bool,
    closed: bool,
}

enum RxState {
    Open,
    Closed,
    Broken,
}

struct Rx {
    nonce: u64,
    tcp: TcpStream,
    /// Bytes of a frame read so far (kept across timeouts).
    partial: Vec<u8>,
    /// Decrypted data not yet handed to the reader.
    plain: Vec<u8>,
    pos: usize,
    state: RxState,
}

/// An established Noise transport over TCP. Sending and receiving are
/// independent (separate locks and nonces), so one thread can block reading
/// while another writes.
pub struct Channel {
    transport: StatelessTransportState,
    tcp: TcpStream,
    tx: Mutex<Tx>,
    rx: Mutex<Rx>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Channel {
    /// Wrap a finished handshake.
    pub fn new(hs: HandshakeState, tcp: TcpStream) -> Result<Self> {
        let transport = hs
            .into_stateless_transport_mode()
            .map_err(|e| Error::Handshake(e.to_string()))?;
        Ok(Self {
            transport,
            tx: Mutex::new(Tx {
                nonce: 0,
                tcp: tcp.try_clone()?,
                broken: false,
                closed: false,
            }),
            rx: Mutex::new(Rx {
                nonce: 0,
                tcp: tcp.try_clone()?,
                partial: Vec::new(),
                plain: Vec::new(),
                pos: 0,
                state: RxState::Open,
            }),
            tcp,
        })
    }

    /// The underlying TCP stream (for timeouts and shutdown).
    pub fn tcp(&self) -> &TcpStream {
        &self.tcp
    }

    fn send_kind(&self, kind: u8, data: &[u8]) -> Result<()> {
        if data.len() > MAX_CHUNK {
            return Err(Error::TooLarge {
                len: data.len(),
                max: MAX_CHUNK,
            });
        }
        let mut tx = lock(&self.tx);
        if tx.broken {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the connection failed earlier",
            )));
        }
        if tx.closed {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the connection was closed",
            )));
        }
        if tx.nonce == u64::MAX {
            tx.broken = true;
            return Err(Error::protocol("nonce exhausted"));
        }
        let mut plain = Vec::with_capacity(1 + data.len());
        plain.push(kind);
        plain.extend_from_slice(data);
        let mut frame = vec![0u8; 2 + plain.len() + TAG_LEN];
        let n = self
            .transport
            .write_message(tx.nonce, &plain, &mut frame[2..])
            .map_err(|e| Error::protocol(e.to_string()))?;
        frame[..2].copy_from_slice(&(n as u16).to_be_bytes());
        frame.truncate(2 + n);
        tx.nonce += 1;
        if kind == KIND_CLOSE {
            tx.closed = true;
        }
        if let Err(e) = tx.tcp.write_all(&frame) {
            // A partial frame can't be resumed; the channel is done.
            tx.broken = true;
            return Err(e.into());
        }
        Ok(())
    }

    /// Send one message (at most [`MAX_CHUNK`] bytes).
    pub fn send(&self, data: &[u8]) -> Result<()> {
        self.send_kind(KIND_DATA, data)
    }

    /// Tell the other side we're done sending, then half-close. Idempotent.
    pub fn close(&self) {
        let already = lock(&self.tx).closed;
        if !already {
            let _ = self.send_kind(KIND_CLOSE, &[]);
        }
        let _ = self.tcp.shutdown(Shutdown::Write);
    }

    /// Send one control message (JSON).
    pub fn send_ctl(&self, msg: &Ctl) -> Result<()> {
        let bytes = serde_json::to_vec(msg).map_err(|e| Error::protocol(e.to_string()))?;
        self.send(&bytes)
    }

    /// Receive one control message; `Ok(None)` when the other side closed.
    pub fn recv_ctl(&self) -> Result<Option<Ctl>> {
        match self.recv()? {
            None => Ok(None),
            Some(bytes) if bytes.len() > MAX_CONTROL_FRAME => Err(Error::TooLarge {
                len: bytes.len(),
                max: MAX_CONTROL_FRAME,
            }),
            Some(bytes) => wire::parse_json(&bytes).map(Some),
        }
    }

    /// Receive one message. `Ok(None)` when the other side closed properly.
    pub fn recv(&self) -> Result<Option<Vec<u8>>> {
        let mut rx = lock(&self.rx);
        if rx.pos < rx.plain.len() {
            let out = rx.plain[rx.pos..].to_vec();
            rx.plain.clear();
            rx.pos = 0;
            return Ok(Some(out));
        }
        self.next_data(&mut rx)
    }

    /// Read decrypted bytes into `out` (stream style).
    fn read_into(&self, out: &mut [u8]) -> Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut rx = lock(&self.rx);
        if rx.pos >= rx.plain.len() {
            match self.next_data(&mut rx)? {
                None => return Ok(0),
                Some(data) => {
                    rx.plain = data;
                    rx.pos = 0;
                }
            }
        }
        let n = out.len().min(rx.plain.len() - rx.pos);
        out[..n].copy_from_slice(&rx.plain[rx.pos..rx.pos + n]);
        rx.pos += n;
        if rx.pos == rx.plain.len() {
            rx.plain.clear();
            rx.pos = 0;
        }
        Ok(n)
    }

    /// The next non-empty data message, decrypting frames as they arrive.
    fn next_data(&self, rx: &mut Rx) -> Result<Option<Vec<u8>>> {
        loop {
            match rx.state {
                RxState::Open => {}
                RxState::Closed => return Ok(None),
                RxState::Broken => return Err(Error::Decrypt),
            }
            let frame = match read_frame_resumable(rx)? {
                Some(f) => f,
                None => {
                    // TCP ended without a close message.
                    rx.state = RxState::Broken;
                    return Err(Error::Truncated);
                }
            };
            if rx.nonce == u64::MAX {
                rx.state = RxState::Broken;
                return Err(Error::protocol("nonce exhausted"));
            }
            let mut plain = vec![0u8; frame.len()];
            let n = match self.transport.read_message(rx.nonce, &frame, &mut plain) {
                Ok(n) => n,
                Err(_) => {
                    rx.state = RxState::Broken;
                    return Err(Error::Decrypt);
                }
            };
            rx.nonce += 1;
            plain.truncate(n);
            match plain.first() {
                Some(&KIND_DATA) if plain.len() > 1 => {
                    plain.remove(0);
                    return Ok(Some(plain));
                }
                Some(&KIND_DATA) => continue,
                Some(&KIND_CLOSE) => {
                    rx.state = RxState::Closed;
                    return Ok(None);
                }
                _ => {
                    rx.state = RxState::Broken;
                    return Err(Error::protocol("unknown message type"));
                }
            }
        }
    }
}

/// Read one frame, keeping partial progress in `rx.partial` so a read
/// timeout in the middle of a frame doesn't lose sync. `Ok(None)` on a
/// clean end of stream between frames.
fn read_frame_resumable(rx: &mut Rx) -> Result<Option<Vec<u8>>> {
    loop {
        let need = if rx.partial.len() < 2 {
            2
        } else {
            let len = u16::from_be_bytes([rx.partial[0], rx.partial[1]]) as usize;
            if len < TAG_LEN + 1 {
                rx.state = RxState::Broken;
                return Err(Error::protocol("frame too short"));
            }
            if rx.partial.len() == 2 + len {
                let frame = rx.partial[2..].to_vec();
                rx.partial.clear();
                return Ok(Some(frame));
            }
            2 + len
        };
        let mut tmp = [0u8; 16 * 1024];
        let want = (need - rx.partial.len()).min(tmp.len());
        let n = match rx.tcp.read(&mut tmp[..want]) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        if n == 0 {
            if rx.partial.is_empty() {
                return Ok(None);
            }
            rx.state = RxState::Broken;
            return Err(Error::Truncated);
        }
        rx.partial.extend_from_slice(&tmp[..n]);
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        // Best effort: say goodbye properly, without hanging on a dead peer.
        let _ = self.tcp.set_write_timeout(Some(Duration::from_secs(2)));
        self.close();
        let _ = self.tcp.shutdown(Shutdown::Both);
    }
}

/// An end-to-end encrypted, authenticated byte stream to a paired device.
///
/// Implements [`Read`] and [`Write`], so a line protocol runs over it
/// unchanged. Clones (see [`SecureStream::try_clone`]) share the same
/// session: one can read while another writes. The connection closes
/// properly when the last clone is dropped or [`SecureStream::shutdown`] is
/// called; a reader then sees end-of-stream. If the connection is cut
/// without that, or anything is modified in transit, reads fail
/// ([`Error::Truncated`] / [`Error::Decrypt`], as `io::Error`s of kind
/// `UnexpectedEof` / `InvalidData`) instead of returning bad data.
pub struct SecureStream {
    ch: Arc<Channel>,
    peer: [u8; 32],
}

impl SecureStream {
    pub(crate) fn new(ch: Channel, peer: [u8; 32]) -> Self {
        Self {
            ch: Arc::new(ch),
            peer,
        }
    }

    /// The other device's public key, authenticated by the handshake.
    pub fn peer_public(&self) -> [u8; 32] {
        self.peer
    }

    /// Another handle to the same session (like `TcpStream::try_clone`).
    pub fn try_clone(&self) -> io::Result<SecureStream> {
        Ok(Self {
            ch: Arc::clone(&self.ch),
            peer: self.peer,
        })
    }

    pub fn set_read_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        self.ch.tcp().set_read_timeout(t)
    }

    pub fn set_write_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        self.ch.tcp().set_write_timeout(t)
    }

    /// Finish sending: the other side reads end-of-stream. Reading still
    /// works until the other side closes too.
    pub fn shutdown(&self) -> io::Result<()> {
        self.ch.close();
        Ok(())
    }

    /// Send one length-prefixed message (at most [`MAX_MESSAGE`] bytes).
    pub fn send_message(&mut self, msg: &[u8]) -> Result<()> {
        if msg.len() > MAX_MESSAGE {
            return Err(Error::TooLarge {
                len: msg.len(),
                max: MAX_MESSAGE,
            });
        }
        let mut first = Vec::with_capacity(4 + msg.len().min(MAX_CHUNK));
        first.extend_from_slice(&(msg.len() as u32).to_be_bytes());
        let head = msg.len().min(MAX_CHUNK - 4);
        first.extend_from_slice(&msg[..head]);
        self.ch.send(&first)?;
        for chunk in msg[head..].chunks(MAX_CHUNK) {
            self.ch.send(chunk)?;
        }
        Ok(())
    }

    /// Receive one message sent with [`SecureStream::send_message`];
    /// `Ok(None)` if the other side closed instead.
    pub fn recv_message(&mut self) -> Result<Option<Vec<u8>>> {
        self.recv_message_max(MAX_MESSAGE)
    }

    /// [`SecureStream::recv_message`] with a custom size limit.
    pub fn recv_message_max(&mut self, max: usize) -> Result<Option<Vec<u8>>> {
        let mut len = [0u8; 4];
        let mut got = 0;
        while got < 4 {
            let n = self.ch.read_into(&mut len[got..])?;
            if n == 0 {
                if got == 0 {
                    return Ok(None);
                }
                return Err(Error::Truncated);
            }
            got += n;
        }
        let len = u32::from_be_bytes(len) as usize;
        if len > max {
            return Err(Error::TooLarge { len, max });
        }
        let mut buf = vec![0u8; len];
        let mut got = 0;
        while got < len {
            let n = self.ch.read_into(&mut buf[got..])?;
            if n == 0 {
                return Err(Error::Truncated);
            }
            got += n;
        }
        Ok(Some(buf))
    }
}

impl std::fmt::Debug for SecureStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureStream")
            .field("peer", &crate::identity::fingerprint(&self.peer))
            .finish_non_exhaustive()
    }
}

impl Read for SecureStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Ok(self.ch.read_into(buf)?)
    }
}

impl Write for SecureStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = buf.len().min(MAX_CHUNK);
        self.ch.send(&buf[..n])?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        // Every write is sent immediately.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;
    use crate::wire::{tunnel_prologue, TUNNEL_NOISE};
    use std::net::TcpListener;

    /// Run KK directly over loopback TCP (no relay).
    fn pair_streams() -> (SecureStream, SecureStream) {
        let a = Identity::generate().unwrap();
        let b = Identity::generate().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (ap, bp) = (a.public(), b.public());
        let bs = b.clone();
        let server = std::thread::spawn(move || {
            let (mut tcp, _) = listener.accept().unwrap();
            let prologue = tunnel_prologue(&ap, &bp);
            let mut hs = snow::Builder::new(TUNNEL_NOISE.parse().unwrap())
                .local_private_key(bs.secret())
                .unwrap()
                .remote_public_key(&ap)
                .unwrap()
                .prologue(&prologue)
                .unwrap()
                .build_responder()
                .unwrap();
            hs_read(&mut tcp, &mut hs).unwrap();
            hs_write(&mut tcp, &mut hs, &[]).unwrap();
            SecureStream::new(Channel::new(hs, tcp).unwrap(), ap)
        });
        let mut tcp = TcpStream::connect(addr).unwrap();
        let prologue = tunnel_prologue(&ap, &bp);
        let mut hs = snow::Builder::new(TUNNEL_NOISE.parse().unwrap())
            .local_private_key(a.secret())
            .unwrap()
            .remote_public_key(&bp)
            .unwrap()
            .prologue(&prologue)
            .unwrap()
            .build_initiator()
            .unwrap();
        hs_write(&mut tcp, &mut hs, &[]).unwrap();
        hs_read(&mut tcp, &mut hs).unwrap();
        let client = SecureStream::new(Channel::new(hs, tcp).unwrap(), bp);
        (client, server.join().unwrap())
    }

    #[test]
    fn stream_chunks_large_writes_and_closes_cleanly() {
        let (mut a, mut b) = pair_streams();
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let sent = data.clone();
        let writer = std::thread::spawn(move || {
            a.write_all(&sent).unwrap();
            drop(a);
        });
        let mut got = Vec::new();
        b.read_to_end(&mut got).unwrap();
        writer.join().unwrap();
        assert_eq!(got, data);
    }

    #[test]
    fn messages_and_line_protocol_over_clones() {
        let (mut a, b) = pair_streams();
        let mut b_writer = b.try_clone().unwrap();
        let mut b_reader = io::BufReader::new(b);
        a.write_all(b"{\"op\":\"ping\"}\n").unwrap();
        let mut line = String::new();
        io::BufRead::read_line(&mut b_reader, &mut line).unwrap();
        assert_eq!(line, "{\"op\":\"ping\"}\n");
        b_writer.send_message(&vec![9u8; 200_000]).unwrap();
        assert_eq!(a.recv_message().unwrap().unwrap(), vec![9u8; 200_000]);
        // Over the limit is refused on the receiving side.
        b_writer.send_message(&[1u8; 100]).unwrap();
        assert!(matches!(
            a.recv_message_max(10),
            Err(Error::TooLarge { len: 100, max: 10 })
        ));
    }

    #[test]
    fn cut_connection_is_an_error_not_eof() {
        let (a, mut b) = pair_streams();
        // Cut the TCP connection without the close message.
        a.ch.tcp().shutdown(Shutdown::Both).unwrap();
        let mut buf = [0u8; 10];
        let err = b.read(&mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        drop(a);
    }
}
