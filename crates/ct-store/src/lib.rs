//! `ct-store` — the single SQLite index and FTS5 keyword search.
//!
//! Persists normalized [`Conversation`]s with content-hash deduplication
//! (FR-ING-06) and answers lexical queries via FTS5/BM25 (FR-SRCH-01, lexical
//! half). Semantic search (`sqlite-vec`) layers onto this same DB next.

use std::path::Path;

use anyhow::{bail, Context, Result};
use ct_core::model::{Conversation, Role};
use rusqlite::{params, Connection};
use serde::Serialize;

mod schema;

pub use schema::SCHEMA_VERSION;

/// Outcome of attempting to persist one conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upsert {
    /// Newly stored.
    Inserted,
    /// Already present (same content hash) — skipped per dedup.
    Duplicate,
}

/// A single keyword-search result, collapsed to the best-matching message of a
/// conversation.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub conversation_id: String,
    pub tool: String,
    pub project: Option<String>,
    pub title: Option<String>,
    pub started_at: Option<String>,
    /// FTS5 snippet with matched terms wrapped in `[` … `]`.
    pub snippet: String,
    /// BM25 score (lower is a better match; we expose it as-is).
    pub score: f64,
    pub source_path: String,
}

/// Handle to the index database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) the index at `path`, applying the schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path.as_ref())
            .with_context(|| format!("opening index at {}", path.as_ref().display()))?;
        Self::init(conn)
    }

    /// In-memory store, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(schema::SCHEMA_SQL)
            .context("applying schema")?;

        // Persist/verify the schema version in the user_version pragma.
        let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if current == 0 {
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if current > SCHEMA_VERSION {
            bail!(
                "index schema version {current} is newer than supported {SCHEMA_VERSION}; upgrade Crossthreads"
            );
        }
        Ok(Self { conn })
    }

    /// Persist a conversation and its messages, skipping if an identical one
    /// (by content hash) is already stored.
    pub fn upsert_conversation(&mut self, convo: &Conversation) -> Result<Upsert> {
        let exists: bool = self.conn.query_row(
            "SELECT 1 FROM conversations WHERE content_hash = ?1",
            [&convo.content_hash],
            |_| Ok(true),
        ).optional_bool()?;
        if exists {
            return Ok(Upsert::Duplicate);
        }

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO conversations (
                id, tool, project, model, started_at, ended_at,
                git_branch, git_commit, source_path, source_offset,
                source_fingerprint, content_hash, title, message_count, indexed_at
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                convo.id,
                convo.tool.slug(),
                convo.project,
                convo.model,
                convo.started_at.map(|t| t.to_rfc3339()),
                convo.ended_at.map(|t| t.to_rfc3339()),
                convo.git_context.branch,
                convo.git_context.commit,
                convo.source.path,
                convo.source.offset,
                convo.source.fingerprint,
                convo.content_hash,
                convo.derived_title(),
                convo.messages.len() as i64,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO messages (conversation_id, seq, role, content, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (seq, m) in convo.messages.iter().enumerate() {
                stmt.execute(params![
                    convo.id,
                    seq as i64,
                    role_str(m.role),
                    m.content,
                    m.timestamp.map(|t| t.to_rfc3339()),
                ])?;
            }
        }

        tx.commit()?;
        Ok(Upsert::Inserted)
    }

    /// Number of stored conversations.
    pub fn conversation_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM conversations", [], |r| r.get(0))?)
    }

    /// Keyword search over message content. Returns at most `limit` hits, one
    /// per conversation (best-scoring message), ranked by BM25.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let match_expr = to_fts_match(query);
        if match_expr.is_empty() {
            return Ok(Vec::new());
        }

        // Over-fetch so collapsing per-conversation still fills `limit`.
        let fetch = (limit * 5).max(limit) as i64;
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.tool, c.project, c.title, c.started_at, c.source_path,
                    snippet(messages_fts, 0, '[', ']', '…', 12) AS snip,
                    bm25(messages_fts) AS score
             FROM messages_fts
             JOIN messages m      ON m.rowid = messages_fts.rowid
             JOIN conversations c ON c.id = m.conversation_id
             WHERE messages_fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![match_expr, fetch], |row| {
            Ok(SearchHit {
                conversation_id: row.get(0)?,
                tool: row.get(1)?,
                project: row.get(2)?,
                title: row.get(3)?,
                started_at: row.get(4)?,
                source_path: row.get(5)?,
                snippet: row.get(6)?,
                score: row.get(7)?,
            })
        })?;

        let mut seen = std::collections::HashSet::new();
        let mut hits = Vec::new();
        for hit in rows {
            let hit = hit?;
            if seen.insert(hit.conversation_id.clone()) {
                hits.push(hit);
                if hits.len() >= limit {
                    break;
                }
            }
        }
        Ok(hits)
    }
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        Role::System => "system",
    }
}

/// Turn a free-text query into a safe FTS5 MATCH expression: extract word
/// tokens, quote each (so punctuation can't break FTS syntax), AND them.
fn to_fts_match(query: &str) -> String {
    let tokens: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t.to_lowercase()))
        .collect();
    tokens.join(" ")
}

/// Small helper so an optional `SELECT 1` reads cleanly as a bool.
trait OptionalBool {
    fn optional_bool(self) -> Result<bool>;
}
impl OptionalBool for rusqlite::Result<bool> {
    fn optional_bool(self) -> Result<bool> {
        match self {
            Ok(b) => Ok(b),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_match_quotes_and_ands_tokens() {
        assert_eq!(to_fts_match("auth retry!"), "\"auth\" \"retry\"");
        assert_eq!(to_fts_match("  "), "");
    }
}
