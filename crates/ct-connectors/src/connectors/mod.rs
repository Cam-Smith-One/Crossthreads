//! Concrete source-tool connectors.

pub mod aider;
pub mod antigravity;
pub mod claude_code;
pub mod cline;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod gemini_cli;
pub mod skills;
pub mod vscode;
pub mod windsurf;

pub use aider::AiderConnector;
pub use antigravity::AntigravityConnector;
pub use claude_code::ClaudeCodeConnector;
pub use cline::ClineConnector;
pub use codex::CodexConnector;
pub use copilot::CopilotConnector;
pub use cursor::CursorConnector;
pub use gemini_cli::GeminiCliConnector;
pub use skills::{ClaudeSkillsConnector, CodexPromptsConnector};
pub use windsurf::WindsurfConnector;
