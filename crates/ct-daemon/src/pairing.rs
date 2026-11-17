//! Adding a device with a short code (SPEC §6.6).
//!
//! On the computer you already use, "Add a device" shows a code like
//! `K7PQ-4XMN` for ten minutes. On the new computer you type it in. The new
//! computer asks every Crossthreads it can see on your Tailscale network
//! whether the code is theirs; the one showing it answers with the shared
//! token, and each adds the other with messaging on. That's the whole setup.
//!
//! Why this is safe:
//! - The request only travels inside Tailscale (WireGuard: encrypted, and only
//!   your own devices are on it). The token is never typed or shown.
//! - A code is 8 characters from 32 (40 bits), lives 10 minutes, and dies
//!   after [`MAX_ATTEMPTS`] wrong guesses, so it can't be guessed online.
//! - Only one code at a time, compared in constant time, and used once.
//! - The new device is added at the address the request came from, not one
//!   it claims.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a code works.
pub const CODE_TTL: Duration = Duration::from_secs(600);
/// Wrong codes allowed before the code is cancelled.
pub const MAX_ATTEMPTS: u32 = 5;
/// Letters and digits that can't be confused with each other.
const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

struct Pending {
    code: String,
    expires: Instant,
    attempts: u32,
}

/// The code this device is showing, if any.
#[derive(Default)]
pub struct DevicePairing {
    pending: Mutex<Option<Pending>>,
}

/// What checking a code found.
#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    Ok,
    /// This device isn't showing a code (or it expired or was used).
    NotPairing,
    Wrong,
}

impl DevicePairing {
    /// Show a new code (replacing any other). Returns it, formatted.
    pub fn start(&self) -> String {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("the OS random number generator failed");
        let code: String = bytes
            .iter()
            .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
            .collect();
        *self.pending.lock().expect("pairing mutex poisoned") = Some(Pending {
            code: code.clone(),
            expires: Instant::now() + CODE_TTL,
            attempts: 0,
        });
        format_code(&code)
    }

    pub fn cancel(&self) {
        *self.pending.lock().expect("pairing mutex poisoned") = None;
    }

    /// Seconds left on the current code, if one is showing.
    pub fn remaining(&self) -> Option<u64> {
        let g = self.pending.lock().expect("pairing mutex poisoned");
        let p = g.as_ref()?;
        p.expires
            .checked_duration_since(Instant::now())
            .map(|d| d.as_secs())
    }

    /// Check a code. A right one is used up; too many wrong ones cancel it.
    pub fn check(&self, code: &str) -> Check {
        let mut g = self.pending.lock().expect("pairing mutex poisoned");
        let Some(p) = g.as_mut() else {
            return Check::NotPairing;
        };
        if Instant::now() > p.expires {
            *g = None;
            return Check::NotPairing;
        }
        if crate::local_auth::same(&normalize(code), &p.code) {
            *g = None;
            return Check::Ok;
        }
        p.attempts += 1;
        if p.attempts >= MAX_ATTEMPTS {
            *g = None;
        }
        Check::Wrong
    }
}

/// `K7PQ4XMN` → `K7PQ-4XMN`.
pub fn format_code(code: &str) -> String {
    let (a, b) = code.split_at(code.len() / 2);
    format!("{a}-{b}")
}

/// What a person typed, as the code: case, spaces and dashes ignored.
pub fn normalize(typed: &str) -> String {
    typed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// A fresh shared token for a device that has none: 32 random bytes,
/// URL-safe base64.
pub fn new_token() -> String {
    use base64::Engine;
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

// ---- the daemon's side ------------------------------------------------------

use crate::{federation, Client, Daemon, Peer, Request, Response, FED_PORT};

impl Daemon {
    /// Show a code for adding another computer. Makes sure this device has a
    /// shared token, and says if the other computer won't be able to find it.
    pub(crate) fn device_pair_start(&self) -> Response {
        let Some(fed) = &self.federation else {
            return Response::Error {
                message: "this Crossthreads was started without device support".into(),
            };
        };
        if fed.token().is_none() {
            fed.set_config(None, Some(new_token()), None, None);
        }
        let problem = if fed.listen_addr.is_none() {
            Some(
                "This computer isn't on Tailscale yet, so the other one can't find it. \
                 Install Tailscale (tailscale.com/download), sign in with the same account \
                 on both computers, then quit and reopen Crossthreads."
                    .to_string(),
            )
        } else {
            None
        };
        let code = self.device_pairing.start();
        Response::DevicePairCode {
            code,
            expires_in: CODE_TTL.as_secs(),
            device: fed.device(),
            addr: fed.listen_addr.clone(),
            problem,
        }
    }

    /// On the new computer: find the computer showing `code` and pair.
    pub(crate) fn device_pair_join(&self, code: &str, addr: Option<&str>) -> Response {
        let fail = |message: &str| Response::Error {
            message: message.to_string(),
        };
        let Some(fed) = &self.federation else {
            return fail("this Crossthreads was started without device support");
        };
        if normalize(code).len() != 8 {
            return fail("a code is 8 letters and numbers, like K7PQ-4XMN");
        }
        let Some(listen) = fed.listen_addr.clone() else {
            return fail(
                "This computer isn't on Tailscale yet. Install Tailscale, sign in with the \
                 same account as your other computer, then quit and reopen Crossthreads.",
            );
        };
        let port = listen
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(FED_PORT);
        let candidates = match addr.map(str::trim).filter(|a| !a.is_empty()) {
            Some(a) => {
                let a = if a.contains(':') {
                    a.to_string()
                } else {
                    format!("{a}:{FED_PORT}")
                };
                let ip_ok = a
                    .parse::<std::net::SocketAddr>()
                    .ok()
                    .is_some_and(|s| match s.ip() {
                        std::net::IpAddr::V4(v4) => federation::is_tailnet_v4(v4),
                        std::net::IpAddr::V6(_) => false,
                    });
                if !ip_ok {
                    return fail("that isn't a Tailscale address (they start with 100.)");
                }
                vec![crate::protocol::DiscoveredDevice {
                    name: String::new(),
                    addr: a,
                    already_approved: false,
                }]
            }
            None => federation::discover(&fed.peers(), FED_PORT, fed.timeout),
        };
        if candidates.is_empty() {
            return fail(
                "No other Crossthreads found on your Tailscale network. Check the other \
                 computer has Crossthreads open and Tailscale connected, or type its \
                 Tailscale address (shown under the code).",
            );
        }
        let req = Request::PeerPairRequest {
            code: normalize(code),
            name: fed.device(),
            port,
        };
        let me = fed.device();
        // Ask them all at once; only the one showing the code says yes.
        let handles: Vec<_> = candidates
            .into_iter()
            .filter(|c| !c.already_approved && c.name != me)
            .map(|c| {
                let req = req.clone();
                std::thread::spawn(move || {
                    let answer =
                        Client::new(c.addr.clone()).call_timeout(&req, Duration::from_secs(5));
                    (c, answer)
                })
            })
            .collect();
        let mut wrong = false;
        let mut found = None;
        for h in handles {
            let Ok((c, answer)) = h.join() else { continue };
            match answer {
                Ok(Response::PeerPaired {
                    name,
                    token,
                    has_peers,
                }) => found = Some((c, name, token, has_peers)),
                Ok(Response::Error { message }) if message.contains("wrong code") => wrong = true,
                _ => {}
            }
        }
        let Some((c, name, token, host_has_peers)) = found else {
            return fail(if wrong {
                "That code didn't match. Check it on the other computer (it changes each time \
                 you open Add a device)."
            } else {
                "No computer accepted that code. Make sure it's still showing on the other \
                 computer (codes last 10 minutes) and both are on Tailscale."
            });
        };
        if federation::check_token_strength(&token).is_err() {
            return fail("the other computer sent an unusable token; update Crossthreads on it");
        }

        // One shared token for all your devices. Whoever already has other
        // devices keeps theirs.
        let mine = fed.token();
        let had_peers = !fed.peers().is_empty();
        let mut note = None;
        match mine {
            Some(own) if had_peers && !host_has_peers && own != token => {
                let adopt = Request::PeerAdoptToken {
                    token: Some(token.clone()),
                    new_token: own,
                };
                match Client::new(c.addr.clone()).call_timeout(&adopt, Duration::from_secs(5)) {
                    Ok(Response::Ok { .. }) => {}
                    _ => {
                        fed.set_config(None, Some(token), None, None);
                        note = Some(
                            "Your other devices need to be added again: they use a different \
                             token."
                                .to_string(),
                        );
                    }
                }
            }
            Some(own) if own == token => {}
            _ => {
                if had_peers {
                    note = Some(format!(
                        "This computer now uses {name}'s devices. Add your other computers again \
                         from {name}."
                    ));
                }
                fed.set_config(None, Some(token), None, None);
            }
        }
        fed.add_peer(Peer {
            name: name.clone(),
            addr: c.addr.clone(),
            rooms: true,
        });
        fed.set_peer_rooms(&name, true);
        self.sync_soon();
        Response::DevicePaired { name, note }
    }

    /// On the computer showing the code: a new computer asks to pair.
    pub(crate) fn peer_pair_request(
        &self,
        code: &str,
        name: &str,
        port: u16,
        from: Option<std::net::SocketAddr>,
    ) -> Response {
        let refuse = |message: &str| Response::Error {
            message: message.to_string(),
        };
        let Some(fed) = &self.federation else {
            return refuse("not pairing");
        };
        let name = name.trim();
        if name.is_empty()
            || name.len() > 64
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return refuse("bad device name");
        }
        match self.device_pairing.check(code) {
            Check::Ok => {}
            Check::Wrong => return refuse("wrong code"),
            Check::NotPairing => return refuse("not pairing"),
        }
        if name == fed.device() {
            return refuse(&format!(
                "both computers are called `{name}`; rename one in Devices first"
            ));
        }
        let Some(from) = from else {
            return refuse("couldn't tell where the request came from");
        };
        if fed.token().is_none() {
            fed.set_config(None, Some(new_token()), None, None);
        }
        let token = fed.token().expect("just set");
        let has_peers = !fed.peers().is_empty();
        // Reach the new device where its request came from: an address on
        // your tailnet, not one it merely claims.
        let addr = std::net::SocketAddr::new(from.ip(), port).to_string();
        fed.add_peer(Peer {
            name: name.to_string(),
            addr,
            rooms: true,
        });
        fed.set_peer_rooms(name, true);
        Response::PeerPaired {
            name: fed.device(),
            token,
            has_peers,
        }
    }

    /// Right after pairing, take the new device's token (it already had other
    /// devices; this one had none).
    pub(crate) fn peer_adopt_token(&self, token: Option<&str>, new_token: &str) -> Response {
        let fed = match self.peer_auth(token) {
            Ok(fed) => fed,
            Err(message) => return Response::Error { message },
        };
        if fed.peers().len() > 1 {
            return Response::Error {
                message: "this device already has other devices; keeping its token".into(),
            };
        }
        if let Err(message) = federation::check_token_strength(new_token) {
            return Response::Error { message };
        }
        fed.set_config(None, Some(new_token.to_string()), None, None);
        Response::Ok { ok: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_works_once() {
        let p = DevicePairing::default();
        let code = p.start();
        assert_eq!(code.len(), 9);
        assert!(p.remaining().unwrap() > 500);
        assert_eq!(p.check(&code.to_lowercase().replace('-', " ")), Check::Ok);
        assert_eq!(p.check(&code), Check::NotPairing);
    }

    #[test]
    fn wrong_codes_cancel_it() {
        let p = DevicePairing::default();
        let code = p.start();
        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(p.check("AAAA-AAAA"), Check::Wrong);
        }
        assert_eq!(p.check(&code), Check::NotPairing);
    }

    #[test]
    fn tokens_are_strong() {
        let t = new_token();
        assert!(crate::federation::check_token_strength(&t).is_ok());
        assert_ne!(t, new_token());
    }
}
