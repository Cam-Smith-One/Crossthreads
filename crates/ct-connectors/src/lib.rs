//! `ct-connectors` — parsers that turn each source tool's on-disk history into
//! the normalized [`ct_core::Conversation`] schema.
//!
//! MVP connectors (see `docs/DECISIONS.md` ADR-008): Claude Code, Cursor, and
//! Aider — all implemented.

pub mod connectors;
pub mod text;

pub use connectors::{AiderConnector, ClaudeCodeConnector, CursorConnector};

use ct_core::connector::Connector;

/// All built-in connectors. The daemon iterates these to detect and index
/// whatever is present on the machine.
pub fn builtin() -> Vec<Box<dyn Connector>> {
    vec![
        Box::new(ClaudeCodeConnector::new()),
        Box::new(CursorConnector::new()),
        Box::new(AiderConnector::new()),
    ]
}
