//! `ct-store` — the single SQLite index and FTS5 keyword search.
//!
//! Persists normalized [`Conversation`]s with content-hash deduplication
//! (FR-ING-06) and answers lexical queries via FTS5/BM25 (FR-SRCH-01, lexical
//! half). Semantic search (`sqlite-vec`) layers onto this same DB next.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ct_core::model::{Conversation, Role};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};

mod schema;
pub mod themes;

pub use schema::SCHEMA_VERSION;
pub use themes::{Theme, ThemeSample};

/// Minimum cosine similarity for a vector to count as a semantic match.
/// Tuned to filter near-zero matches for both the hash and ONNX embedders
/// while keeping genuinely related sentences (which score well above it).
pub const MIN_SIMILARITY: f32 = 0.15;

/// Above this many cached vectors, parallelize the semantic scan across cores.
/// Below it, the per-task overhead isn't worth it.
const PARALLEL_SCAN_THRESHOLD: usize = 8_192;

/// Outcome of attempting to persist one conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upsert {
    /// Newly stored.
    Inserted,
    /// Already present (same content hash) — skipped per dedup.
    Duplicate,
    /// Skipped because the user forgot this thread (tombstoned).
    Forgotten,
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
    /// FTS5 snippet with matched terms wrapped in STX … ETX control chars
    /// (`\u{2}`/`\u{3}`) — sentinels that can't occur in real content, so the UI
    /// won't mistake literal `[brackets]` in code/logs for highlights.
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
    /// User-set tags (lowercased).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Origin device for cross-device (federated) results. `None` for local
    /// hits; set to a peer's configured name when a result came from another
    /// device. See ADR-010 / CROSS_DEVICE_SEARCH.md.
    #[serde(default)]
    pub device: Option<String>,
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
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub tags: Vec<String>,
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
    /// Exact (case-insensitive) tag membership.
    pub tag: Option<String>,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        self.tool.is_none()
            && self.kind.is_none()
            && self.project.is_none()
            && self.since.is_none()
            && self.until.is_none()
            && self.tag.is_none()
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
            // `get` (not byte-slicing) avoids a panic on non-ASCII `started_at`.
            let date = hit.started_at.as_deref().map(|s| s.get(..10).unwrap_or(s));
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
        if let Some(tag) = &self.tag {
            let want = tag.trim().to_lowercase();
            if !want.is_empty() && !hit.tags.iter().any(|t| t == &want) {
                return false;
            }
        }
        true
    }
}

/// Handle to the index database.
pub struct Store {
    conn: Connection,
    /// Shared, thread-safe vector cache. The writer and every read-pool
    /// connection point at the same `VectorCache`, so the normalized vectors are
    /// built once and reused rather than per-connection (ADR-005 read pool).
    cache: Arc<VectorCache>,
}

/// A recent conversation with its (truncated) text, for LLM-synthesis insights.
/// Produced by [`Store::recent_conversations`].
#[derive(Debug, Clone)]
pub struct RecentConversation {
    pub id: String,
    pub title: Option<String>,
    pub tool: String,
    /// Record kind: `thread` (a conversation) or `skill` (a reusable skill/prompt).
    pub kind: String,
    pub project: Option<String>,
    pub started_at: Option<String>,
    /// Concatenated `role: content` message text, truncated.
    pub text: String,
}

/// Activity in one time bucket (a day or an ISO week), for the temporal view.
/// Produced by [`Store::activity`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityBucket {
    /// `YYYY-MM-DD` (day) or `YYYY-Www` (week).
    pub period: String,
    /// Total sessions in this bucket.
    pub total: i64,
    /// Per-tool counts, busiest tool first.
    pub by_tool: Vec<(String, i64)>,
}

/// A node in the knowledge [`Graph`]: a project, tool, or tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    /// Stable id, namespaced by kind: `project:…` | `tool:…` | `tag:…`.
    pub id: String,
    /// Display label.
    pub label: String,
    /// `project` | `tool` | `tag`.
    pub kind: String,
    /// How many sessions this node covers (node size).
    pub weight: i64,
}

/// A weighted, undirected co-occurrence edge between two [`GraphNode`]s.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Source node id.
    pub source: String,
    /// Target node id.
    pub target: String,
    /// Number of sessions where the two co-occur.
    pub weight: i64,
}

/// A knowledge graph of the user's work: projects/tools/tags as nodes, with
/// co-occurrence edges. Produced by [`Store::graph`]. Deterministic; no model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// A conversation reduced to one centroid embedding plus light metadata, for
/// clustering / theme extraction. Produced by [`Store::conversation_vectors`].
#[derive(Debug, Clone)]
pub struct ConversationVec {
    pub id: String,
    pub title: Option<String>,
    pub tool: String,
    pub project: Option<String>,
    /// L2-normalized centroid; cosine against another unit vector is the dot.
    pub vec: Vec<f32>,
}

/// L2-normalized embedding vectors held in memory for fast semantic search,
/// shared across a [`Store`] writer and its read-pool connections.
pub struct VectorCache {
    /// Bumped on every write that can affect embeddings; the built snapshot is
    /// stale when its `gen` lags this.
    generation: AtomicU64,
    built: Mutex<Built>,
}

impl Default for VectorCache {
    fn default() -> Self {
        Self {
            // Start at 1 so the empty (gen 0) snapshot is stale and built lazily.
            generation: AtomicU64::new(1),
            built: Mutex::new(Built::default()),
        }
    }
}

/// The built vector snapshot plus the `generation` it was built at. `entries`
/// is an `Arc` so a search can clone it and release the lock before the scan.
#[derive(Default)]
struct Built {
    gen: u64,
    ready: bool,
    entries: Arc<Vec<CachedVec>>,
}

struct CachedVec {
    rowid: i64,
    conversation_id: String,
    /// L2-normalized embedding; cosine against another unit vector is its dot.
    norm: Vec<f32>,
}

impl Store {
    /// Open (creating if needed) the index at `path`, applying the schema. This
    /// is the single **writer**; pair it with [`Store::open_read`] for a read
    /// pool. WAL mode lets readers and the writer run concurrently.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path.as_ref())
            .with_context(|| format!("opening index at {}", path.as_ref().display()))?;
        // WAL: concurrent readers don't block the writer (and vice-versa);
        // NORMAL sync is the standard durable-enough WAL pairing; busy_timeout
        // makes a brief lock wait rather than error.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        Self::init(conn)
    }

    /// Open an additional **read-only** connection to an already-initialized
    /// index at `path`, sharing `cache` with the writer (from
    /// [`Store::cache_handle`]). Used to build a read pool for true search
    /// concurrency; never runs migrations.
    pub fn open_read(path: impl AsRef<Path>, cache: Arc<VectorCache>) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path.as_ref(),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening read connection at {}", path.as_ref().display()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        Ok(Self { conn, cache })
    }

    /// The shared vector cache, to hand to [`Store::open_read`] connections.
    pub fn cache_handle(&self) -> Arc<VectorCache> {
        self.cache.clone()
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
        // v6 -> v7: notes and tags on user_state.
        let _ = conn.execute(
            "ALTER TABLE user_state ADD COLUMN note TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE user_state ADD COLUMN tags TEXT NOT NULL DEFAULT ''",
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
            cache: Arc::new(VectorCache::default()),
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
        self.cache.generation.fetch_add(1, Ordering::Release);
    }

    /// Persist a conversation and its messages, skipping if an identical one
    /// (by content hash) is already stored.
    pub fn upsert_conversation(&mut self, convo: &Conversation) -> Result<Upsert> {
        // Stable key for durable user state: tool + source + first message.
        let seed = convo
            .messages
            .first()
            .map(|m| m.content.as_str())
            .unwrap_or("");
        let session_key = ct_core::hash::session_key(&convo.tool, &convo.source.path, seed);

        // A conversation the user has forgotten stays forgotten across
        // re-indexing, even though its source file still exists.
        let forgotten: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM forgotten WHERE session_key = ?1",
                [&session_key],
                |_| Ok(true),
            )
            .optional_bool()?;
        if forgotten {
            return Ok(Upsert::Forgotten);
        }

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
                    COALESCE(us.bookmarked, 0), COALESCE(us.pinned, 0), COALESCE(us.tags, ''),
                    snippet(messages_fts, 0, '\u{2}', '\u{3}', '…', 12) AS snip,
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
                tags: split_tags(&row.get::<_, String>(9)?),
                snippet: row.get(10)?,
                score: row.get(11)?,
                device: None,
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

    /// One L2-normalized centroid vector per conversation (the mean of its
    /// message embeddings), plus light metadata. Feeds clustering / theme
    /// extraction (ADR-pending). Conversations with no embeddings are skipped.
    pub fn conversation_vectors(&self) -> Result<Vec<ConversationVec>> {
        use std::collections::HashMap;
        let entries = self.vectors()?;
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        let dim = entries[0].norm.len();
        // Sum message vectors per conversation, then average + re-normalize.
        let mut sums: HashMap<String, (Vec<f32>, usize)> = HashMap::new();
        for e in entries.iter() {
            let (sum, n) = sums
                .entry(e.conversation_id.clone())
                .or_insert_with(|| (vec![0.0; dim], 0));
            for (s, v) in sum.iter_mut().zip(&e.norm) {
                *s += *v;
            }
            *n += 1;
        }
        // Pull metadata for every conversation in one pass.
        let mut meta: HashMap<String, (Option<String>, String, Option<String>)> = HashMap::new();
        let mut stmt = self
            .conn
            .prepare("SELECT id, title, tool, project FROM conversations")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (id, title, tool, project) = row?;
            meta.insert(id, (title, tool, project));
        }

        let mut out = Vec::with_capacity(sums.len());
        for (id, (mut centroid, n)) in sums {
            let inv = 1.0 / n as f32;
            for v in centroid.iter_mut() {
                *v *= inv;
            }
            normalize_in_place(&mut centroid);
            let (title, tool, project) = meta.remove(&id).unwrap_or((None, String::new(), None));
            out.push(ConversationVec {
                id,
                title,
                tool,
                project,
                vec: centroid,
            });
        }
        Ok(out)
    }

    /// Cluster the indexed conversations into at most `k` themes, each carrying
    /// up to `max_samples` sample conversations. See [`themes::cluster`].
    pub fn themes(&self, k: usize, max_samples: usize) -> Result<Vec<Theme>> {
        let convos = self.conversation_vectors()?;
        Ok(themes::cluster(&convos, k, max_samples))
    }

    /// The most recent `limit` conversations (threads), newest first, each with
    /// its message text concatenated and truncated to `max_chars`. Feeds the
    /// LLM-synthesis insights (open loops, knowledge cards, …).
    pub fn recent_conversations(
        &self,
        limit: usize,
        max_chars: usize,
    ) -> Result<Vec<RecentConversation>> {
        // Span every record kind (threads *and* skills/prompts) and every tool —
        // insights should reflect all of the user's work, not just chat threads.
        // Recent dated threads sort first; undated records (e.g. skills) follow.
        let mut stmt = self.conn.prepare(
            "SELECT id, title, tool, kind, project, started_at FROM conversations
             ORDER BY started_at IS NULL, started_at DESC
             LIMIT ?1",
        )?;
        let metas = stmt
            .query_map([limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut out = Vec::with_capacity(metas.len());
        for (id, title, tool, kind, project, started_at) in metas {
            let mut msg_stmt = self.conn.prepare(
                "SELECT role, content FROM messages WHERE conversation_id = ?1 ORDER BY seq",
            )?;
            let rows = msg_stmt.query_map([&id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            let mut text = String::new();
            for row in rows {
                let (role, content) = row?;
                if text.len() >= max_chars {
                    break;
                }
                let remaining = max_chars.saturating_sub(text.len());
                let snippet: String = content.chars().take(remaining).collect();
                text.push_str(&role);
                text.push_str(": ");
                text.push_str(snippet.trim());
                text.push('\n');
            }
            out.push(RecentConversation {
                id,
                title,
                tool,
                kind,
                project,
                started_at,
                text,
            });
        }
        Ok(out)
    }

    /// Session activity over time, bucketed by `day` (default) or `week`, with a
    /// per-tool breakdown — for the temporal view. Honors the simple `Filters`
    /// (tool, kind, project substring, since/until). Oldest bucket first.
    pub fn activity(&self, bucket: &str, f: &Filters) -> Result<Vec<ActivityBucket>> {
        let by_week = bucket.eq_ignore_ascii_case("week");
        // Compute both period keys in SQL (SQLite parses ISO-8601 `started_at`),
        // so we avoid date math in Rust; filter + pick in the loop.
        let mut stmt = self.conn.prepare(
            "SELECT tool, project, kind,
                    substr(started_at, 1, 10)        AS day,
                    strftime('%Y-W%W', started_at)   AS week
             FROM conversations
             WHERE started_at IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;

        use std::collections::BTreeMap;
        let mut buckets: BTreeMap<String, (i64, BTreeMap<String, i64>)> = BTreeMap::new();
        for row in rows {
            let (tool, project, kind, day, week) = row?;
            if let Some(t) = &f.tool {
                if &tool != t {
                    continue;
                }
            }
            if let Some(k) = &f.kind {
                if &kind != k {
                    continue;
                }
            }
            if let Some(p) = &f.project {
                if !project.as_deref().unwrap_or("").contains(p.as_str()) {
                    continue;
                }
            }
            if let Some(since) = &f.since {
                if day.as_str() < since.as_str() {
                    continue;
                }
            }
            if let Some(until) = &f.until {
                if day.as_str() > until.as_str() {
                    continue;
                }
            }
            let period = if by_week {
                week.clone().unwrap_or_else(|| day.clone())
            } else {
                day.clone()
            };
            let entry = buckets.entry(period).or_default();
            entry.0 += 1;
            *entry.1.entry(tool).or_default() += 1;
        }

        Ok(buckets
            .into_iter()
            .map(|(period, (total, tools))| {
                let mut by_tool: Vec<(String, i64)> = tools.into_iter().collect();
                by_tool.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                ActivityBucket {
                    period,
                    total,
                    by_tool,
                }
            })
            .collect())
    }

    /// A deterministic knowledge graph of the user's work: project, tool, and tag
    /// nodes (sized by session count) with co-occurrence edges (project↔tool,
    /// project↔tag). `limit` caps the nodes kept per category (by weight). No
    /// model involved — pure metadata aggregation.
    pub fn graph(&self, limit: usize) -> Result<Graph> {
        use std::collections::{HashMap, HashSet};
        let mut stmt = self.conn.prepare(
            "SELECT c.tool, c.project, COALESCE(us.tags, '')
             FROM conversations c
             LEFT JOIN user_state us ON us.session_key = c.session_key
             WHERE c.kind = 'thread'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;

        let mut tool_w: HashMap<String, i64> = HashMap::new();
        let mut proj_w: HashMap<String, i64> = HashMap::new();
        let mut tag_w: HashMap<String, i64> = HashMap::new();
        let mut pt: HashMap<(String, String), i64> = HashMap::new(); // project↔tool
        let mut pg: HashMap<(String, String), i64> = HashMap::new(); // project↔tag
        for row in rows {
            let (tool, project, tags) = row?;
            *tool_w.entry(tool.clone()).or_default() += 1;
            let proj = project
                .as_deref()
                .map(project_label)
                .filter(|p| !p.is_empty());
            if let Some(proj) = &proj {
                *proj_w.entry(proj.clone()).or_default() += 1;
                *pt.entry((proj.clone(), tool.clone())).or_default() += 1;
            }
            for tag in split_tags(&tags) {
                *tag_w.entry(tag.clone()).or_default() += 1;
                if let Some(proj) = &proj {
                    *pg.entry((proj.clone(), tag)).or_default() += 1;
                }
            }
        }

        // Keep the top `limit` of each category by weight.
        let top = |m: &HashMap<String, i64>| -> HashSet<String> {
            let mut v: Vec<(&String, &i64)> = m.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            v.into_iter().take(limit).map(|(k, _)| k.clone()).collect()
        };
        let keep_tool = top(&tool_w);
        let keep_proj = top(&proj_w);
        let keep_tag = top(&tag_w);

        let mut nodes = Vec::new();
        for (k, w) in &proj_w {
            if keep_proj.contains(k) {
                nodes.push(GraphNode {
                    id: format!("project:{k}"),
                    label: k.clone(),
                    kind: "project".into(),
                    weight: *w,
                });
            }
        }
        for (k, w) in &tool_w {
            if keep_tool.contains(k) {
                nodes.push(GraphNode {
                    id: format!("tool:{k}"),
                    label: k.clone(),
                    kind: "tool".into(),
                    weight: *w,
                });
            }
        }
        for (k, w) in &tag_w {
            if keep_tag.contains(k) {
                nodes.push(GraphNode {
                    id: format!("tag:{k}"),
                    label: k.clone(),
                    kind: "tag".into(),
                    weight: *w,
                });
            }
        }
        nodes.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.id.cmp(&b.id)));

        let mut edges = Vec::new();
        for ((proj, tool), w) in &pt {
            if keep_proj.contains(proj) && keep_tool.contains(tool) {
                edges.push(GraphEdge {
                    source: format!("project:{proj}"),
                    target: format!("tool:{tool}"),
                    weight: *w,
                });
            }
        }
        for ((proj, tag), w) in &pg {
            if keep_proj.contains(proj) && keep_tag.contains(tag) {
                edges.push(GraphEdge {
                    source: format!("project:{proj}"),
                    target: format!("tag:{tag}"),
                    weight: *w,
                });
            }
        }
        edges.sort_by(|a, b| {
            b.weight
                .cmp(&a.weight)
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| a.target.cmp(&b.target))
        });

        Ok(Graph { nodes, edges })
    }

    /// Semantic search over stored vectors, collapsed to the best message per
    /// conversation. `query` is the query embedding.
    ///
    /// Fast path: vectors are kept L2-normalized in memory (rebuilt only when
    /// the index changes), so this scores each candidate with a single dot
    /// product — no per-query DB read and no per-vector sqrt. For large corpora
    /// the scan is parallelized across cores (a clean seam where an ANN index
    /// like sqlite-vec/HNSW could drop in later). Only the top `limit` messages
    /// are then hydrated into full [`SearchHit`]s.
    pub fn search_semantic(&self, query: &[f32], limit: usize) -> Result<Vec<SearchHit>> {
        // Snapshot the shared vectors (cheap Arc clone) so the cosine scan runs
        // *without* holding the cache lock — letting other readers proceed.
        let entries = self.vectors()?;
        let qn = normalized(query);

        // Score in memory, collapse to the best message per conversation, keep
        // the rowids of the survivors in rank order.
        let mut scored: Vec<(f32, i64, &str)> = if entries.len() >= PARALLEL_SCAN_THRESHOLD {
            use rayon::prelude::*;
            entries
                .par_iter()
                .map(|e| (dot(&qn, &e.norm), e.rowid, e.conversation_id.as_str()))
                .filter(|(s, ..)| *s >= MIN_SIMILARITY)
                .collect()
        } else {
            entries
                .iter()
                .map(|e| (dot(&qn, &e.norm), e.rowid, e.conversation_id.as_str()))
                .filter(|(s, ..)| *s >= MIN_SIMILARITY)
                .collect()
        };
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
                    COALESCE(us.bookmarked, 0), COALESCE(us.pinned, 0), \
                    COALESCE(us.note, ''), COALESCE(us.tags, '') \
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
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                ))
            },
        );
        let (tool, kind, project, title, started_at, source_path, bookmarked, pinned, note, tags) =
            match meta {
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
            note,
            tags: split_tags(&tags),
            messages,
        }))
    }

    /// Forget a conversation: delete it (and every other indexed version of the
    /// same session) from the index, and tombstone its `session_key` so it is
    /// not re-added on the next index pass. Returns `false` if no such id.
    ///
    /// Messages, embeddings, and FTS rows cascade away via foreign keys/triggers.
    pub fn forget(&self, id: &str) -> Result<bool> {
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
        self.conn.execute(
            "INSERT OR IGNORE INTO forgotten (session_key, forgotten_at) VALUES (?1, ?2)",
            params![key, chrono::Utc::now().to_rfc3339()],
        )?;
        self.conn
            .execute("DELETE FROM conversations WHERE session_key = ?1", [&key])?;
        self.conn
            .execute("DELETE FROM user_state WHERE session_key = ?1", [&key])?;
        self.bump_write_gen();
        Ok(true)
    }

    /// How many conversations are tombstoned (for status/diagnostics).
    pub fn forgotten_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM forgotten", [], |r| r.get(0))?)
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

    /// Resolve a conversation id to its stable session key, if it exists.
    fn session_key_of(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT session_key FROM conversations WHERE id = ?1",
                [id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .filter(|k| !k.is_empty()))
    }

    /// Attach a free-text note to a conversation (durable, by session key).
    pub fn set_note(&self, id: &str, note: &str) -> Result<bool> {
        let Some(key) = self.session_key_of(id)? else {
            return Ok(false);
        };
        self.conn.execute(
            "INSERT INTO user_state (session_key, note, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(session_key) DO UPDATE SET note = ?2, updated_at = ?3",
            params![key, note, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(true)
    }

    /// Set the tags on a conversation (normalized, de-duplicated, durable).
    pub fn set_tags(&self, id: &str, tags: &[String]) -> Result<bool> {
        let Some(key) = self.session_key_of(id)? else {
            return Ok(false);
        };
        let joined = join_tags(tags);
        self.conn.execute(
            "INSERT INTO user_state (session_key, tags, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(session_key) DO UPDATE SET tags = ?2, updated_at = ?3",
            params![key, joined, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(true)
    }

    /// All distinct tags in use, sorted (for filter UIs).
    pub fn facets_tags(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT tags FROM user_state WHERE tags <> ''")?;
        let mut set = std::collections::BTreeSet::new();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for r in rows {
            for t in split_tags(&r?) {
                set.insert(t);
            }
        }
        Ok(set.into_iter().collect())
    }

    /// Saved conversations — pinned first, then bookmarked — newest first within
    /// each group, one row per stable session. Powers the "Saved &amp; pinned" panel.
    pub fn saved(&self) -> Result<Vec<SearchHit>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.tool, c.kind, c.project, c.title, c.started_at, c.source_path,
                    us.bookmarked, us.pinned, us.tags,
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
                tags: split_tags(&row.get::<_, String>(9)?),
                snippet: String::new(),
                score: 0.0,
                device: None,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Render a paste-ready markdown context block from the given conversations,
    /// in order, stopping once `max_chars` is reached (FR-ACT-03 / AGENT_API §5).
    pub fn render_context(&self, ids: &[String], max_chars: usize) -> Result<ContextBlock> {
        let mut convos = Vec::new();
        for id in ids {
            if let Some(convo) = self.get_conversation(id)? {
                convos.push(convo);
            }
        }
        Ok(self.render_conversations(&convos, max_chars))
    }

    /// Render already-loaded conversations into a paste-ready context block.
    /// Shared by local id rendering and cross-device assembly, where remote
    /// transcripts are fetched from a peer first (ADR-010). Touches no DB state.
    pub fn render_conversations(
        &self,
        convos: &[StoredConversation],
        max_chars: usize,
    ) -> ContextBlock {
        let mut markdown = String::from("## Prior context (Crossthreads)\n");
        let mut sources = Vec::new();

        for convo in convos {
            let id = &convo.id;
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
        ContextBlock {
            markdown,
            sources,
            chars,
            token_estimate: chars / 4, // rough heuristic
        }
    }

    /// Rebuild the in-memory normalized-vector cache if a write has advanced
    /// the `generation` since it was last built. The returned `Arc` lets the
    /// caller scan without holding the cache lock. Cheap on the hot path (an
    /// `Arc` clone when the snapshot is current).
    fn vectors(&self) -> Result<Arc<Vec<CachedVec>>> {
        let gen = self.cache.generation.load(Ordering::Acquire);
        {
            let built = self.cache.built.lock().expect("vec cache poisoned");
            if built.ready && built.gen == gen {
                return Ok(built.entries.clone());
            }
        }
        // Rebuild from the embeddings table (read-only SELECT — safe on a
        // read-pool connection). Done without the lock held.
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
        let entries = Arc::new(entries);
        let mut built = self.cache.built.lock().expect("vec cache poisoned");
        // Don't let a slower, older rebuild clobber a newer one another reader
        // may have just stored.
        if !built.ready || gen >= built.gen {
            *built = Built {
                gen,
                ready: true,
                entries: entries.clone(),
            };
        }
        Ok(entries)
    }

    /// Hydrate the given message rowids (with their scores) into full hits,
    /// preserving the order given. Used by semantic search after ranking.
    fn hits_for_rowids(&self, ranked: &[(i64, f32)]) -> Result<Vec<SearchHit>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.conversation_id, m.content,
                    c.tool, c.kind, c.project, c.title, c.started_at, c.source_path,
                    COALESCE(us.bookmarked, 0), COALESCE(us.pinned, 0), COALESCE(us.tags, '')
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
                    tags: split_tags(&row.get::<_, String>(10)?),
                    snippet: snippet_of(&content, 160),
                    score: *score as f64,
                    device: None,
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

/// A short, human label for a project path: its last path component (the repo /
/// folder name), trimmed of trailing slashes. Falls back to the whole string.
fn project_label(path: &str) -> String {
    let p = path.trim().trim_end_matches('/');
    p.rsplit(['/', '\\']).next().unwrap_or(p).trim().to_string()
}

/// Parse the stored comma-joined tag string into normalized tags.
fn split_tags(s: &str) -> Vec<String> {
    s.split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Normalize, de-duplicate, and join tags for storage.
fn join_tags(tags: &[String]) -> String {
    let mut v: Vec<String> = tags
        .iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v.join(",")
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
