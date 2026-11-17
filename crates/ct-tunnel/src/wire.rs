//! The wire format shared by devices and the relay.
//!
//! Every message on a relay connection is a *frame*: a 2-byte big-endian
//! length followed by that many bytes (at most 65535, the Noise message
//! limit). The first frame on a new connection is a plaintext JSON
//! [`Hello`]; what follows depends on the operation. See `docs/RELAY.md`.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Version carried in every [`Hello`].
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest frame on the wire (the Noise message limit).
pub const MAX_FRAME: usize = 65535;
/// Largest control frame (hello, replies, handshake messages).
pub const MAX_CONTROL_FRAME: usize = 4096;

/// Device ↔ relay registration: the device proves it holds the secret for
/// the public key it registers. `K`: the relay is told the key up front;
/// `N`: the relay has no static key of its own.
pub const REGISTER_NOISE: &str = "Noise_KN_25519_ChaChaPoly_BLAKE2s";
/// Prologue mixed into the registration handshake.
pub const REGISTER_PROLOGUE: &[u8] = b"crossthreads-relay-register/1";
/// Device ↔ device: both static keys pinned at pairing.
pub const TUNNEL_NOISE: &str = "Noise_KK_25519_ChaChaPoly_BLAKE2s";

/// Prologue for a device-to-device KK handshake. Binds the protocol version
/// and both keys, so a relay can't splice two unrelated handshakes together.
pub fn tunnel_prologue(initiator: &[u8; 32], responder: &[u8; 32]) -> Vec<u8> {
    let mut p = b"crossthreads-tunnel/1".to_vec();
    p.extend_from_slice(initiator);
    p.extend_from_slice(responder);
    p
}

/// The first frame on every connection to the relay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hello {
    pub v: u32,
    #[serde(flatten)]
    pub op: Op,
}

/// What a new connection is for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Keep a control connection open for this public key (hex). Followed by
    /// a Noise KN handshake that proves possession of its secret.
    Register { key: String },
    /// Connect to the registered device `peer` (hex). `from` is the
    /// initiator's key, a hint the device uses to pick the KK key; KK itself
    /// authenticates it.
    Join { peer: String, from: String },
    /// A device's data connection answering an incoming join.
    Accept { token: String },
    /// A pairing rendezvous on `slot`.
    Pair {
        slot: String,
        role: PairRole,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_secs: Option<u64>,
    },
}

/// Which side of a pairing rendezvous.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairRole {
    Host,
    Join,
}

/// The relay's plaintext answer to a [`Hello`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reply {
    pub ok: bool,
    /// Machine-readable outcome, e.g. `spliced`, `waiting`, `offline`.
    pub code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}

impl Reply {
    pub fn ok(code: &str) -> Self {
        Self {
            ok: true,
            code: code.into(),
            message: String::new(),
        }
    }
    pub fn err(code: &str, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Reply codes.
pub mod code {
    pub const SPLICED: &str = "spliced";
    pub const WAITING: &str = "waiting";
    pub const PAIRED: &str = "paired";
    pub const OFFLINE: &str = "offline";
    pub const REFUSED: &str = "refused";
    pub const TIMEOUT: &str = "timeout";
    pub const BUSY: &str = "busy";
    pub const UNKNOWN: &str = "unknown";
    pub const USED: &str = "used";
    pub const EXPIRED: &str = "expired";
    pub const LIMIT: &str = "limit";
    pub const BAD_REQUEST: &str = "bad_request";
}

/// Messages on an established (encrypted) control connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Ctl {
    /// Device → relay, first encrypted message: completes the possession
    /// proof (only the key holder can encrypt it).
    Ready,
    /// Relay → device: registration accepted.
    Registered,
    /// Relay → device: registration refused (e.g. relay full).
    Refused {
        code: String,
        message: String,
    },
    Ping,
    Pong,
    /// Relay → device: someone wants to connect; dial back with `token`.
    Incoming {
        token: String,
        from: String,
    },
    /// Device → relay: not accepting this one.
    Decline {
        token: String,
    },
}

/// Write one frame.
pub fn write_frame(w: &mut impl Write, data: &[u8]) -> Result<()> {
    if data.len() > MAX_FRAME {
        return Err(Error::TooLarge {
            len: data.len(),
            max: MAX_FRAME,
        });
    }
    let mut buf = Vec::with_capacity(2 + data.len());
    buf.extend_from_slice(&(data.len() as u16).to_be_bytes());
    buf.extend_from_slice(data);
    w.write_all(&buf)?;
    w.flush()?;
    Ok(())
}

/// Read one frame of at most `max` bytes. A clean end of stream before the
/// frame starts is `Ok(None)`; one in the middle is [`Error::Truncated`].
pub fn read_frame_opt(r: &mut impl Read, max: usize) -> Result<Option<Vec<u8>>> {
    let mut len = [0u8; 2];
    match read_full(r, &mut len)? {
        0 => return Ok(None),
        2 => {}
        _ => return Err(Error::Truncated),
    }
    let len = u16::from_be_bytes(len) as usize;
    if len > max {
        return Err(Error::TooLarge { len, max });
    }
    let mut buf = vec![0u8; len];
    if read_full(r, &mut buf)? != len {
        return Err(Error::Truncated);
    }
    Ok(Some(buf))
}

/// [`read_frame_opt`], where the end of the stream is an error.
pub fn read_frame(r: &mut impl Read, max: usize) -> Result<Vec<u8>> {
    read_frame_opt(r, max)?.ok_or(Error::Truncated)
}

/// Read until `buf` is full or the stream ends; returns bytes read.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(filled)
}

/// Write a JSON value as one frame.
pub fn write_json<T: Serialize>(w: &mut impl Write, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| Error::protocol(e.to_string()))?;
    write_frame(w, &bytes)
}

/// Read one frame and parse it as JSON.
pub fn read_json<T: for<'de> Deserialize<'de>>(r: &mut impl Read, max: usize) -> Result<T> {
    let frame = read_frame(r, max)?;
    parse_json(&frame)
}

/// Parse JSON bytes, as a protocol error when malformed.
pub fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| Error::protocol(format!("malformed message: {e}")))
}

/// How long to wait for a TCP connection to the relay.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Dial the relay (`host:port`), trying each resolved address.
pub fn dial(relay_addr: &str) -> Result<TcpStream> {
    let addrs = relay_addr.to_socket_addrs().map_err(|e| {
        Error::Io(io::Error::new(
            e.kind(),
            format!("resolving {relay_addr}: {e}"),
        ))
    })?;
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) => e.into(),
        None => Error::Io(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{relay_addr} did not resolve"),
        )),
    })
}

/// Generate `n` random bytes.
pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| Error::Io(io::Error::other(e.to_string())))?;
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_enforce_limits() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello").unwrap();
        write_frame(&mut buf, &[7u8; 5000]).unwrap();
        let mut r = &buf[..];
        assert_eq!(read_frame(&mut r, 100).unwrap(), b"hello");
        assert!(matches!(
            read_frame(&mut r, 100),
            Err(Error::TooLarge {
                len: 5000,
                max: 100
            })
        ));
        assert!(write_frame(&mut Vec::new(), &vec![0; MAX_FRAME + 1]).is_err());
    }

    #[test]
    fn truncated_and_empty_streams() {
        let mut empty: &[u8] = &[];
        assert!(read_frame_opt(&mut empty, 10).unwrap().is_none());
        let mut half: &[u8] = &[0];
        assert!(matches!(
            read_frame_opt(&mut half, 10),
            Err(Error::Truncated)
        ));
        let mut short: &[u8] = &[0, 5, 1, 2];
        assert!(matches!(read_frame(&mut short, 10), Err(Error::Truncated)));
    }

    #[test]
    fn hello_json_shape() {
        let h = Hello {
            v: PROTOCOL_VERSION,
            op: Op::Pair {
                slot: "ABCD".into(),
                role: PairRole::Host,
                ttl_secs: Some(600),
            },
        };
        let s = serde_json::to_string(&h).unwrap();
        assert_eq!(
            s,
            r#"{"v":1,"op":"pair","slot":"ABCD","role":"host","ttl_secs":600}"#
        );
        assert_eq!(parse_json::<Hello>(s.as_bytes()).unwrap(), h);
        assert!(parse_json::<Hello>(b"{\"v\":1,\"op\":\"nope\"}").is_err());
        assert!(parse_json::<Hello>(b"\xff\xfe").is_err());
    }
}
