//! The one error type for the tunnel.

use std::io;

/// Everything that can go wrong opening, pairing or using a tunnel.
///
/// Errors never carry secret material or decrypted content.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The network failed (connection refused, reset, ...).
    #[error("network error: {0}")]
    Io(io::Error),
    /// A read or write took longer than its budget.
    #[error("timed out")]
    Timeout,
    /// The relay answered with an error (`code` is machine-readable).
    #[error("relay refused: {message} ({code})")]
    Relay { code: String, message: String },
    /// The device asked for is not connected to the relay right now.
    #[error("that device is not connected to the relay")]
    PeerOffline,
    /// The device asked for is connected but declined this device.
    #[error("that device declined the connection (not paired with this device)")]
    PeerRefused,
    /// The Noise handshake failed: the other side does not hold the key it
    /// claims, or it is not the device we pinned.
    #[error("handshake failed: {0}")]
    Handshake(String),
    /// A message failed authentication: the data was changed in transit.
    #[error("decryption failed: the data was modified in transit")]
    Decrypt,
    /// The connection ended without the other side closing it properly
    /// (possible truncation by someone in the middle).
    #[error("connection cut before the other side finished")]
    Truncated,
    /// The other side sent something that doesn't follow the protocol.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// A frame or message was larger than allowed.
    #[error("frame too large: {len} bytes (limit {max})")]
    TooLarge { len: usize, max: usize },
    /// A public key was malformed or a low-order point.
    #[error("invalid public key")]
    InvalidKey,
    /// The pairing code is not in the expected format.
    #[error("invalid pairing code (expected 8 characters like ABCD-EFGH)")]
    InvalidCode,
    /// Pairing failed: the two sides used different codes (or someone guessed
    /// wrong). Nothing was revealed, and the code can't be used again.
    #[error("pairing failed: the code didn't match")]
    WrongCode,
    /// The code can't be used: unknown, already used, expired or busy.
    #[error("pairing code unavailable: {0}")]
    CodeUnavailable(String),
    /// Pairing was cancelled by the caller.
    #[error("cancelled")]
    Cancelled,
    /// Reading or writing the identity key file failed.
    #[error("identity key: {0}")]
    Key(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Error::Timeout,
            _ => {
                // An `io::Error` that wraps one of ours (from `SecureStream`'s
                // `Read`/`Write`) is unwrapped back.
                if e.get_ref().is_some_and(|inner| inner.is::<Error>()) {
                    let kind = e.kind();
                    return match e.into_inner().map(|inner| inner.downcast::<Error>()) {
                        Some(Ok(inner)) => *inner,
                        _ => Error::Io(io::Error::from(kind)),
                    };
                }
                Error::Io(e)
            }
        }
    }
}

impl From<Error> for io::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::Io(e) => e,
            Error::Timeout => io::Error::new(io::ErrorKind::TimedOut, Error::Timeout),
            e @ Error::Decrypt => io::Error::new(io::ErrorKind::InvalidData, e),
            e @ (Error::Protocol(_) | Error::TooLarge { .. }) => {
                io::Error::new(io::ErrorKind::InvalidData, e)
            }
            e @ Error::Truncated => io::Error::new(io::ErrorKind::UnexpectedEof, e),
            e => io::Error::other(e),
        }
    }
}

impl Error {
    pub(crate) fn protocol(msg: impl Into<String>) -> Self {
        Error::Protocol(msg.into())
    }
}
