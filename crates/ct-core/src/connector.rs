//! The `Connector` trait every source-tool parser implements (FR-ING-*).
//!
//! A connector is responsible for: detecting whether its tool's data is present
//! on this machine, discovering individual sessions, and parsing one session
//! into a normalized [`Conversation`]. Parsing is fallible and isolated — one
//! bad session must never abort a whole index run (FR-ING-07).

use std::path::PathBuf;

use crate::model::{Conversation, Tool};

/// A session located on disk, ready to be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSession {
    /// File (or db) the session lives in.
    pub path: PathBuf,
    /// Optional sub-locator within `path` (e.g. a JSONL line range, a db rowid).
    /// Connectors that store one session per file leave this `None`.
    pub locator: Option<String>,
}

impl DiscoveredSession {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            locator: None,
        }
    }
}

/// Errors a connector can raise. Parse failures carry the offending session so
/// the caller can log-and-skip without losing the trail.
#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    #[error("io error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse session {path}: {reason}")]
    Parse { path: PathBuf, reason: String },
    #[error("unrecognized source format for {path}: {reason}")]
    UnknownFormat { path: PathBuf, reason: String },
}

pub type Result<T> = std::result::Result<T, ConnectorError>;

/// A source-tool parser. Implementors are stateless and cheap to construct.
pub trait Connector: Send + Sync {
    /// Which tool this connector handles.
    fn tool(&self) -> Tool;

    /// Format fingerprint stamped onto every [`crate::model::Source`] this
    /// connector produces, e.g. `"claude-code/v1"`. Bump when the parse logic
    /// changes in a way that affects output (FR-ING-08).
    fn fingerprint(&self) -> &'static str;

    /// Is this tool's data present on the machine? Cheap probe of known paths.
    fn detect(&self) -> bool {
        !self.roots().is_empty()
    }

    /// Candidate root directories/files for this tool on this OS. Used by
    /// [`Connector::detect`] and discovery. Only existing paths are returned.
    fn roots(&self) -> Vec<PathBuf>;

    /// Enumerate sessions under the discovered roots.
    fn discover(&self) -> Result<Vec<DiscoveredSession>>;

    /// Parse one discovered session into a normalized conversation.
    fn parse(&self, session: &DiscoveredSession) -> Result<Conversation>;
}
