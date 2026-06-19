//! `ct-store` — the single SQLite index and FTS5 keyword search.
//!
//! Persists normalized [`Conversation`]s with content-hash deduplication
//! (FR-ING-06) and answers lexical queries via FTS5/BM25 (FR-SRCH-01, lexical
//! half). Semantic search (`sqlite-vec`) layers onto this same DB next.

use std::path::Path;

use anyhow::{bail, Context, Result};
use ct_core::model::{Conversation, Role};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

        // Persist/verify the schema version in the user_version pragma. The
        // schema is additive (CREATE … IF NOT EXISTS), so older DBs upgrade in
        // place; a newer-than-supported DB is refused.
        let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if current > SCHEMA_VERSION {
            bail!(
                "index schema version {current} is newer than supported {SCHEMA_VERSION}; upgrade Crossthreads"
            );
        }
        if current < SCHEMA_VERSION {
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
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

    // ---- Embeddings -------------------------------------------------------

    /// Messages that have no embedding yet, oldest first, capped at `batch`.
    /// The caller embeds them and feeds the vectors back via [`Store::store_embeddings`].
    pub fn pending_embeddings(&self, batch: usize) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.rowid, m.content
             FROM messages m
             LEFT JOIN embeddings e ON e.message_rowid = m.rowid
             WHERE e.message_rowid IS NULL
             ORDER BY m.rowid
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([batch as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Persist embeddings for the given message rowids.
    pub fn store_embeddings(&mut self, model: &str, rows: &[(i64, Vec<f32>)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO embeddings (message_rowid, model, dim, vec)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (rowid, vec) in rows {
                stmt.execute(params![rowid, model, vec.len() as i64, f32s_to_bytes(vec)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Number of stored embeddings.
    pub fn embedding_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))?)
    }

    /// Semantic search: brute-force cosine over stored vectors, collapsed to the
    /// best message per conversation. `query` is the query embedding.
    pub fn search_semantic(&self, query: &[f32], limit: usize) -> Result<Vec<SearchHit>> {
        let mut scored = self.vector_scored(query)?;
        // Highest cosine first.
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for (score, mut hit) in scored {
            if seen.insert(hit.conversation_id.clone()) {
                hit.score = score as f64;
                out.push(hit);
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Hybrid search: fuse lexical (FTS/BM25) and semantic (cosine) rankings
    /// with Reciprocal Rank Fusion (FR-SRCH-01). `query` is the text, `query_vec`
    /// its embedding. Returns up to `limit` hits ordered by fused score (higher
    /// is better), preferring the lexical snippet when available.
    pub fn search_hybrid(
        &self,
        query: &str,
        query_vec: &[f32],
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let depth = (limit * 5).max(20);
        let lexical = self.search(query, depth)?;
        let semantic = self.search_semantic(query_vec, depth)?;

        // Map conversation id -> best representative hit (lexical wins for its
        // highlighted snippet).
        let mut rep: std::collections::HashMap<String, SearchHit> = std::collections::HashMap::new();
        for h in &semantic {
            rep.entry(h.conversation_id.clone()).or_insert_with(|| h.clone());
        }
        for h in &lexical {
            rep.insert(h.conversation_id.clone(), h.clone());
        }

        let lex_ids: Vec<String> = lexical.iter().map(|h| h.conversation_id.clone()).collect();
        let sem_ids: Vec<String> = semantic.iter().map(|h| h.conversation_id.clone()).collect();
        let fused = rrf_fuse(&[lex_ids, sem_ids], 60.0);

        let mut out = Vec::new();
        for (id, score) in fused.into_iter().take(limit) {
            if let Some(mut hit) = rep.remove(&id) {
                hit.score = score;
                out.push(hit);
            }
        }
        Ok(out)
    }

    /// Score every stored vector against `query`, returning (cosine, hit) with
    /// per-message snippets. Internal helper for semantic/hybrid search.
    fn vector_scored(&self, query: &[f32]) -> Result<Vec<(f32, SearchHit)>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.conversation_id, m.content, e.vec,
                    c.tool, c.project, c.title, c.started_at, c.source_path
             FROM embeddings e
             JOIN messages m      ON m.rowid = e.message_rowid
             JOIN conversations c ON c.id = m.conversation_id",
        )?;
        let rows = stmt.query_map([], |row| {
            let content: String = row.get(1)?;
            let bytes: Vec<u8> = row.get(2)?;
            Ok((
                bytes_to_f32s(&bytes),
                SearchHit {
                    conversation_id: row.get(0)?,
                    tool: row.get(3)?,
                    project: row.get(4)?,
                    title: row.get(5)?,
                    started_at: row.get(6)?,
                    source_path: row.get(7)?,
                    snippet: snippet_of(&content, 160),
                    score: 0.0,
                },
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (vec, hit) = row?;
            out.push((cosine(query, &vec), hit));
        }
        Ok(out)
    }
}

/// Reciprocal Rank Fusion: combine several ranked id-lists into one ordering.
/// score(id) = Σ 1/(k + rank), rank 0-based. Higher is better.
fn rrf_fuse(lists: &[Vec<String>], k: f64) -> Vec<(String, f64)> {
    use std::collections::HashMap;
    let mut scores: HashMap<String, f64> = HashMap::new();
    for list in lists {
        for (rank, id) in list.iter().enumerate() {
            *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k + rank as f64);
        }
    }
    let mut fused: Vec<(String, f64)> = scores.into_iter().collect();
    fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    fused
}

/// Cosine similarity; 0 for a zero vector or length mismatch.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

fn f32s_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

fn bytes_to_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// A plain (un-highlighted) snippet: first `max` chars of the content, single-lined.
fn snippet_of(content: &str, max: usize) -> String {
    let flat = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let mut s: String = flat.chars().take(max).collect();
        s.push('…');
        s
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
