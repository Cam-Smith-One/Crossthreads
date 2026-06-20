//! `ct-store` — the single SQLite index and FTS5 keyword search.
//!
//! Persists normalized [`Conversation`]s with content-hash deduplication
//! (FR-ING-06) and answers lexical queries via FTS5/BM25 (FR-SRCH-01, lexical
//! half). Semantic search (`sqlite-vec`) layers onto this same DB next.

use std::path::Path;

use anyhow::{bail, Context, Result};
use ct_core::model::{Conversation, Role};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

mod schema;

pub use schema::SCHEMA_VERSION;

/// Minimum cosine similarity for a vector to count as a semantic match.
/// Tuned to filter near-zero matches for both the hash and ONNX embedders
/// while keeping genuinely related sentences (which score well above it).
pub const MIN_SIMILARITY: f32 = 0.15;

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
    /// `thread` or `skill`.
    #[serde(default)]
    pub kind: String,
    pub project: Option<String>,
    pub title: Option<String>,
    pub started_at: Option<String>,
    /// FTS5 snippet with matched terms wrapped in `[` … `]`.
    pub snippet: String,
    /// BM25 score (lower is a better match; we expose it as-is).
    pub score: f64,
    pub source_path: String,
    /// User-set: saved to the bookmarks list.
    #[serde(default)]
    pub bookmarked: bool,
    /// User-set: pinned to the top.
    #[serde(default)]
    pub pinned: bool,
}

/// One message of a stored conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
}

/// A full conversation fetched from the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredConversation {
    pub id: String,
    pub tool: String,
    #[serde(default)]
    pub kind: String,
    pub project: Option<String>,
    pub title: Option<String>,
    pub started_at: Option<String>,
    /// Absolute path to the source file the conversation was parsed from,
    /// for "open original" / reveal-in-folder (FR-ACT-01).
    #[serde(default)]
    pub source_path: String,
    #[serde(default)]
    pub bookmarked: bool,
    #[serde(default)]
    pub pinned: bool,
    pub messages: Vec<StoredMessage>,
}

/// A paste-ready context block built from one or more conversations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBlock {
    pub markdown: String,
    /// Conversation ids included, in order.
    pub sources: Vec<String>,
    pub chars: usize,
    pub token_estimate: usize,
}

/// Optional result filters (FR-SRCH-02). Empty fields impose no constraint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Filters {
    /// Exact tool slug, e.g. `claude-code`.
    pub tool: Option<String>,
    /// Record kind: `thread` or `skill`.
    pub kind: Option<String>,
    /// Substring match against the project path.
    pub project: Option<String>,
    /// Inclusive lower bound on the start date (ISO `YYYY-MM-DD`).
    pub since: Option<String>,
    /// Inclusive upper bound on the start date (ISO `YYYY-MM-DD`).
    pub until: Option<String>,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        self.tool.is_none()
            && self.kind.is_none()
            && self.project.is_none()
            && self.since.is_none()
            && self.until.is_none()
    }

    /// Does a hit satisfy every set constraint?
    fn passes(&self, hit: &SearchHit) -> bool {
        if let Some(t) = &self.tool {
            if &hit.tool != t {
                return false;
            }
        }
        if let Some(k) = &self.kind {
            if &hit.kind != k {
                return false;
            }
        }
        if let Some(p) = &self.project {
            if !hit.project.as_deref().unwrap_or("").contains(p.as_str()) {
                return false;
            }
        }
        if self.since.is_some() || self.until.is_some() {
            // Compare on the date prefix so a date-only bound is day-inclusive.
            let date = hit.started_at.as_deref().map(|s| &s[..s.len().min(10)]);
            let Some(date) = date else { return false };
            if let Some(since) = &self.since {
                if date < since.as_str() {
                    return false;
                }
            }
            if let Some(until) = &self.until {
                if date > until.as_str() {
                    return false;
                }
            }
        }
        true
    }
}

/// Handle to the index database.
pub struct Store {
    conn: Connection,
    /// Monotonic counter bumped on every write that can affect embeddings, so
    /// the in-memory vector cache knows when it's stale.
    write_gen: std::cell::Cell<u64>,
    /// L2-normalized vectors held in memory for fast semantic search. Rebuilt
    /// lazily from the `embeddings` table when `write_gen` advances, turning the
    /// per-query cosine scan into a cache-friendly dot product (no DB I/O, no
    /// per-vector sqrt). Interior-mutable so `&self` search can refresh it.
    vec_cache: std::cell::RefCell<VecCache>,
}

/// In-memory, normalized vectors plus the `write_gen` they were built at.
#[derive(Default)]
struct VecCache {
    gen: u64,
    built: bool,
    entries: Vec<CachedVec>,
}

struct CachedVec {
    rowid: i64,
    conversation_id: String,
    /// L2-normalized embedding; cosine against another unit vector is its dot.
    norm: Vec<f32>,
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

        // Forward migration for v2 DBs that predate the `kind` column. Ignored
        // (errors) when the column already exists on a fresh schema.
        let _ = conn.execute(
            "ALTER TABLE conversations ADD COLUMN kind TEXT NOT NULL DEFAULT 'thread'",
            [],
        );
        // v3 -> v4: user-set bookmark/pin flags. Ignored when already present.
        let _ = conn.execute(
            "ALTER TABLE conversations ADD COLUMN bookmarked INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE conversations ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0",
            [],
        );
        // v4 -> v5: stable session key for durable user state.
        let _ = conn.execute(
            "ALTER TABLE conversations ADD COLUMN session_key TEXT NOT NULL DEFAULT ''",
            [],
        );

        // Indexes on migration-added columns, created only now that the ALTERs
        // above guarantee the columns exist (on a fresh DB they came from
        // CREATE TABLE; on an upgrade, from the ALTERs).
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_conversations_kind    ON conversations(kind);
             CREATE INDEX IF NOT EXISTS idx_conversations_session ON conversations(session_key);",
        )
        .context("creating migration-column indexes")?;

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

        let store = Self {
            conn,
            write_gen: std::cell::Cell::new(1),
            vec_cache: std::cell::RefCell::new(VecCache::default()),
        };
        // Backfill session keys for rows indexed before v5, carrying any legacy
        // bookmark/pin flags into the durable `user_state` table.
        store.backfill_session_keys()?;
        Ok(store)
    }

    /// Compute `session_key` for any pre-v5 conversation rows that lack one, and
    /// migrate their legacy `bookmarked`/`pinned` columns into `user_state`.
    /// Cheap no-op once everything has a key.
    fn backfill_session_keys(&self) -> Result<()> {
        let rows: Vec<(String, String, String, i64, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, tool, source_path, bookmarked, pinned
                 FROM conversations WHERE session_key = ''",
            )?;
            let collected = stmt
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            collected
        };
        if rows.is_empty() {
            return Ok(());
        }
        for (id, tool, source_path, bookmarked, pinned) in rows {
            let seed: String = self
                .conn
                .query_row(
                    "SELECT content FROM messages WHERE conversation_id = ?1 ORDER BY seq LIMIT 1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap_or_default();
            let key =
                ct_core::hash::session_key(&ct_core::model::Tool::Other(tool), &source_path, &seed);
            self.conn.execute(
                "UPDATE conversations SET session_key = ?1 WHERE id = ?2",
                params![key, id],
            )?;
            if bookmarked != 0 || pinned != 0 {
                self.conn.execute(
                    "INSERT INTO user_state (session_key, bookmarked, pinned, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(session_key) DO UPDATE SET
                        bookmarked = max(bookmarked, excluded.bookmarked),
                        pinned     = max(pinned, excluded.pinned)",
                    params![key, bookmarked, pinned, chrono::Utc::now().to_rfc3339()],
                )?;
            }
        }
        Ok(())
    }

    /// Mark the vector cache stale after a write that touched embeddings.
    fn bump_write_gen(&self) {
        self.write_gen.set(self.write_gen.get().wrapping_add(1));
    }

    /// Persist a conversation and its messages, skipping if an identical one
    /// (by content hash) is already stored.
    pub fn upsert_conversation(&mut self, convo: &Conversation) -> Result<Upsert> {
        let exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM conversations WHERE content_hash = ?1",
                [&convo.content_hash],
                |_| Ok(true),
            )
            .optional_bool()?;
        if exists {
            return Ok(Upsert::Duplicate);
        }

        // Stable key for durable user state: tool + source + first message.
        let seed = convo
            .messages
            .first()
            .map(|m| m.content.as_str())
            .unwrap_or("");
        let session_key = ct_core::hash::session_key(&convo.tool, &convo.source.path, seed);

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO conversations (
                id, tool, kind, project, model, started_at, ended_at,
                git_branch, git_commit, source_path, source_offset,
                source_fingerprint, content_hash, title, message_count, indexed_at,
                session_key
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            params![
                convo.id,
                convo.tool.slug(),
                convo.kind.slug(),
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
                session_key,
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
        self.bump_write_gen();
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
            "SELECT c.id, c.tool, c.kind, c.project, c.title, c.started_at, c.source_path,
                    COALESCE(us.bookmarked, 0), COALESCE(us.pinned, 0),
                    snippet(messages_fts, 0, '[', ']', '…', 12) AS snip,
                    bm25(messages_fts) AS score
             FROM messages_fts
             JOIN messages m      ON m.rowid = messages_fts.rowid
             JOIN conversations c ON c.id = m.conversation_id
             LEFT JOIN user_state us ON us.session_key = c.session_key
             WHERE messages_fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![match_expr, fetch], |row| {
            Ok(SearchHit {
                conversation_id: row.get(0)?,
                tool: row.get(1)?,
                kind: row.get(2)?,
                project: row.get(3)?,
                title: row.get(4)?,
                started_at: row.get(5)?,
                source_path: row.get(6)?,
                bookmarked: row.get::<_, i64>(7)? != 0,
                pinned: row.get::<_, i64>(8)? != 0,
                snippet: row.get(9)?,
                score: row.get(10)?,
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
        self.bump_write_gen();
        Ok(())
    }

    /// Number of stored embeddings.
    pub fn embedding_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))?)
    }

    /// Semantic search over stored vectors, collapsed to the best message per
    /// conversation. `query` is the query embedding.
    ///
    /// Fast path: vectors are kept L2-normalized in memory (rebuilt only when
    /// the index changes), so this scores each candidate with a single dot
    /// product — no per-query DB read and no per-vector sqrt. Only the top
    /// `limit` messages are then hydrated into full [`SearchHit`]s.
    pub fn search_semantic(&self, query: &[f32], limit: usize) -> Result<Vec<SearchHit>> {
        self.refresh_vec_cache()?;
        let qn = normalized(query);

        // Score in memory, collapse to the best message per conversation, keep
        // the rowids of the survivors in rank order.
        let cache = self.vec_cache.borrow();
        let mut scored: Vec<(f32, i64, &str)> = cache
            .entries
            .iter()
            .map(|e| (dot(&qn, &e.norm), e.rowid, e.conversation_id.as_str()))
            .filter(|(s, ..)| *s >= MIN_SIMILARITY)
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut seen = std::collections::HashSet::new();
        let mut top: Vec<(i64, f32)> = Vec::with_capacity(limit);
        for (score, rowid, conv) in scored {
            if seen.insert(conv.to_string()) {
                top.push((rowid, score));
                if top.len() >= limit {
                    break;
                }
            }
        }
        drop(cache);

        // Hydrate only the survivors into full hits, preserving rank order.
        self.hits_for_rowids(&top)
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
        let mut rep: std::collections::HashMap<String, SearchHit> =
            std::collections::HashMap::new();
        for h in &semantic {
            rep.entry(h.conversation_id.clone())
                .or_insert_with(|| h.clone());
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

    // ---- Filtered search (FR-SRCH-02) -------------------------------------
    //
    // Filters are applied by over-fetching an unfiltered candidate pool and
    // keeping those that pass — simple and correct for the target corpus.

    /// Over-fetch size for a target `limit` when filtering.
    fn pool(limit: usize) -> usize {
        (limit * 10).max(50)
    }

    pub fn search_filtered(
        &self,
        query: &str,
        limit: usize,
        f: &Filters,
    ) -> Result<Vec<SearchHit>> {
        if f.is_empty() {
            return self.search(query, limit);
        }
        Ok(self
            .search(query, Self::pool(limit))?
            .into_iter()
            .filter(|h| f.passes(h))
            .take(limit)
            .collect())
    }

    pub fn search_semantic_filtered(
        &self,
        query: &[f32],
        limit: usize,
        f: &Filters,
    ) -> Result<Vec<SearchHit>> {
        if f.is_empty() {
            return self.search_semantic(query, limit);
        }
        Ok(self
            .search_semantic(query, Self::pool(limit))?
            .into_iter()
            .filter(|h| f.passes(h))
            .take(limit)
            .collect())
    }

    pub fn search_hybrid_filtered(
        &self,
        query: &str,
        query_vec: &[f32],
        limit: usize,
        f: &Filters,
    ) -> Result<Vec<SearchHit>> {
        if f.is_empty() {
            return self.search_hybrid(query, query_vec, limit);
        }
        Ok(self
            .search_hybrid(query, query_vec, Self::pool(limit))?
            .into_iter()
            .filter(|h| f.passes(h))
            .take(limit)
            .collect())
    }

    /// Distinct tools present in the index (for filter UIs).
    pub fn facets_tools(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT tool FROM conversations ORDER BY tool")?;
        let tools = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(tools)
    }

    /// Record counts grouped by (tool, kind), most-frequent first. Lets a user
    /// see at a glance how much each tool contributed (e.g. is Codex indexing?).
    pub fn counts_by_tool(&self) -> Result<Vec<(String, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT tool, kind, COUNT(*) AS n FROM conversations
             GROUP BY tool, kind ORDER BY n DESC",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // ---- Retrieval of full conversations / context ------------------------

    /// Fetch a full stored conversation by id, with its messages in order.
    pub fn get_conversation(&self, id: &str) -> Result<Option<StoredConversation>> {
        let meta = self.conn.query_row(
            "SELECT c.tool, c.kind, c.project, c.title, c.started_at, c.source_path, \
                    COALESCE(us.bookmarked, 0), COALESCE(us.pinned, 0) \
             FROM conversations c \
             LEFT JOIN user_state us ON us.session_key = c.session_key \
             WHERE c.id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)? != 0,
                    r.get::<_, i64>(7)? != 0,
                ))
            },
        );
        let (tool, kind, project, title, started_at, source_path, bookmarked, pinned) = match meta {
            Ok(t) => t,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let mut stmt = self.conn.prepare(
            "SELECT role, content FROM messages WHERE conversation_id = ?1 ORDER BY seq",
        )?;
        let messages = stmt
            .query_map([id], |r| {
                Ok(StoredMessage {
                    role: r.get(0)?,
                    content: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(Some(StoredConversation {
            id: id.to_string(),
            tool,
            kind,
            project,
            title,
            started_at,
            source_path,
            bookmarked,
            pinned,
            messages,
        }))
    }

    // ---- Bookmarks &amp; pins (FR-SRCH-*) -------------------------------------

    /// Set or clear the bookmark / pin flag on a conversation. `None` leaves a
    /// flag unchanged. Returns `false` if no such conversation exists. The state
    /// is written to the durable `user_state` table keyed by the conversation's
    /// stable `session_key`, so it survives re-indexing and a growing session.
    pub fn set_flags(
        &self,
        id: &str,
        bookmarked: Option<bool>,
        pinned: Option<bool>,
    ) -> Result<bool> {
        let key: Option<String> = self
            .conn
            .query_row(
                "SELECT session_key FROM conversations WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(key) = key.filter(|k| !k.is_empty()) else {
            return Ok(false);
        };

        // Upsert, leaving the unspecified flag at its current value.
        self.conn.execute(
            "INSERT INTO user_state (session_key, bookmarked, pinned, updated_at)
             VALUES (?1, COALESCE(?2, 0), COALESCE(?3, 0), ?4)
             ON CONFLICT(session_key) DO UPDATE SET
                bookmarked = COALESCE(?2, bookmarked),
                pinned     = COALESCE(?3, pinned),
                updated_at = ?4",
            params![
                key,
                bookmarked.map(|b| b as i64),
                pinned.map(|p| p as i64),
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(true)
    }

    /// Saved conversations — pinned first, then bookmarked — newest first within
    /// each group, one row per stable session. Powers the "Saved &amp; pinned" panel.
    pub fn saved(&self) -> Result<Vec<SearchHit>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.tool, c.kind, c.project, c.title, c.started_at, c.source_path,
                    us.bookmarked, us.pinned,
                    MAX(c.indexed_at) AS latest
             FROM user_state us
             JOIN conversations c ON c.session_key = us.session_key
             WHERE us.bookmarked = 1 OR us.pinned = 1
             GROUP BY us.session_key
             ORDER BY us.pinned DESC, c.started_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SearchHit {
                conversation_id: row.get(0)?,
                tool: row.get(1)?,
                kind: row.get(2)?,
                project: row.get(3)?,
                title: row.get(4)?,
                started_at: row.get(5)?,
                source_path: row.get(6)?,
                bookmarked: row.get::<_, i64>(7)? != 0,
                pinned: row.get::<_, i64>(8)? != 0,
                snippet: String::new(),
                score: 0.0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Render a paste-ready markdown context block from the given conversations,
    /// in order, stopping once `max_chars` is reached (FR-ACT-03 / AGENT_API §5).
    pub fn render_context(&self, ids: &[String], max_chars: usize) -> Result<ContextBlock> {
        let mut markdown = String::from("## Prior context (Crossthreads)\n");
        let mut sources = Vec::new();

        for id in ids {
            let Some(convo) = self.get_conversation(id)? else {
                continue;
            };
            let mut section = String::new();
            let when = convo
                .started_at
                .as_deref()
                .and_then(|s| s.get(..10))
                .unwrap_or("");
            section.push_str(&format!(
                "\n### {} — {}{}\n",
                convo.title.as_deref().unwrap_or("(untitled)"),
                convo.tool,
                if when.is_empty() {
                    String::new()
                } else {
                    format!("  [{when}]")
                },
            ));
            if let Some(p) = &convo.project {
                section.push_str(&format!("_{p}_\n\n"));
            }
            // Add messages up to the remaining budget so neither a long
            // conversation nor a single huge message can blow past max_chars.
            let budget = max_chars.saturating_sub(markdown.len());
            let mut truncated = false;
            for m in &convo.messages {
                let prefix = format!("**{}:** ", m.role);
                let avail = budget.saturating_sub(section.len());
                // Room needed for the prefix, the "\n\n", and an ellipsis.
                if avail <= prefix.len() + 8 {
                    truncated = true;
                    break;
                }
                let body = m.content.trim();
                let turn = format!("{prefix}{body}\n\n");
                if turn.len() <= avail {
                    section.push_str(&turn);
                } else {
                    // The message alone overflows the budget — include a head of it.
                    let keep = avail - prefix.len() - 6;
                    section.push_str(&prefix);
                    section.push_str(truncate_on_boundary(body, keep));
                    section.push_str(" …\n\n");
                    truncated = true;
                    break;
                }
            }
            if truncated {
                section.push_str("_…(truncated)_\n");
            }

            // Stop before exceeding the budget; always include at least one.
            if !sources.is_empty() && markdown.len() + section.len() > max_chars {
                break;
            }
            markdown.push_str(&section);
            sources.push(id.clone());
            if markdown.len() >= max_chars {
                break;
            }
        }

        let chars = markdown.chars().count();
        Ok(ContextBlock {
            markdown,
            sources,
            chars,
            token_estimate: chars / 4, // rough heuristic
        })
    }

    /// Rebuild the in-memory normalized-vector cache if a write has advanced
    /// `write_gen` since it was last built. Cheap no-op on the hot path.
    fn refresh_vec_cache(&self) -> Result<()> {
        let gen = self.write_gen.get();
        if self.vec_cache.borrow().built && self.vec_cache.borrow().gen == gen {
            return Ok(());
        }
        let mut stmt = self.conn.prepare(
            "SELECT e.message_rowid, m.conversation_id, e.vec
             FROM embeddings e
             JOIN messages m ON m.rowid = e.message_rowid",
        )?;
        let entries = stmt
            .query_map([], |row| {
                let rowid: i64 = row.get(0)?;
                let conversation_id: String = row.get(1)?;
                let bytes: Vec<u8> = row.get(2)?;
                let mut norm = bytes_to_f32s(&bytes);
                normalize_in_place(&mut norm);
                Ok(CachedVec {
                    rowid,
                    conversation_id,
                    norm,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        *self.vec_cache.borrow_mut() = VecCache {
            gen,
            built: true,
            entries,
        };
        Ok(())
    }

    /// Hydrate the given message rowids (with their scores) into full hits,
    /// preserving the order given. Used by semantic search after ranking.
    fn hits_for_rowids(&self, ranked: &[(i64, f32)]) -> Result<Vec<SearchHit>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.conversation_id, m.content,
                    c.tool, c.kind, c.project, c.title, c.started_at, c.source_path,
                    COALESCE(us.bookmarked, 0), COALESCE(us.pinned, 0)
             FROM messages m
             JOIN conversations c ON c.id = m.conversation_id
             LEFT JOIN user_state us ON us.session_key = c.session_key
             WHERE m.rowid = ?1",
        )?;
        let mut out = Vec::with_capacity(ranked.len());
        for (rowid, score) in ranked {
            let hit = stmt.query_row([rowid], |row| {
                let content: String = row.get(1)?;
                Ok(SearchHit {
                    conversation_id: row.get(0)?,
                    tool: row.get(2)?,
                    kind: row.get(3)?,
                    project: row.get(4)?,
                    title: row.get(5)?,
                    started_at: row.get(6)?,
                    source_path: row.get(7)?,
                    bookmarked: row.get::<_, i64>(8)? != 0,
                    pinned: row.get::<_, i64>(9)? != 0,
                    snippet: snippet_of(&content, 160),
                    score: *score as f64,
                })
            });
            match hit {
                Ok(h) => out.push(h),
                Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                Err(e) => return Err(e.into()),
            }
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

/// Dot product of two equal-length vectors (0 on length mismatch). For two
/// L2-normalized vectors this equals their cosine similarity.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Scale a vector to unit length in place; leaves a zero vector untouched.
fn normalize_in_place(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Return an L2-normalized copy of `v`.
fn normalized(v: &[f32]) -> Vec<f32> {
    let mut out = v.to_vec();
    normalize_in_place(&mut out);
    out
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

/// Borrow at most `max` bytes of `s`, backing up to the nearest char boundary
/// so the slice is always valid UTF-8.
fn truncate_on_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
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
