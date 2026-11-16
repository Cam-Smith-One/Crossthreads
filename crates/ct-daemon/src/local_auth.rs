//! The local key: only your own user account may use this machine's daemon.
//!
//! The daemon listens on loopback, and loopback is shared by every account on
//! the computer. So on first start the daemon writes a random key to a file
//! only your account can read, and every local client (CLI, MCP server, hooks,
//! the desktop shell) presents it before doing anything. Another account on
//! the same machine can connect, but gets nothing beyond `ping`.
//!
//! The browser can't read files, so the web app signs in with a one-time
//! ticket instead: `crossthreads open` asks the daemon (with the key) for a
//! ticket that works once for 60 seconds, and opens `/login#<ticket>`. The page
//! trades it for an `HttpOnly`, `SameSite=Strict` cookie derived from the key,
//! so the key itself never reaches the browser.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Cookie the local web app carries once signed in.
pub const COOKIE: &str = "ct_local";
/// How long a sign-in ticket works.
pub const TICKET_TTL: Duration = Duration::from_secs(60);

/// Where the key lives. `CROSSTHREADS_LOCAL_KEY_FILE` overrides it.
pub fn key_path() -> PathBuf {
    if let Some(p) = std::env::var_os("CROSSTHREADS_LOCAL_KEY_FILE") {
        return PathBuf::from(p);
    }
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("crossthreads")
        .join("local.key")
}

fn random_hex<const N: usize>() -> String {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("the OS random number generator failed");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// The key, for a client: `CROSSTHREADS_LOCAL_KEY`, else the key file.
/// `None` when neither exists (no daemon has run as this account yet).
pub fn read() -> Option<String> {
    if let Ok(k) = std::env::var("CROSSTHREADS_LOCAL_KEY") {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    std::fs::read_to_string(key_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| s.len() >= 32)
}

/// The key, for the daemon: read it, or create it readable by this account
/// only.
pub fn load_or_create() -> Result<String> {
    if let Some(k) = read() {
        restrict(&key_path());
        return Ok(k);
    }
    let path = key_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        restrict_dir(dir);
    }
    let key = random_hex::<32>();
    write_private(&path, key.as_bytes())?;
    Ok(key)
}

/// Write a file that only this account can read, from the moment it exists.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("writing {}", path.display()))?;
        f.write_all(bytes)?;
        restrict(path);
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // On Windows the per-user profile folders are already private.
        std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
    }
}

/// Owner-only file permissions, where that means something.
pub fn restrict(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.exists() {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Owner-only directory permissions, where that means something.
pub fn restrict_dir(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// The cookie value the web app carries: derived from the key, so the key
/// itself never reaches the browser, and stable across daemon restarts.
pub fn http_session(key: &str) -> String {
    let digest = Sha256::digest(format!("crossthreads-http-session-v1:{key}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compare without leaking how much matched through timing.
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Is `addr` (`host:port`) on this machine only?
pub fn is_loopback(addr: &str) -> bool {
    let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// One-time sign-in tickets for the web app.
#[derive(Debug, Default)]
pub struct Tickets(Mutex<HashMap<String, Instant>>);

impl Tickets {
    pub fn issue(&self) -> String {
        let ticket = random_hex::<24>();
        let mut map = self.0.lock().expect("tickets poisoned");
        let now = Instant::now();
        map.retain(|_, exp| *exp > now);
        map.insert(ticket.clone(), now + TICKET_TTL);
        ticket
    }

    /// True once per issued ticket, within its lifetime.
    pub fn redeem(&self, ticket: &str) -> bool {
        let mut map = self.0.lock().expect("tickets poisoned");
        let now = Instant::now();
        map.retain(|_, exp| *exp > now);
        map.remove(ticket.trim()).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_file_is_private_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("local.key");
        std::env::set_var("CROSSTHREADS_LOCAL_KEY_FILE", &path);
        let a = load_or_create().unwrap();
        let b = load_or_create().unwrap();
        std::env::remove_var("CROSSTHREADS_LOCAL_KEY_FILE");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn tickets_work_once() {
        let t = Tickets::default();
        let ticket = t.issue();
        assert!(t.redeem(&ticket));
        assert!(!t.redeem(&ticket));
        assert!(!t.redeem("made-up"));
    }

    #[test]
    fn knows_loopback() {
        assert!(is_loopback("127.0.0.1:47100"));
        assert!(is_loopback("localhost:1"));
        assert!(is_loopback("[::1]:47100"));
        assert!(!is_loopback("100.64.0.2:47100"));
        assert!(!is_loopback("0.0.0.0:47100"));
    }

    #[test]
    fn the_cookie_does_not_reveal_the_key() {
        let s = http_session("k");
        assert_ne!(s, "k");
        assert_eq!(s, http_session("k"));
        assert!(same(&s, &http_session("k")) && !same(&s, &http_session("j")));
    }
}
