//! Concrete source-tool connectors.

pub mod aider;
pub mod claude_code;
pub mod cursor;

pub use aider::AiderConnector;
pub use claude_code::ClaudeCodeConnector;
pub use cursor::CursorConnector;
