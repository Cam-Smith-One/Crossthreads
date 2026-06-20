//! `ct-connectors` — parsers that turn each source tool's on-disk history into
//! the normalized [`ct_core::Conversation`] schema.
//!
//! Conversation connectors: Claude Code, Cursor, Aider, Codex, Cline, GitHub
//! Copilot Chat, Gemini CLI, Windsurf, and Antigravity (best-effort). Plus
//! skill/prompt connectors for Claude Code and Codex.

pub mod connectors;
pub mod text;

pub use connectors::{
    AiderConnector, AntigravityConnector, ClaudeCodeConnector, ClaudeSkillsConnector,
    ClineConnector, CodexConnector, CodexPromptsConnector, CopilotConnector, CursorConnector,
    GeminiCliConnector, WindsurfConnector,
};

use ct_core::connector::Connector;

/// All built-in connectors. The daemon iterates these to detect and index
/// whatever is present on the machine — conversation threads *and* reusable
/// skill/prompt artifacts.
pub fn builtin() -> Vec<Box<dyn Connector>> {
    vec![
        Box::new(ClaudeCodeConnector::new()),
        Box::new(CursorConnector::new()),
        Box::new(AiderConnector::new()),
        Box::new(CodexConnector::new()),
        Box::new(ClineConnector::new()),
        Box::new(CopilotConnector::new()),
        Box::new(GeminiCliConnector::new()),
        Box::new(WindsurfConnector::new()),
        Box::new(AntigravityConnector::new()),
        Box::new(ClaudeSkillsConnector::new()),
        Box::new(CodexPromptsConnector::new()),
    ]
}
