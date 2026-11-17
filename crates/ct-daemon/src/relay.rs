//! Reaching your other devices without Tailscale, through an end-to-end
//! encrypted relay (see docs/RELAY.md and the `ct-tunnel` crate).
//!
//! With a relay set (`--relay HOST:PORT`, `CROSSTHREADS_RELAY`, or Devices →
//! name and privacy), this device:
//! - keeps a long-term key (`device.key`, owner-only) that identifies it,
//! - stays registered with the relay so paired devices can reach it, and
//!   accepts connections only from the keys of devices it paired with,
//! - pairs with the same short code as on Tailscale, the relay standing in
//!   for the network (SPAKE2, so the relay can't learn the token), and
//! - reaches a device paired this way at the address `relay:<its key>`.
//!
//! Everything a device says to another is encrypted end to end (Noise KK
//! with both keys pinned at pairing); the relay forwards ciphertext. The peer
//! protocol, with its token, runs unchanged inside.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ct_tunnel::{Identity, SecureStream};

use crate::{Daemon, Request, Response};

/// Addresses of devices reached through the relay start with this.
pub const PREFIX: &str = "relay:";

/// The relay and this device's key, set once at start.
pub struct RelayCtx {
    pub addr: String,
    pub identity: Identity,
}

static CTX: OnceLock<RelayCtx> = OnceLock::new();

/// Use `addr` as the relay, with the key at `key_path` (made on first use).
pub fn init(addr: &str, key_path: &std::path::Path) -> Result<()> {
    let identity = Identity::load_or_create(key_path)
        .with_context(|| format!("loading this device's key from {}", key_path.display()))?;
    let _ = CTX.set(RelayCtx {
        addr: addr.to_string(),
        identity,
    });
    Ok(())
}

pub fn ctx() -> Option<&'static RelayCtx> {
    CTX.get()
}

/// The address of a device reached through the relay.
pub fn peer_addr(public: &[u8; 32]) -> String {
    format!("{PREFIX}{}", ct_tunnel::key_to_hex(public))
}

/// The key in a relay address.
pub fn parse(addr: &str) -> Option<[u8; 32]> {
    ct_tunnel::key_from_hex(addr.strip_prefix(PREFIX)?)
}

/// Send one peer request through the relay and read the answer.
pub fn call(addr: &str, req: &Request, timeout: Duration) -> Result<Response> {
    if !req.is_peer_request() {
        bail!("only the peer protocol goes through the relay");
    }
    let Some(ctx) = ctx() else {
        bail!("{addr} is reached through a relay, and this device has none set");
    };
    let Some(public) = parse(addr) else {
        bail!("bad relay address {addr}");
    };
    let stream = ct_tunnel::connect(&ctx.addr, &ctx.identity, &public)
        .with_context(|| format!("reaching {addr} through the relay"))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut writer = stream.try_clone()?;
    let mut bytes = serde_json::to_vec(req)?;
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()?;
    let mut line = String::new();
    BufReader::new(stream)
        .take(crate::MAX_LINE)
        .read_line(&mut line)?;
    let _ = writer.shutdown();
    Ok(serde_json::from_str(&line)?)
}

impl Daemon {
    /// Stay registered with the relay, answering the peer protocol for
    /// devices paired with this one. Does nothing without a relay.
    pub fn start_relay_listener(&self) -> Option<ct_tunnel::Listener> {
        let ctx = ctx()?;
        let fed = self.federation.clone()?;
        let this = self.clone();
        let listener = ct_tunnel::listen(
            &ctx.addr,
            &ctx.identity,
            move |key| {
                fed.peers()
                    .iter()
                    .any(|p| parse(&p.addr).as_ref() == Some(key))
            },
            move |stream, _key| {
                if let Err(e) = this.serve_secure(stream) {
                    eprintln!("warn: relay connection: {e:#}");
                }
            },
        );
        eprintln!(
            "crossthreadsd: reachable through the relay at {} as {}",
            ctx.addr,
            ctx.identity.fingerprint()
        );
        Some(listener)
    }

    /// The peer protocol over one relayed connection.
    fn serve_secure(&self, stream: SecureStream) -> Result<()> {
        stream.set_read_timeout(Some(Duration::from_secs(60)))?;
        let mut writer = stream.try_clone()?;
        let mut reader = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let n = (&mut reader).take(crate::MAX_LINE).read_line(&mut line)?;
            if n == 0 {
                break;
            }
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Request>(&line) {
                // Pairing over the relay uses its own code exchange.
                Ok(Request::PeerPairRequest { .. }) => Response::Error {
                    message: "pair through the relay's code exchange".into(),
                },
                Ok(req) if req.is_peer_request() => self.handle(req),
                Ok(_) => Response::Error {
                    message: "only the peer protocol is served to other machines".into(),
                },
                Err(e) => Response::Error {
                    message: format!("bad request: {e}"),
                },
            };
            let mut bytes = serde_json::to_vec(&response)?;
            bytes.push(b'\n');
            writer.write_all(&bytes)?;
        }
        Ok(())
    }

    /// While a code is showing, also wait for the new device at the relay.
    pub(crate) fn host_relay_pairing(
        &self,
        code: &str,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    ) {
        let (Some(ctx), Some(fed)) = (ctx(), self.federation.clone()) else {
            return;
        };
        let this = self.clone();
        let code = code.to_string();
        std::thread::spawn(move || {
            let Some(token) = fed.token() else { return };
            let payload = serde_json::json!({
                "name": fed.device(),
                "token": token,
                "has_peers": !fed.peers().is_empty(),
            });
            let opts = ct_tunnel::PairOptions {
                ttl: crate::pairing::CODE_TTL,
                cancel: Some(cancel),
            };
            let paired = match ct_tunnel::pair_host_with(
                &ctx.addr,
                &ctx.identity,
                &code,
                payload.to_string().as_bytes(),
                &opts,
            ) {
                Ok(p) => p,
                Err(ct_tunnel::Error::Cancelled) => return,
                Err(e) => {
                    eprintln!("warn: pairing through the relay: {e}");
                    return;
                }
            };
            let theirs: serde_json::Value =
                serde_json::from_slice(&paired.peer_payload).unwrap_or_default();
            let Some(name) = theirs["name"]
                .as_str()
                .filter(|n| crate::pairing::valid_name(n))
            else {
                eprintln!("warn: a device paired through the relay without a usable name");
                return;
            };
            this.device_pairing.cancel();
            fed.add_peer(crate::Peer {
                name: name.to_string(),
                addr: peer_addr(&paired.peer_public),
                rooms: true,
            });
            fed.set_peer_rooms(name, true);
            eprintln!("crossthreadsd: paired with {name} through the relay");
        });
    }

    /// On the new device: pair through the relay. Returns the other device's
    /// name, token, whether it had other devices, and its address.
    pub(crate) fn join_through_relay(&self, code: &str) -> Result<(String, String, bool, String)> {
        let (Some(ctx), Some(fed)) = (ctx(), &self.federation) else {
            bail!("no relay set");
        };
        let payload = serde_json::json!({ "name": fed.device() });
        let paired = ct_tunnel::pair_join(
            &ctx.addr,
            &ctx.identity,
            code,
            payload.to_string().as_bytes(),
        )
        .map_err(|e| match e {
            ct_tunnel::Error::WrongCode => anyhow::anyhow!(
                "That code didn't match. Check it on the other computer (it changes each time \
                     you open Add a device)."
            ),
            ct_tunnel::Error::CodeUnavailable(_) => anyhow::anyhow!(
                "No computer is showing that code at the relay. Check it's still showing on \
                     the other computer (codes last 10 minutes)."
            ),
            other => anyhow::anyhow!("pairing through the relay failed: {other}"),
        })?;
        let theirs: serde_json::Value = serde_json::from_slice(&paired.peer_payload)?;
        let name = theirs["name"]
            .as_str()
            .filter(|n| crate::pairing::valid_name(n))
            .context("the other computer sent no usable name")?;
        let token = theirs["token"]
            .as_str()
            .context("the other computer sent no token")?;
        Ok((
            name.to_string(),
            token.to_string(),
            theirs["has_peers"].as_bool().unwrap_or(false),
            peer_addr(&paired.peer_public),
        ))
    }
}
