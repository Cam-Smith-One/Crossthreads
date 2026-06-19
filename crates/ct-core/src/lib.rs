//! `ct-core` — the schema, connector contract, and normalization primitives
//! shared by every Crossthreads surface (daemon, CLI, MCP, desktop).
//!
//! See `docs/ARCHITECTURE.md` for the component map and `docs/DECISIONS.md`
//! for the choices behind this layout.

pub mod connector;
pub mod hash;
pub mod model;

pub use connector::{Connector, ConnectorError, DiscoveredSession};
pub use model::{
    CodeSnippet, Conversation, GitContext, Message, Role, Source, Tool, ToolCall,
};
