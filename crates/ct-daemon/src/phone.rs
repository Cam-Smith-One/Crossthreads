//! The board from a phone (SPEC FR-BRD-04, ADR-013).
//!
//! A second, separate HTTP listener on this device's Tailscale address serves
//! the same web app to a phone. It is off until the user turns it on, and it
//! never binds every interface (`0.0.0.0`): only the tailnet, so only the
//! user's own devices can reach it, and no hosted relay is involved.
//!
//! A phone is let in by **pairing**: the desktop shows a short one-time code
//! (and a QR code for the link carrying it). The phone sends the code back
//! once and receives a long random token in an `HttpOnly`, `SameSite=Strict`
//! cookie. Only a hash of each token is stored. Each phone can be revoked.
//!
//! What a phone may do is a fixed allowlist of reading operations. Posting to
//! a room is a separate grant per phone, off by default (SPEC §8).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Request;

/// Port the phone listener uses unless told otherwise.
pub const PHONE_PORT: u16 = 47102;
/// How long a pairing code works.
pub const CODE_TTL: Duration = Duration::from_secs(10 * 60);
/// Wrong codes allowed before every open code is thrown away.
const MAX_BAD_CODES: u32 = 5;
/// A phone unused for this long is signed out and must pair again.
pub const IDLE_EXPIRY_SECS: i64 = 30 * 24 * 60 * 60;
/// Cookie that carries a paired phone's token.
pub const COOKIE: &str = "ct_phone";
/// Letters for pairing codes: no 0/O, 1/I/L, so they are easy to type.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// A paired phone, as stored (the token itself is never stored).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPhone {
    id: String,
    name: String,
    token_sha256: String,
    created: i64,
    #[serde(default)]
    last_seen: i64,
    /// May post to rooms ("can post"). Off by default.
    #[serde(default)]
    can_post: bool,
}

/// A paired phone, as shown in Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneInfo {
    pub id: String,
    pub name: String,
    /// Epoch seconds.
    pub created: i64,
    /// Epoch seconds of its last request, 0 if never.
    pub last_seen: i64,
    pub can_post: bool,
}

impl From<&StoredPhone> for PhoneInfo {
    fn from(p: &StoredPhone) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            created: p.created,
            last_seen: p.last_seen,
            can_post: p.can_post,
        }
    }
}

/// `phone.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct PhoneConfig {
    #[serde(default)]
    enabled: bool,
    /// Where to listen, when not this device's tailnet address.
    #[serde(default)]
    addr: Option<String>,
    #[serde(default)]
    phones: Vec<StoredPhone>,
}

#[derive(Debug, Default)]
struct State {
    config: PhoneConfig,
    /// Open pairing codes and when each stops working.
    codes: HashMap<String, Instant>,
    bad_codes: u32,
}

/// Phone access: the paired phones, open pairing codes, and the on/off switch.
#[derive(Debug)]
pub struct PhoneAccess {
    state: Mutex<State>,
    path: Option<PathBuf>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("the OS random number generator failed");
    buf
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(s: &str) -> String {
    hex(&Sha256::digest(s.as_bytes()))
}

/// Compare without leaking how much matched through timing.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

impl PhoneAccess {
    /// Load from `path` (or start empty), persisting changes there.
    pub fn load(path: Option<PathBuf>) -> Self {
        let config = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            state: Mutex::new(State {
                config,
                ..Default::default()
            }),
            path,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("phone state poisoned")
    }

    fn save(&self, state: &State) {
        let Some(path) = &self.path else {
            return;
        };
        if let Ok(json) = serde_json::to_vec_pretty(&state.config) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = crate::local_auth::write_private(path, &json) {
                eprintln!("warn: could not save {}: {e:#}", path.display());
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.lock().config.enabled
    }

    pub fn set_enabled(&self, on: bool) {
        let mut s = self.lock();
        s.config.enabled = on;
        if !on {
            s.codes.clear();
        }
        self.save(&s);
    }

    /// The address saved in `phone.json`, if any.
    pub fn saved_addr(&self) -> Option<String> {
        self.lock().config.addr.clone()
    }

    pub fn phones(&self) -> Vec<PhoneInfo> {
        self.lock()
            .config
            .phones
            .iter()
            .map(PhoneInfo::from)
            .collect()
    }

    /// Open a new one-time pairing code. Returns the code.
    pub fn start_pairing(&self) -> String {
        let bytes = random_bytes::<8>();
        let code: String = bytes
            .iter()
            .map(|b| CODE_ALPHABET[*b as usize % CODE_ALPHABET.len()] as char)
            .collect();
        let mut s = self.lock();
        let t = Instant::now();
        s.codes.retain(|_, exp| *exp > t);
        s.codes.insert(code.clone(), t + CODE_TTL);
        s.bad_codes = 0;
        code
    }

    /// Trade a pairing code for a token. The code works once. Too many wrong
    /// codes close every open one, so guessing is pointless.
    pub fn redeem(&self, code: &str, name: &str) -> Option<String> {
        let code = normalize_code(code);
        let mut s = self.lock();
        let t = Instant::now();
        s.codes.retain(|_, exp| *exp > t);
        if s.codes.remove(&code).is_none() {
            s.bad_codes += 1;
            if s.bad_codes >= MAX_BAD_CODES {
                s.codes.clear();
            }
            return None;
        }
        let token = hex(&random_bytes::<32>());
        let name: String = name.trim().chars().take(40).collect();
        let phone = StoredPhone {
            id: hex(&random_bytes::<4>()),
            name: if name.is_empty() {
                "phone".into()
            } else {
                name
            },
            token_sha256: sha256(&token),
            created: now(),
            last_seen: now(),
            can_post: false,
        };
        s.config.phones.push(phone);
        self.save(&s);
        Some(token)
    }

    /// The phone a token belongs to, noting that it was seen.
    pub fn check(&self, token: &str) -> Option<PhoneInfo> {
        let hash = sha256(token);
        let mut s = self.lock();
        let t = now();
        let phone = s
            .config
            .phones
            .iter_mut()
            .find(|p| same(&p.token_sha256, &hash))?;
        if t - phone.last_seen > IDLE_EXPIRY_SECS {
            let id = phone.id.clone();
            s.config.phones.retain(|p| p.id != id);
            self.save(&s);
            return None;
        }
        let persist = t - phone.last_seen > 300;
        phone.last_seen = t;
        let info = PhoneInfo::from(&*phone);
        if persist {
            self.save(&s);
        }
        Some(info)
    }

    /// Forget a phone. Returns whether it existed.
    pub fn revoke(&self, id: &str) -> bool {
        let mut s = self.lock();
        let before = s.config.phones.len();
        s.config.phones.retain(|p| p.id != id);
        let gone = s.config.phones.len() != before;
        if gone {
            self.save(&s);
        }
        gone
    }

    /// Grant or withdraw posting to rooms. Returns whether the phone exists.
    pub fn set_can_post(&self, id: &str, can_post: bool) -> bool {
        let mut s = self.lock();
        let Some(p) = s.config.phones.iter_mut().find(|p| p.id == id) else {
            return false;
        };
        p.can_post = can_post;
        self.save(&s);
        true
    }
}

/// Where `phone.json` lives. `CROSSTHREADS_PHONE_CONFIG` overrides it.
pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("CROSSTHREADS_PHONE_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("crossthreads")
        .join("phone.json")
}

/// An address the phone listener may bind: a specific interface, never every
/// interface.
pub fn safe_bind(addr: &str) -> Result<(), String> {
    let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() || host == "0.0.0.0" || host == "::" {
        return Err(format!(
            "`{addr}` would let any network reach the board; use this device's Tailscale address"
        ));
    }
    Ok(())
}

/// What a phone may ask for. Everything else is refused: settings, pairing,
/// edits, model calls, opening files on the desktop, and the peer protocol.
/// `RoomPost` is allowed only with the phone's "can post" grant.
pub fn allowed(req: &Request, phone: &PhoneInfo) -> Result<(), &'static str> {
    use Request as R;
    match req {
        R::Ping
        | R::Status
        | R::Facets
        | R::Session
        | R::Search { .. }
        | R::GetConversation { .. }
        | R::Devices
        | R::Glance
        | R::Threads { .. }
        | R::Thread { .. }
        | R::ThreadHandoff { .. }
        | R::Rooms
        | R::RoomMessages { .. }
        | R::RoomHistory { .. }
        | R::Saved
        | R::Resume { .. } => Ok(()),
        R::RoomPost { .. } if phone.can_post => Ok(()),
        R::RoomPost { .. } => Err(
            "this phone can read rooms but not post; allow it in Devices → Phones on the desktop",
        ),
        _ => Err("not available from a phone"),
    }
}

/// The value of `name` in a `Cookie` header.
pub fn cookie<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

/// A QR code for `text`, as SVG.
pub fn qr_svg(text: &str) -> Option<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::svg::Color<'_>>()
            .min_dimensions(220, 220)
            .quiet_zone(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_works_once_and_gives_a_token_that_checks() {
        let p = PhoneAccess::load(None);
        let code = p.start_pairing();
        assert_eq!(code.len(), 8);
        // Typed loosely: lower case, with a space.
        let loose = format!("{} {}", code[..4].to_lowercase(), &code[4..]);
        let token = p.redeem(&loose, "Cam's iPhone").expect("redeems");
        assert!(p.redeem(&code, "again").is_none(), "one use only");
        let phone = p.check(&token).expect("token is valid");
        assert_eq!(phone.name, "Cam's iPhone");
        assert!(!phone.can_post, "posting is off by default");
        assert!(p.check("not-a-token").is_none());

        assert!(p.set_can_post(&phone.id, true));
        assert!(p.check(&token).unwrap().can_post);
        assert!(p.revoke(&phone.id));
        assert!(p.check(&token).is_none(), "revoked");
    }

    #[test]
    fn an_idle_phone_is_signed_out() {
        let p = PhoneAccess::load(None);
        let token = p.redeem(&p.start_pairing(), "old").unwrap();
        p.lock().config.phones[0].last_seen = now() - IDLE_EXPIRY_SECS - 1;
        assert!(p.check(&token).is_none());
        assert!(p.phones().is_empty());
    }

    #[test]
    fn guessing_closes_every_open_code() {
        let p = PhoneAccess::load(None);
        let code = p.start_pairing();
        for _ in 0..MAX_BAD_CODES {
            assert!(p.redeem("WRONG123", "x").is_none());
        }
        assert!(p.redeem(&code, "x").is_none());
    }

    #[test]
    fn tokens_are_stored_hashed_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phone.json");
        let p = PhoneAccess::load(Some(path.clone()));
        p.set_enabled(true);
        let token = p.redeem(&p.start_pairing(), "pixel").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains(&token), "the token itself is never written");

        let again = PhoneAccess::load(Some(path));
        assert!(again.enabled());
        assert_eq!(again.check(&token).unwrap().name, "pixel");
    }

    #[test]
    fn never_binds_every_interface() {
        assert!(safe_bind("0.0.0.0:47102").is_err());
        assert!(safe_bind("[::]:47102").is_err());
        assert!(safe_bind("100.64.1.2:47102").is_ok());
    }

    #[test]
    fn phones_read_but_only_post_with_the_grant() {
        let mut phone = PhoneInfo {
            id: "a".into(),
            name: "p".into(),
            created: 0,
            last_seen: 0,
            can_post: false,
        };
        assert!(allowed(&Request::Rooms, &phone).is_ok());
        let post = Request::RoomPost {
            room: "r".into(),
            text: "hi".into(),
            to: None,
            author: None,
        };
        assert!(allowed(&post, &phone).is_err());
        phone.can_post = true;
        assert!(allowed(&post, &phone).is_ok());
        assert!(allowed(&Request::Reindex, &phone).is_err());
        assert!(allowed(
            &Request::RoomLaunch {
                id: "tk_1".into(),
                allow: true
            },
            &phone
        )
        .is_err());
        assert!(allowed(&Request::PhonePair, &phone).is_err());
        assert!(allowed(
            &Request::SetFederationConfig {
                device: None,
                token: None,
                exclude_tools: None,
                exclude_projects: None,
                relay: None,
            },
            &phone
        )
        .is_err());
    }

    #[test]
    fn reads_a_cookie() {
        assert_eq!(cookie("a=1; ct_phone=abc; b=2", COOKIE), Some("abc"));
        assert_eq!(cookie("a=1", COOKIE), None);
    }

    #[test]
    fn draws_a_qr_code() {
        assert!(qr_svg("http://100.64.1.2:47102/pair#ABCD2345")
            .unwrap()
            .starts_with("<?xml"));
    }
}
