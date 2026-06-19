//! `ct-connectors` — parsers that turn each source tool's on-disk history into
//! the normalized [`ct_core::Conversation`] schema.
//!
//! MVP connectors: Claude Code, Cursor, Aider (see `docs/DECISIONS.md` ADR-008).
//! Only Claude Code is implemented so far (Phase 0 pipeline spike).

pub mod connectors;
pub mod text;

pub use connectors::ClaudeCodeConnector;

use ct_core::connector::Connector;

/// All built-in connectors. The daemon iterates these to detect and index
/// whatever is present on the machine.
pub fn builtin() -> Vec<Box<dyn Connector>> {
    vec![Box::new(ClaudeCodeConnector::new())]
}
