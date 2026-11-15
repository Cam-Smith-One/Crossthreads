//! `ct-core` — the schema, connector contract, and normalization primitives
//! shared by every Crossthreads surface (daemon, CLI, MCP, desktop).
//!
//! See `docs/ARCHITECTURE.md` for the component map and `docs/DECISIONS.md`
//! for the choices behind this layout.

pub mod connector;
pub mod hash;
pub mod model;
pub mod redact;
pub mod repo;

/// Directory name Crossthreads runs its own model-CLI calls from (weekly
/// review, insights, theme names). Those calls are sessions too, and each tool
/// records them like any other; the indexer skips sessions whose project is a
/// directory of this name, so Crossthreads never indexes its own homework.
pub const HELPER_PROJECT_DIR: &str = "crossthreads-llm-scratch";

/// True when a session's project path is Crossthreads' own helper directory.
pub fn is_helper_project(project: Option<&str>) -> bool {
    project.is_some_and(|p| {
        p.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next() == Some(HELPER_PROJECT_DIR)
    })
}

pub use connector::{Connector, ConnectorError, DiscoveredSession};
pub use model::{
    CodeSnippet, Conversation, GitContext, Kind, Message, Role, Source, Tool, ToolCall,
};
