//! A device's long-term X25519 identity.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::Path;

use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// This device's long-term static X25519 keypair.
///
/// The public key is what other devices pin at pairing and what the relay
/// routes by. The secret never leaves the device; `Debug` prints only the
/// fingerprint.
#[derive(Clone)]
pub struct Identity {
    secret: Zeroizing<[u8; 32]>,
    public: [u8; 32],
}

impl Identity {
    /// A fresh random identity (not saved anywhere).
    pub fn generate() -> Result<Self> {
        let mut secret = Zeroizing::new([0u8; 32]);
        getrandom::fill(secret.as_mut())
            .map_err(|e| Error::Key(format!("no system randomness: {e}")))?;
        Ok(Self::from_secret(*secret))
    }

    /// An identity from its 32-byte secret.
    pub fn from_secret(secret: [u8; 32]) -> Self {
        let secret = Zeroizing::new(secret);
        let public = PublicKey::from(&StaticSecret::from(*secret)).to_bytes();
        Self { secret, public }
    }

    /// Load the identity saved at `path`, or create one and save it there.
    ///
    /// The file holds the secret as 64 hex characters and is written
    /// owner-only (`0600` on Unix; the folder is created `0700`). A file that
    /// exists but can't be read as a key is an error: it is never silently
    /// replaced, because that would change this device's identity and unpair
    /// it from every other device.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => {
                tighten_permissions(path);
                let bytes = decode_hex32(text.trim()).ok_or_else(|| {
                    Error::Key(format!("{} is not a valid key file", path.display()))
                })?;
                Ok(Self::from_secret(bytes))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let id = Self::generate()?;
                write_secret(path, &id.secret)?;
                Ok(id)
            }
            Err(e) => Err(Error::Key(format!("reading {}: {e}", path.display()))),
        }
    }

    /// The 32-byte public key.
    pub fn public(&self) -> [u8; 32] {
        self.public
    }

    /// A short, stable fingerprint of the public key for people to compare,
    /// like `ABCD-EFGH-IJKL-MNOP`.
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.public)
    }

    pub(crate) fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

/// The fingerprint of any public key: base32 of the first 10 bytes of its
/// SHA-256 (80 bits), in four groups of four.
pub fn fingerprint(public: &[u8; 32]) -> String {
    let digest = Sha256::digest(public);
    let b32 = base32(&digest[..10]);
    b32.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// True when `public` is a usable X25519 public key: not one of the
/// low-order points that would make every shared secret predictable.
pub fn is_valid_public(public: &[u8; 32]) -> bool {
    let mut probe = Zeroizing::new([0u8; 32]);
    if getrandom::fill(probe.as_mut()).is_err() {
        return false;
    }
    StaticSecret::from(*probe)
        .diffie_hellman(&PublicKey::from(*public))
        .was_contributory()
}

/// Lower-case hex of a key.
pub fn key_to_hex(key: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in key {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Parse 64 hex characters into a key.
pub fn key_from_hex(s: &str) -> Option<[u8; 32]> {
    decode_hex32(s)
}

fn decode_hex32(s: &str) -> Option<[u8; 32]> {
    let s = s.as_bytes();
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in s.chunks(2).enumerate() {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Some(out)
}

fn base32(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &b in data {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn write_secret(path: &Path, secret: &[u8; 32]) -> Result<()> {
    let err = |what: &str, e: std::io::Error| Error::Key(format!("{what} {}: {e}", path.display()));
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        create_private_dir(dir).map_err(|e| err("creating the folder for", e))?;
    }
    let text = Zeroizing::new(key_to_hex(secret) + "\n");
    // Write a temporary file next to it, then rename: a crash never leaves a
    // half-written key behind.
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    {
        let mut file = open_private(&tmp).map_err(|e| err("creating", e))?;
        file.write_all(text.as_bytes())
            .map_err(|e| err("writing", e))?;
        file.sync_all().map_err(|e| err("writing", e))?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        err("saving", e)
    })
}

#[cfg(unix)]
fn open_private(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.is_dir() {
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)
}

#[cfg(unix)]
fn tighten_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        if meta.permissions().mode() & 0o077 != 0 {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(not(unix))]
fn tighten_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_or_create_round_trips_and_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("device.key");
        let a = Identity::load_or_create(&path).unwrap();
        let b = Identity::load_or_create(&path).unwrap();
        assert_eq!(a.public(), b.public());
        assert_eq!(a.fingerprint(), b.fingerprint());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert!(!format!("{a:?}").contains(&key_to_hex(a.secret())));
    }

    #[test]
    fn corrupt_key_file_is_an_error_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.key");
        fs::write(&path, "not a key").unwrap();
        assert!(matches!(
            Identity::load_or_create(&path),
            Err(Error::Key(_))
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "not a key");
    }

    #[test]
    fn fingerprint_shape() {
        let id = Identity::generate().unwrap();
        let fp = id.fingerprint();
        assert_eq!(fp.len(), 19);
        assert_eq!(fp.matches('-').count(), 3);
    }

    #[test]
    fn hex_round_trip_and_rejects_garbage() {
        let id = Identity::generate().unwrap();
        assert_eq!(key_from_hex(&key_to_hex(&id.public())), Some(id.public()));
        assert_eq!(key_from_hex("zz"), None);
        assert_eq!(key_from_hex(&"g".repeat(64)), None);
        assert_eq!(key_from_hex(&"é".repeat(32)), None);
    }

    #[test]
    fn low_order_points_are_invalid() {
        assert!(!is_valid_public(&[0u8; 32]));
        let mut one = [0u8; 32];
        one[0] = 1;
        assert!(!is_valid_public(&one));
        assert!(is_valid_public(&Identity::generate().unwrap().public()));
    }
}
