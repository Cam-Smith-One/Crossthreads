//! End-to-end encrypted transport between a user's own devices, through a
//! relay that only ever sees ciphertext (see `docs/RELAY.md`).
//!
//! - [`Identity`]: this device's long-term X25519 key.
//! - [`pair_host`] / [`pair_join`]: pair two devices with a short code
//!   (SPAKE2 + key confirmation); each learns the other's public key and an
//!   opaque payload.
//! - [`listen`] / [`connect`]: reach a paired device through the relay over
//!   Noise KK, with both static keys pinned. The result is a
//!   [`SecureStream`] (`Read + Write`).
//!
//! Everything is blocking std I/O; each connection gets its own thread.

mod channel;
mod client;
mod error;
mod identity;
mod pair;
pub mod wire;

pub use channel::{
    hs_read, hs_write, Channel, SecureStream, HANDSHAKE_TIMEOUT, MAX_CHUNK, MAX_MESSAGE,
};
pub use client::{connect, listen, Listener, CONTROL_TIMEOUT, JOIN_TIMEOUT, PING_INTERVAL};
pub use error::{Error, Result};
pub use identity::{fingerprint, is_valid_public, key_from_hex, key_to_hex, Identity};
pub use pair::{
    generate_code, is_valid_slot, normalize_code, pair_host, pair_host_with, pair_join,
    PairOptions, Paired, CODE_ALPHABET, CODE_LEN, DEFAULT_PAIR_TTL, MAX_PAIR_PAYLOAD, SLOT_LEN,
};
