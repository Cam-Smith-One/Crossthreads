//! Pairing two devices with a short code, through the relay.
//!
//! The code is 8 characters (`ABCD-EFGH`, ~39.6 bits). The first 4 are the
//! rendezvous slot the relay matches on; the whole code is the SPAKE2
//! password. So the relay learns 4 characters and has to guess the other
//! ~19.8 bits online, with one try per pairing; anyone else has to guess all
//! of it. After SPAKE2 both sides confirm the key (a MAC each way) before
//! anything else is sent, then swap their static public keys and payloads,
//! encrypted with keys derived from the SPAKE2 key.

use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity as SpakeId, Password, Spake2};
use zeroize::Zeroizing;

use crate::client::{read_reply, send_hello};
use crate::error::{Error, Result};
use crate::identity::{is_valid_public, Identity};
use crate::wire::{self, code, Op, PairRole};

/// The code alphabet: no 0/O, 1/I/L.
pub const CODE_ALPHABET: &[u8; 31] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
/// Characters in a code.
pub const CODE_LEN: usize = 8;
/// Characters of the code that name the rendezvous slot.
pub const SLOT_LEN: usize = 4;
/// How long a hosted code waits by default.
pub const DEFAULT_PAIR_TTL: Duration = Duration::from_secs(600);
/// Largest payload each side may send.
pub const MAX_PAIR_PAYLOAD: usize = 16 * 1024;
/// Budget for each step once both sides are connected.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);

const ID_HOST: &[u8] = b"crossthreads-pair-host";
const ID_JOIN: &[u8] = b"crossthreads-pair-join";

/// What pairing yields: the other device's static public key (pin it) and
/// the opaque payload it sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub peer_public: [u8; 32],
    pub peer_payload: Vec<u8>,
}

/// Options for [`pair_host_with`].
#[derive(Debug, Clone)]
pub struct PairOptions {
    /// How long the code stays open (the relay caps it, 15 min by default).
    pub ttl: Duration,
    /// Set to `true` from another thread to give up waiting.
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Default for PairOptions {
    fn default() -> Self {
        Self {
            ttl: DEFAULT_PAIR_TTL,
            cancel: None,
        }
    }
}

/// A fresh random code, shown as `XXXX-XXXX`.
pub fn generate_code() -> String {
    let mut out = String::with_capacity(CODE_LEN + 1);
    while out.len() < CODE_LEN + 1 {
        let Ok(bytes) = wire::random_bytes::<32>() else {
            continue;
        };
        for b in bytes {
            // Rejection sampling: 248 = 8 * 31, so every character is equally likely.
            if b < 248 && out.len() < CODE_LEN + 1 {
                if out.len() == SLOT_LEN {
                    out.push('-');
                }
                out.push(CODE_ALPHABET[(b % 31) as usize] as char);
            }
        }
    }
    out
}

/// Normalise a typed code: case-insensitive, dashes and spaces ignored.
/// Returns the 8 code characters.
pub fn normalize_code(code: &str) -> Result<String> {
    let s: String = code
        .chars()
        .filter(|c| !matches!(c, '-' | ' ' | '\t'))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if s.len() != CODE_LEN || !s.bytes().all(|b| CODE_ALPHABET.contains(&b)) {
        return Err(Error::InvalidCode);
    }
    Ok(s)
}

/// True if `slot` is a well-formed rendezvous slot (used by the relay).
pub fn is_valid_slot(slot: &str) -> bool {
    slot.len() == SLOT_LEN && slot.bytes().all(|b| CODE_ALPHABET.contains(&b))
}

/// Host a pairing: open the code at the relay and wait (up to 10 minutes)
/// for the other device to join with it.
pub fn pair_host(
    relay_addr: &str,
    identity: &Identity,
    code: &str,
    my_payload: &[u8],
) -> Result<Paired> {
    pair_host_with(
        relay_addr,
        identity,
        code,
        my_payload,
        &PairOptions::default(),
    )
}

/// [`pair_host`] with a custom lifetime and a cancel flag.
pub fn pair_host_with(
    relay_addr: &str,
    identity: &Identity,
    code: &str,
    my_payload: &[u8],
    opts: &PairOptions,
) -> Result<Paired> {
    let code = normalize_code(code)?;
    check_payload(my_payload)?;
    let slot = code[..SLOT_LEN].to_string();
    let mut tcp = wire::dial(relay_addr)?;
    tcp.set_write_timeout(Some(STEP_TIMEOUT))?;
    tcp.set_read_timeout(Some(STEP_TIMEOUT))?;
    send_hello(
        &mut tcp,
        Op::Pair {
            slot: slot.clone(),
            role: PairRole::Host,
            ttl_secs: Some(opts.ttl.as_secs().max(1)),
        },
    )?;
    let reply = read_reply(&mut tcp)?;
    if !reply.ok {
        return Err(unavailable(reply.code, reply.message));
    }
    wait_for_joiner(&tcp, opts)?;
    let reply = read_reply(&mut tcp)?;
    if !reply.ok {
        return Err(unavailable(reply.code, reply.message));
    }
    exchange(&mut tcp, Side::Host, &code, &slot, identity, my_payload)
}

/// Join a pairing hosted by the other device, with the code it shows.
pub fn pair_join(
    relay_addr: &str,
    identity: &Identity,
    code: &str,
    my_payload: &[u8],
) -> Result<Paired> {
    let code = normalize_code(code)?;
    check_payload(my_payload)?;
    let slot = code[..SLOT_LEN].to_string();
    let mut tcp = wire::dial(relay_addr)?;
    tcp.set_write_timeout(Some(STEP_TIMEOUT))?;
    tcp.set_read_timeout(Some(STEP_TIMEOUT))?;
    send_hello(
        &mut tcp,
        Op::Pair {
            slot: slot.clone(),
            role: PairRole::Join,
            ttl_secs: None,
        },
    )?;
    let reply = read_reply(&mut tcp)?;
    if !reply.ok {
        return Err(unavailable(reply.code, reply.message));
    }
    exchange(&mut tcp, Side::Join, &code, &slot, identity, my_payload)
}

fn check_payload(p: &[u8]) -> Result<()> {
    if p.len() > MAX_PAIR_PAYLOAD {
        return Err(Error::TooLarge {
            len: p.len(),
            max: MAX_PAIR_PAYLOAD,
        });
    }
    Ok(())
}

fn unavailable(code: String, message: String) -> Error {
    match code.as_str() {
        code::UNKNOWN => Error::CodeUnavailable("no device is waiting with that code".into()),
        code::USED => Error::CodeUnavailable("that code was already used".into()),
        code::EXPIRED => Error::CodeUnavailable("that code expired".into()),
        code::BUSY => Error::CodeUnavailable("that code is in use, generate a new one".into()),
        _ => Error::Relay { code, message },
    }
}

/// Block until the relay has data for us (the joiner arrived), the deadline
/// passes, or the caller cancels. Uses `peek`, so no frame is half-read.
fn wait_for_joiner(tcp: &TcpStream, opts: &PairOptions) -> Result<()> {
    // The relay answers "expired" itself at the deadline; allow some slack.
    let deadline = Instant::now() + opts.ttl + Duration::from_secs(10);
    tcp.set_read_timeout(Some(Duration::from_millis(250)))?;
    let mut probe = [0u8; 1];
    loop {
        if opts
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::SeqCst))
        {
            return Err(Error::Cancelled);
        }
        match tcp.peek(&mut probe) {
            Ok(0) => {
                return Err(Error::Relay {
                    code: "closed".into(),
                    message: "the relay closed the connection".into(),
                })
            }
            Ok(_) => break,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e.into()),
        }
        if Instant::now() > deadline {
            return Err(Error::CodeUnavailable("that code expired".into()));
        }
    }
    tcp.set_read_timeout(Some(STEP_TIMEOUT))?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Host,
    Join,
}

struct Keys {
    transcript: [u8; 32],
    confirm_host: Zeroizing<[u8; 32]>,
    confirm_join: Zeroizing<[u8; 32]>,
    enc_host: Zeroizing<[u8; 32]>,
    enc_join: Zeroizing<[u8; 32]>,
}

fn derive(spake_key: &[u8], slot: &str, msg_host: &[u8], msg_join: &[u8]) -> Result<Keys> {
    let mut h = Sha256::new();
    h.update(b"crossthreads-pair/1\0");
    for part in [slot.as_bytes(), msg_host, msg_join] {
        h.update((part.len() as u32).to_be_bytes());
        h.update(part);
    }
    let transcript: [u8; 32] = h.finalize().into();
    let hk = Hkdf::<Sha256>::new(Some(&transcript), spake_key);
    let expand = |info: &[u8]| -> Result<Zeroizing<[u8; 32]>> {
        let mut k = Zeroizing::new([0u8; 32]);
        hk.expand(info, k.as_mut())
            .map_err(|_| Error::protocol("key derivation failed"))?;
        Ok(k)
    };
    Ok(Keys {
        transcript,
        confirm_host: expand(b"ct-pair confirm host")?,
        confirm_join: expand(b"ct-pair confirm join")?,
        enc_host: expand(b"ct-pair encrypt host")?,
        enc_join: expand(b"ct-pair encrypt join")?,
    })
}

fn mac(key: &[u8; 32], transcript: &[u8; 32]) -> Result<Hmac<Sha256>> {
    let mut m =
        <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(|_| Error::protocol("bad MAC key"))?;
    m.update(transcript);
    Ok(m)
}

fn seal(
    key: &[u8; 32],
    transcript: &[u8; 32],
    public: &[u8; 32],
    payload: &[u8],
) -> Result<Vec<u8>> {
    let mut plain = Zeroizing::new(Vec::with_capacity(32 + payload.len()));
    plain.extend_from_slice(public);
    plain.extend_from_slice(payload);
    // Each key encrypts exactly one message, so a zero nonce is safe.
    ChaCha20Poly1305::new(Key::from_slice(key))
        .encrypt(
            Nonce::from_slice(&[0u8; 12]),
            Payload {
                msg: &plain,
                aad: transcript,
            },
        )
        .map_err(|_| Error::protocol("encryption failed"))
}

fn open(key: &[u8; 32], transcript: &[u8; 32], sealed: &[u8]) -> Result<Paired> {
    let plain = Zeroizing::new(
        ChaCha20Poly1305::new(Key::from_slice(key))
            .decrypt(
                Nonce::from_slice(&[0u8; 12]),
                Payload {
                    msg: sealed,
                    aad: transcript,
                },
            )
            .map_err(|_| Error::Decrypt)?,
    );
    if plain.len() < 32 || plain.len() > 32 + MAX_PAIR_PAYLOAD {
        return Err(Error::protocol("bad pairing payload"));
    }
    let mut peer_public = [0u8; 32];
    peer_public.copy_from_slice(&plain[..32]);
    if !is_valid_public(&peer_public) {
        return Err(Error::InvalidKey);
    }
    Ok(Paired {
        peer_public,
        peer_payload: plain[32..].to_vec(),
    })
}

/// Read a frame during the exchange; the other side hanging up after we
/// sent our confirmation means it rejected it (wrong code).
fn read_step(tcp: &mut TcpStream, max: usize, hangup_means_wrong_code: bool) -> Result<Vec<u8>> {
    match wire::read_frame_opt(tcp, max) {
        Ok(Some(f)) => Ok(f),
        Ok(None) | Err(Error::Truncated) if hangup_means_wrong_code => Err(Error::WrongCode),
        Ok(None) | Err(Error::Truncated) => Err(Error::protocol("the other device hung up")),
        Err(Error::Io(e))
            if hangup_means_wrong_code
                && matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ) =>
        {
            Err(Error::WrongCode)
        }
        Err(e) => Err(e),
    }
}

fn exchange(
    tcp: &mut TcpStream,
    side: Side,
    code: &str,
    slot: &str,
    identity: &Identity,
    my_payload: &[u8],
) -> Result<Paired> {
    let password = Password::new(code.as_bytes());
    let (ida, idb) = (SpakeId::new(ID_HOST), SpakeId::new(ID_JOIN));
    let (state, my_msg) = match side {
        Side::Host => Spake2::<Ed25519Group>::start_a(&password, &ida, &idb),
        Side::Join => Spake2::<Ed25519Group>::start_b(&password, &ida, &idb),
    };
    // 1. Both send their SPAKE2 message.
    wire::write_frame(tcp, &my_msg)?;
    let their_msg = read_step(tcp, 256, false)?;
    let key = Zeroizing::new(
        state
            .finish(&their_msg)
            .map_err(|_| Error::protocol("malformed SPAKE2 message"))?,
    );
    let (msg_host, msg_join) = match side {
        Side::Host => (&my_msg, &their_msg),
        Side::Join => (&their_msg, &my_msg),
    };
    let k = derive(&key, slot, msg_host, msg_join)?;
    let mac_len = 32;
    let sealed_max = 32 + MAX_PAIR_PAYLOAD + 16;

    match side {
        Side::Join => {
            // 2. Joiner proves it has the key.
            wire::write_frame(
                tcp,
                &mac(&k.confirm_join, &k.transcript)?.finalize().into_bytes(),
            )?;
            // 3. Host proves it has the key (or hangs up: wrong code).
            let theirs = read_step(tcp, mac_len, true)?;
            mac(&k.confirm_host, &k.transcript)?
                .verify_slice(&theirs)
                .map_err(|_| Error::WrongCode)?;
            // 4. Joiner sends its key and payload, 5. host answers.
            wire::write_frame(
                tcp,
                &seal(&k.enc_join, &k.transcript, &identity.public(), my_payload)?,
            )?;
            let sealed = read_step(tcp, sealed_max, false)?;
            open(&k.enc_host, &k.transcript, &sealed)
        }
        Side::Host => {
            let theirs = read_step(tcp, mac_len, false)?;
            // Wrong code: stop here. Nothing of ours is revealed.
            mac(&k.confirm_join, &k.transcript)?
                .verify_slice(&theirs)
                .map_err(|_| Error::WrongCode)?;
            wire::write_frame(
                tcp,
                &mac(&k.confirm_host, &k.transcript)?.finalize().into_bytes(),
            )?;
            let sealed = read_step(tcp, sealed_max, false)?;
            let peer = open(&k.enc_join, &k.transcript, &sealed)?;
            wire::write_frame(
                tcp,
                &seal(&k.enc_host, &k.transcript, &identity.public(), my_payload)?,
            )?;
            Ok(peer)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_look_right_and_normalise() {
        for _ in 0..200 {
            let c = generate_code();
            assert_eq!(c.len(), 9);
            assert_eq!(&c[4..5], "-");
            let n = normalize_code(&c).unwrap();
            assert_eq!(n.len(), 8);
            assert_eq!(normalize_code(&n.to_lowercase()).unwrap(), n);
        }
        assert!(normalize_code("ABCD-EFG").is_err());
        assert!(normalize_code("ABCD-EFGO").is_err()); // O is not in the alphabet
        assert!(normalize_code("ÄBCD-EFGH").is_err());
        assert!(is_valid_slot("AB2D"));
        assert!(!is_valid_slot("AB0D"));
    }

    #[test]
    fn exchange_over_socket_pair() {
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let a = Identity::generate().unwrap();
        let b = Identity::generate().unwrap();
        let (ap, bp) = (a.public(), b.public());
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            exchange(&mut s, Side::Host, "ABCDEFGH", "ABCD", &a, b"host")
        });
        let mut c = TcpStream::connect(addr).unwrap();
        let joined = exchange(&mut c, Side::Join, "ABCDEFGH", "ABCD", &b, b"join").unwrap();
        let hosted = t.join().unwrap().unwrap();
        assert_eq!(joined.peer_public, ap);
        assert_eq!(joined.peer_payload, b"host");
        assert_eq!(hosted.peer_public, bp);
        assert_eq!(hosted.peer_payload, b"join");
    }
}
