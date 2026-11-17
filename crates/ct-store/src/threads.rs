//! Threads: one identity for a piece of work across tools (SPEC §6.1).
//!
//! A **session** is one conversation in one tool (a `conversations` row; a
//! growing session has several rows sharing a `session_key`). A **thread** is
//! one or more sessions that are the same work. Every session belongs to
//! exactly one thread: a session nobody has linked is a thread of one, with an
//! id derived from its `session_key`, and costs no storage.
//!
//! Links are typed edges between sessions. They come from an explicit action,
//! from the user, or from [`Store::detect_links`], a deterministic pass that
//! needs no model. Everything here lives in durable tables keyed by
//! `session_key`, so rebuilding the index never loses it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::Store;

/// Thread lifecycle states.
pub const STATUSES: [&str; 3] = ["active", "parked", "done"];
/// Link types (SPEC §6.1).
pub const LINK_KINDS: [&str; 5] = ["continued", "spawned", "reviewed", "conversed", "manual"];

/// Detected links at or above this confidence are applied; below, suggested.
pub const APPLY_THRESHOLD: f64 = 0.8;

/// A later session may continue an earlier one up to this long after it went
/// quiet and still be compared for copied content.
const OVERLAP_WINDOW_HOURS: i64 = 24;
/// A session in another tool that starts while an earlier one is active, or
/// this soon after, is suggested as a continuation.
const PROXIMITY_MINUTES: i64 = 30;
/// Earlier sessions compared against each new one.
const MAX_CANDIDATES: usize = 20;
/// Tail messages of the earlier session used as fingerprints.
const FINGERPRINTS: usize = 8;
const FINGERPRINT_CHARS: usize = 160;
const MIN_FINGERPRINT_CHARS: usize = 80;
/// How much of the later session's opening is searched for those fingerprints.
const OPENING_MESSAGES: i64 = 60;
const OPENING_CHARS: usize = 400_000;

/// What a session is doing right now (SPEC §6.2). The store never fills this
/// in; the daemon does, from tool hooks or from the transcript file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Live {
    /// `running` | `needs_you` | `listening` | `idle` | `ended`.
    pub state: String,
    /// True when guessed from the transcript file instead of reported by a
    /// hook. An inferred state can't tell "needs you" from "idle".
    pub inferred: bool,
    /// When the session entered this state.
    pub since: Option<String>,
}

/// One session, as the latest indexed version of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRef {
    /// Id of the latest indexed version; pass it to `get_conversation`.
    pub conversation_id: String,
    pub session_key: String,
    pub tool: String,
    pub project: Option<String>,
    pub repo_key: Option<String>,
    pub title: Option<String>,
    pub started_at: Option<String>,
    /// When the session was last active (its end, else its start).
    pub last_activity: Option<String>,
    pub branch: Option<String>,
    pub message_count: i64,
    /// The transcript this session was read from.
    #[serde(default)]
    pub source_path: String,
    /// Current state, when known. Filled in by the daemon.
    #[serde(default)]
    pub live: Option<Live>,
    /// The device this session ran on, when the thread spans devices. Filled
    /// in by the daemon.
    #[serde(default)]
    pub device: Option<String>,
}

/// A thread as shown in a list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub id: String,
    pub title: String,
    /// `active` | `parked` | `done`.
    pub status: String,
    pub repo_key: Option<String>,
    /// Short display name for the repository (`owner/repo`).
    pub repo_label: Option<String>,
    /// Tools that took part, earliest first.
    pub tools: Vec<String>,
    pub branches: Vec<String>,
    pub session_count: usize,
    pub message_count: i64,
    pub first_activity: Option<String>,
    pub last_activity: Option<String>,
    /// Suggested links waiting for a decision.
    pub pending_suggestions: usize,
    /// The most pressing state among the thread's sessions, when any is
    /// known. Filled in by the daemon.
    #[serde(default)]
    pub live: Option<Live>,
    /// The device this thread lives on, when the list spans devices. Filled
    /// in by the daemon.
    #[serde(default)]
    pub device: Option<String>,
    /// Every device with part of this thread, when it spans more than one.
    #[serde(default)]
    pub devices: Vec<String>,
    /// Threads on other devices that look like the same work (same repository
    /// and branch, active around the same time) and could be joined to this
    /// one. Filled in by the daemon.
    #[serde(default)]
    pub related: Vec<PartRef>,
    /// Where the work stands. Filled in by the daemon.
    #[serde(default)]
    pub standing: Option<crate::Standing>,
}

/// A thread on a particular device.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartRef {
    pub device: String,
    pub id: String,
    #[serde(default)]
    pub title: String,
}

/// An applied link between two sessions of a thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadLink {
    /// Conversation id of the earlier session.
    pub from: String,
    /// Conversation id of the later session.
    pub to: String,
    pub kind: String,
    /// `explicit` | `detected` | `manual`.
    pub origin: String,
    pub confidence: f64,
}

/// A link the detector proposed but did not apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    /// The session in this thread.
    pub with: String,
    /// The session elsewhere that may belong here.
    pub session: SessionRef,
    /// The thread that session is in now.
    pub thread_id: String,
    pub kind: String,
    pub confidence: f64,
}

/// A thread with its sessions in time order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadDetail {
    #[serde(flatten)]
    pub summary: ThreadSummary,
    pub sessions: Vec<SessionRef>,
    pub links: Vec<ThreadLink>,
    pub suggestions: Vec<Suggestion>,
    /// Notes kept on the thread by the user and by agents. Filled in by the
    /// daemon.
    #[serde(default)]
    pub notes: Vec<crate::ThreadNote>,
    /// This thread's parts on other devices. Filled in by the daemon.
    #[serde(default)]
    pub parts: Vec<PartRef>,
}

/// Filters for a thread list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ThreadFilters {
    /// Only this status.
    #[serde(default)]
    pub status: Option<String>,
    /// Only threads a session of this tool took part in.
    #[serde(default)]
    pub tool: Option<String>,
    /// Only threads whose repo key or label contains this text.
    #[serde(default)]
    pub repo: Option<String>,
    /// At most this many, most recently active first.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// What a detection pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectReport {
    pub scanned: usize,
    pub applied: usize,
    pub suggested: usize,
}

/// Branches that say nothing about which piece of work a session is.
pub const GENERIC_BRANCHES: &[&str] = &["main", "master", "trunk", "develop", "dev", "HEAD"];
/// On a generic branch (or none), sessions in one repository belong to the
/// same thread while you keep working: a gap longer than this starts a new one.
const WORK_GAP_HOURS: i64 = 3;

/// The thread a set of sessions forms on its own (SPEC §6.1, "work key"):
/// - on a feature branch, every session in that repository and branch, in any
///   tool, is one thread, and its id is derived from the repository and
///   branch alone, so every device arrives at the same id;
/// - otherwise, sessions in the same repository (or folder) chain into one
///   thread while each starts within [`WORK_GAP_HOURS`] of the last one ending.
///
/// Sessions with neither a repository nor a folder stay on their own.
fn auto_ids(sessions: &[&SessionRef]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut chains: BTreeMap<(String, String), Vec<&SessionRef>> = BTreeMap::new();
    for s in sessions {
        let Some(place) = s
            .repo_key
            .clone()
            .or_else(|| s.project.clone().filter(|p| !p.is_empty()))
        else {
            continue;
        };
        let branch = s.branch.clone().unwrap_or_default();
        if !branch.is_empty() && !GENERIC_BRANCHES.contains(&branch.as_str()) {
            out.insert(s.session_key.clone(), work_id(&[&place, &branch]));
        } else {
            chains.entry((place, branch)).or_default().push(s);
        }
    }
    let gap = Duration::hours(WORK_GAP_HOURS);
    for ((place, branch), mut list) in chains {
        list.sort_by(|a, b| {
            (a.started_at.is_none(), &a.started_at, &a.session_key).cmp(&(
                b.started_at.is_none(),
                &b.started_at,
                &b.session_key,
            ))
        });
        let mut current: Option<(String, Option<DateTime<Utc>>)> = None;
        for s in list {
            let start = parse_ts(&s.started_at);
            let end = parse_ts(&s.last_activity).or(start);
            let joins = match (&current, start) {
                (Some((_, Some(last))), Some(start)) => start - *last <= gap,
                _ => false,
            };
            if !joins {
                current = Some((work_id(&[&place, &branch, &s.session_key]), end));
            }
            let entry = current.as_mut().expect("set above");
            if end > entry.1 {
                entry.1 = end;
            }
            out.insert(s.session_key.clone(), entry.0.clone());
        }
    }
    out
}

/// A thread id from the parts that identify a piece of work.
fn work_id(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"crossthreads-work-v1");
    for p in parts {
        h.update([0u8]);
        h.update(p.as_bytes());
    }
    let d = h.finalize();
    format!(
        "th_w{}",
        d.iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

/// The id of the thread a session forms on its own.
fn implicit_id(session_key: &str) -> String {
    format!(
        "th_{}",
        session_key.strip_prefix("sk_").unwrap_or(session_key)
    )
}

fn parse_ts(s: &Option<String>) -> Option<DateTime<Utc>> {
    s.as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Utc))
}

/// Lowercase and collapse whitespace, so copied text matches however it was
/// re-wrapped.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(word.chars().flat_map(char::to_lowercase));
    }
    out
}

/// Sessions grouped into threads, with the thread table's titles and statuses.
struct Grouped {
    /// thread id → member sessions, earliest first.
    members: BTreeMap<String, Vec<SessionRef>>,
    /// session key → thread id.
    thread_of: HashMap<String, String>,
    /// thread id → (custom title, status), for threads with a row.
    meta: HashMap<String, (Option<String>, String)>,
}

impl Store {
    // ---- repository identity ----------------------------------------------

    /// Project paths that have sessions without a repo key yet.
    pub fn projects_missing_repo_key(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT project FROM conversations
             WHERE repo_key IS NULL AND project IS NOT NULL AND project <> ''",
        )?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Record the repo key for every session of a project that lacks one.
    pub fn set_repo_key(&self, project: &str, repo_key: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE conversations SET repo_key = ?2 WHERE project = ?1 AND repo_key IS NULL",
            params![project, repo_key],
        )?)
    }

    // ---- reading -----------------------------------------------------------

    /// Every session (latest version of each), with the thread it was put in.
    fn session_rows(&self) -> Result<Vec<(SessionRef, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.session_key, c.tool, c.project, c.repo_key, c.title,
                    c.started_at, c.ended_at, c.git_branch, c.message_count, ts.thread_id,
                    c.source_path
             FROM conversations c
             LEFT JOIN thread_sessions ts ON ts.session_key = c.session_key
             WHERE c.kind = 'thread' AND c.session_key <> ''",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let started_at: Option<String> = r.get(6)?;
                let ended_at: Option<String> = r.get(7)?;
                Ok((
                    SessionRef {
                        conversation_id: r.get(0)?,
                        session_key: r.get(1)?,
                        tool: r.get(2)?,
                        project: r.get(3)?,
                        repo_key: r.get(4)?,
                        title: r.get(5)?,
                        last_activity: ended_at.or_else(|| started_at.clone()),
                        started_at,
                        branch: r.get(8)?,
                        message_count: r.get(9)?,
                        source_path: r.get(11)?,
                        live: None,
                        device: None,
                    },
                    r.get::<_, Option<String>>(10)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // A session that grew has one row per indexed version. Keep the longest.
        let mut latest: HashMap<String, (SessionRef, Option<String>)> = HashMap::new();
        for row in rows {
            match latest.get(&row.0.session_key) {
                Some(kept) if kept.0.message_count >= row.0.message_count => {}
                _ => {
                    latest.insert(row.0.session_key.clone(), row);
                }
            }
        }
        Ok(latest.into_values().collect())
    }

    fn grouped(&self) -> Result<Grouped> {
        let mut members: BTreeMap<String, Vec<SessionRef>> = BTreeMap::new();
        let mut thread_of = HashMap::new();
        let rows = self.session_rows()?;
        let auto = {
            let unplaced: Vec<&SessionRef> = rows
                .iter()
                .filter(|(_, t)| t.is_none())
                .map(|(s, _)| s)
                .collect();
            auto_ids(&unplaced)
        };
        for (session, thread_id) in rows {
            let id = thread_id
                .or_else(|| auto.get(&session.session_key).cloned())
                .unwrap_or_else(|| implicit_id(&session.session_key));
            thread_of.insert(session.session_key.clone(), id.clone());
            members.entry(id).or_default().push(session);
        }
        for sessions in members.values_mut() {
            // Undated sessions sort last; ties break on the key for stability.
            sessions.sort_by(|a, b| {
                (a.started_at.is_none(), &a.started_at, &a.session_key).cmp(&(
                    b.started_at.is_none(),
                    &b.started_at,
                    &b.session_key,
                ))
            });
        }

        let mut meta = HashMap::new();
        let mut stmt = self.conn.prepare("SELECT id, title, status FROM threads")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, title, status) = row?;
            meta.insert(id, (title, status));
        }
        Ok(Grouped {
            members,
            thread_of,
            meta,
        })
    }

    /// Suggested links, as (earlier key, later key, kind, confidence).
    fn suggested_links(&self) -> Result<Vec<(String, String, String, f64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT from_key, to_key, kind, confidence FROM links WHERE state = 'suggested'",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn summarize(
        id: &str,
        sessions: &[SessionRef],
        meta: Option<&(Option<String>, String)>,
        suggestions: usize,
    ) -> ThreadSummary {
        let custom = meta.and_then(|m| m.0.clone()).filter(|t| !t.is_empty());
        let derived = sessions
            .iter()
            .find_map(|s| s.title.clone().filter(|t| !t.trim().is_empty()));
        let repo_key = sessions.iter().find_map(|s| s.repo_key.clone());
        let mut tools: Vec<String> = Vec::new();
        let mut branches: Vec<String> = Vec::new();
        for s in sessions {
            if !tools.contains(&s.tool) {
                tools.push(s.tool.clone());
            }
            if let Some(b) = s.branch.as_ref().filter(|b| !b.is_empty()) {
                if !branches.contains(b) {
                    branches.push(b.clone());
                }
            }
        }
        ThreadSummary {
            id: id.to_string(),
            title: custom
                .or(derived)
                .unwrap_or_else(|| "(untitled)".to_string()),
            status: meta
                .map(|m| m.1.clone())
                .unwrap_or_else(|| "active".to_string()),
            repo_label: repo_key.as_deref().map(ct_core::repo::repo_label),
            repo_key,
            tools,
            branches,
            session_count: sessions.len(),
            message_count: sessions.iter().map(|s| s.message_count).sum(),
            first_activity: sessions.iter().filter_map(|s| s.started_at.clone()).min(),
            last_activity: sessions
                .iter()
                .filter_map(|s| s.last_activity.clone())
                .max(),
            pending_suggestions: suggestions,
            live: None,
            device: None,
            devices: Vec::new(),
            related: Vec::new(),
            standing: None,
        }
    }

    /// Threads, most recently active first.
    pub fn threads(&self, f: &ThreadFilters) -> Result<Vec<ThreadSummary>> {
        Ok(self
            .threads_with_sessions(f)?
            .into_iter()
            .map(|(t, _)| t)
            .collect())
    }

    /// Like [`Store::threads`], with each thread's sessions (oldest first).
    pub fn threads_with_sessions(
        &self,
        f: &ThreadFilters,
    ) -> Result<Vec<(ThreadSummary, Vec<SessionRef>)>> {
        let g = self.grouped()?;
        let mut pending: HashMap<&str, usize> = HashMap::new();
        let suggested = self.suggested_links()?;
        for (from, to, _, _) in &suggested {
            for key in [from, to] {
                if let Some(id) = g.thread_of.get(key) {
                    *pending.entry(id.as_str()).or_default() += 1;
                }
            }
        }

        let want_repo = f.repo.as_deref().map(str::to_lowercase);
        let mut out: Vec<(ThreadSummary, Vec<SessionRef>)> = g
            .members
            .iter()
            .map(|(id, sessions)| {
                let summary = Self::summarize(
                    id,
                    sessions,
                    g.meta.get(id),
                    pending.get(id.as_str()).copied().unwrap_or(0),
                );
                (summary, sessions.clone())
            })
            .filter(|(t, _)| f.status.as_deref().map_or(true, |s| t.status == s))
            .filter(|(t, _)| {
                f.tool
                    .as_deref()
                    .map_or(true, |x| t.tools.iter().any(|y| y == x))
            })
            .filter(|(t, _)| {
                want_repo.as_deref().map_or(true, |want| {
                    [&t.repo_key, &t.repo_label]
                        .into_iter()
                        .flatten()
                        .any(|v| v.to_lowercase().contains(want))
                })
            })
            .collect();
        out.sort_by(|(a, _), (b, _)| {
            (b.last_activity.is_some(), &b.last_activity, &b.id).cmp(&(
                a.last_activity.is_some(),
                &a.last_activity,
                &a.id,
            ))
        });
        if let Some(limit) = f.limit {
            out.truncate(limit);
        }
        Ok(out)
    }

    /// One thread with its sessions, links and pending suggestions. `id` may be
    /// a thread id or the conversation id of any session in it.
    pub fn thread(&self, id: &str) -> Result<Option<ThreadDetail>> {
        let g = self.grouped()?;
        let id = match g.members.contains_key(id) {
            true => id.to_string(),
            false => match self
                .session_key_of(id)?
                .and_then(|k| g.thread_of.get(&k).cloned())
            {
                Some(found) => found,
                None => return Ok(None),
            },
        };
        let sessions = g.members[&id].clone();
        let conv_of: HashMap<&str, &SessionRef> = g
            .members
            .values()
            .flatten()
            .map(|s| (s.session_key.as_str(), s))
            .collect();
        let in_thread: HashSet<&str> = sessions.iter().map(|s| s.session_key.as_str()).collect();

        let mut links = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT from_key, to_key, kind, origin, confidence FROM links
                 WHERE state = 'applied' ORDER BY created_at",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, f64>(4)?,
                ))
            })?;
            for row in rows {
                let (from, to, kind, origin, confidence) = row?;
                if !in_thread.contains(from.as_str()) || !in_thread.contains(to.as_str()) {
                    continue;
                }
                links.push(ThreadLink {
                    from: conv_of[from.as_str()].conversation_id.clone(),
                    to: conv_of[to.as_str()].conversation_id.clone(),
                    kind,
                    origin,
                    confidence,
                });
            }
        }

        let mut suggestions = Vec::new();
        for (from, to, kind, confidence) in self.suggested_links()? {
            let (mine, other) = match (
                in_thread.contains(from.as_str()),
                in_thread.contains(to.as_str()),
            ) {
                (true, false) => (from, to),
                (false, true) => (to, from),
                _ => continue,
            };
            // The other session may have been forgotten since.
            let (Some(other_ref), Some(other_thread)) =
                (conv_of.get(other.as_str()), g.thread_of.get(&other))
            else {
                continue;
            };
            suggestions.push(Suggestion {
                with: conv_of[mine.as_str()].conversation_id.clone(),
                session: (*other_ref).clone(),
                thread_id: other_thread.clone(),
                kind,
                confidence,
            });
        }

        Ok(Some(ThreadDetail {
            summary: Self::summarize(&id, &sessions, g.meta.get(&id), suggestions.len()),
            sessions,
            links,
            suggestions,
            notes: Vec::new(),
            parts: Vec::new(),
        }))
    }

    // ---- writing -----------------------------------------------------------

    fn explicit_thread(&self, session_key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT thread_id FROM thread_sessions WHERE session_key = ?1",
                [session_key],
                |r| r.get(0),
            )
            .optional()?)
    }

    fn session_started(&self, session_key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT MIN(started_at) FROM conversations WHERE session_key = ?1",
                [session_key],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Give a session's thread a row and a membership entry, if it has none.
    fn materialize(&self, session_key: &str) -> Result<String> {
        if let Some(id) = self.explicit_thread(session_key)? {
            return Ok(id);
        }
        // The thread it is in now, automatically grouped or on its own.
        let id = self
            .grouped()?
            .thread_of
            .get(session_key)
            .cloned()
            .unwrap_or_else(|| implicit_id(session_key));
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR IGNORE INTO threads (id, status, created_at, updated_at)
             VALUES (?1, 'active', ?2, ?2)",
            params![id, now],
        )?;
        self.conn.execute(
            "INSERT OR REPLACE INTO thread_sessions (session_key, thread_id) VALUES (?1, ?2)",
            params![session_key, id],
        )?;
        Ok(id)
    }

    /// Put two sessions in one thread, keeping the earlier session's thread.
    fn merge(&self, earlier: &str, later: &str) -> Result<String> {
        let keep = self.materialize(earlier)?;
        let gone = self.explicit_thread(later)?;
        match gone {
            Some(other) if other == keep => {}
            Some(other) => {
                // Carry a custom title across when the kept thread has none.
                self.conn.execute(
                    "UPDATE threads SET title = COALESCE(title,
                        (SELECT title FROM threads WHERE id = ?2)) WHERE id = ?1",
                    params![keep, other],
                )?;
                self.conn.execute(
                    "UPDATE thread_sessions SET thread_id = ?1 WHERE thread_id = ?2",
                    params![keep, other],
                )?;
                self.conn
                    .execute("DELETE FROM threads WHERE id = ?1", [&other])?;
            }
            None => {
                self.conn.execute(
                    "INSERT OR REPLACE INTO thread_sessions (session_key, thread_id)
                     VALUES (?1, ?2)",
                    params![later, keep],
                )?;
            }
        }
        self.conn.execute(
            "UPDATE threads SET updated_at = ?2 WHERE id = ?1",
            params![keep, Utc::now().to_rfc3339()],
        )?;
        Ok(keep)
    }

    /// Order two session keys so the earlier-started session comes first.
    fn ordered(&self, a: &str, b: &str) -> Result<(String, String)> {
        let (sa, sb) = (self.session_started(a)?, self.session_started(b)?);
        let a_first = match (&sa, &sb) {
            (Some(x), Some(y)) => (x, a) <= (y, b),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => a <= b,
        };
        Ok(if a_first {
            (a.to_string(), b.to_string())
        } else {
            (b.to_string(), a.to_string())
        })
    }

    fn rejected(&self, a: &str, b: &str) -> Result<bool> {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM link_rejections WHERE a_key = ?1 AND b_key = ?2",
                params![lo, hi],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    fn link_exists(&self, a: &str, b: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM links
                 WHERE (from_key = ?1 AND to_key = ?2) OR (from_key = ?2 AND to_key = ?1)",
                params![a, b],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Write a link between two session keys, replacing any link between the
    /// same pair. Applied links also merge the two sessions' threads.
    fn put_link(
        &self,
        a: &str,
        b: &str,
        kind: &str,
        origin: &str,
        confidence: f64,
        apply: bool,
    ) -> Result<Option<String>> {
        let (from, to) = self.ordered(a, b)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM links
             WHERE (from_key = ?1 AND to_key = ?2) OR (from_key = ?2 AND to_key = ?1)",
            params![from, to],
        )?;
        tx.execute(
            "INSERT INTO links (from_key, to_key, kind, origin, confidence, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                from,
                to,
                kind,
                origin,
                confidence,
                if apply { "applied" } else { "suggested" },
                Utc::now().to_rfc3339()
            ],
        )?;
        let thread = if apply {
            Some(self.merge(&from, &to)?)
        } else {
            None
        };
        tx.commit()?;
        Ok(thread)
    }

    /// Link two sessions (by conversation id) into one thread. This is also how
    /// a suggestion is accepted. Returns the thread id, or `None` when either
    /// id is unknown or both name the same session.
    pub fn link_sessions(
        &self,
        a: &str,
        b: &str,
        kind: &str,
        origin: &str,
    ) -> Result<Option<String>> {
        if !LINK_KINDS.contains(&kind) {
            bail!("unknown link kind `{kind}` (expected one of {LINK_KINDS:?})");
        }
        let (Some(ka), Some(kb)) = (self.session_key_of(a)?, self.session_key_of(b)?) else {
            return Ok(None);
        };
        if ka == kb {
            return Ok(None);
        }
        // Linking on purpose overrides an earlier "these aren't related".
        let (lo, hi) = if ka <= kb { (&ka, &kb) } else { (&kb, &ka) };
        self.conn.execute(
            "DELETE FROM link_rejections WHERE a_key = ?1 AND b_key = ?2",
            params![lo, hi],
        )?;
        self.put_link(&ka, &kb, kind, origin, 1.0, true)
    }

    /// Move every session of thread `part` into thread `into` (both on this
    /// device). The kept thread keeps its id; a custom title moves across when
    /// it has none. Returns the thread id, or `None` if either is unknown.
    pub fn merge_threads(&self, part: &str, into: &str) -> Result<Option<String>> {
        let g = self.grouped()?;
        let resolve = |id: &str| -> Result<Option<String>> {
            if g.members.contains_key(id) {
                return Ok(Some(id.to_string()));
            }
            Ok(self
                .session_key_of(id)?
                .and_then(|k| g.thread_of.get(&k).cloned()))
        };
        let (Some(part), Some(into)) = (resolve(part)?, resolve(into)?) else {
            return Ok(None);
        };
        if part == into {
            return Ok(Some(into));
        }
        let now = Utc::now().to_rfc3339();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO threads (id, status, created_at, updated_at)
             VALUES (?1, 'active', ?2, ?2)",
            params![into, now],
        )?;
        tx.execute(
            "UPDATE threads SET title = COALESCE(title,
                (SELECT title FROM threads WHERE id = ?2)), updated_at = ?3 WHERE id = ?1",
            params![into, part, now],
        )?;
        for s in &g.members[&part] {
            tx.execute(
                "INSERT OR REPLACE INTO thread_sessions (session_key, thread_id) VALUES (?1, ?2)",
                params![s.session_key, into],
            )?;
        }
        tx.execute("DELETE FROM threads WHERE id = ?1", [&part])?;
        tx.commit()?;
        Ok(Some(into))
    }

    /// Dismiss a suggested link and remember it, so it is never proposed again.
    /// An applied link is left alone (use [`Store::unlink_session`]).
    pub fn reject_link(&self, a: &str, b: &str) -> Result<bool> {
        let (Some(ka), Some(kb)) = (self.session_key_of(a)?, self.session_key_of(b)?) else {
            return Ok(false);
        };
        self.conn.execute(
            "DELETE FROM links WHERE state = 'suggested'
             AND ((from_key = ?1 AND to_key = ?2) OR (from_key = ?2 AND to_key = ?1))",
            params![ka, kb],
        )?;
        self.remember_rejection(&ka, &kb)?;
        Ok(true)
    }

    fn remember_rejection(&self, a: &str, b: &str) -> Result<()> {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        self.conn.execute(
            "INSERT OR IGNORE INTO link_rejections (a_key, b_key) VALUES (?1, ?2)",
            params![lo, hi],
        )?;
        Ok(())
    }

    /// Take a session (by conversation id) out of its thread, back to a thread
    /// of one. Its links are removed, and detected ones are remembered as
    /// rejected. Returns `false` if the session is unknown or already alone.
    pub fn unlink_session(&self, conversation_id: &str) -> Result<bool> {
        let Some(key) = self.session_key_of(conversation_id)? else {
            return Ok(false);
        };
        let alone = implicit_id(&key);
        {
            let g = self.grouped()?;
            let Some(current) = g.thread_of.get(&key) else {
                return Ok(false);
            };
            if g.members.get(current).map_or(0, Vec::len) <= 1 {
                return Ok(false);
            }
        }
        let Some(thread) = self.explicit_thread(&key)? else {
            // Grouped automatically: pin it on its own, so it stays out.
            self.conn.execute(
                "INSERT OR REPLACE INTO thread_sessions (session_key, thread_id) VALUES (?1, ?2)",
                params![key, alone],
            )?;
            return Ok(true);
        };
        let tx = self.conn.unchecked_transaction()?;

        let touching: Vec<(String, String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT from_key, to_key, origin FROM links
                 WHERE state = 'applied' AND (from_key = ?1 OR to_key = ?1)",
            )?;
            let rows = stmt
                .query_map([&key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (from, to, origin) in &touching {
            if origin == "detected" {
                self.remember_rejection(from, to)?;
            }
        }
        tx.execute(
            "DELETE FROM links WHERE state = 'applied' AND (from_key = ?1 OR to_key = ?1)",
            [&key],
        )?;
        tx.execute("DELETE FROM thread_sessions WHERE session_key = ?1", [&key])?;

        let rest: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT ts.session_key FROM thread_sessions ts
                 LEFT JOIN conversations c ON c.session_key = ts.session_key
                 WHERE ts.thread_id = ?1
                 GROUP BY ts.session_key
                 ORDER BY MIN(c.started_at) IS NULL, MIN(c.started_at), ts.session_key",
            )?;
            let rows = stmt
                .query_map([&thread], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let (custom_title, status): (Option<String>, String) = tx
            .query_row(
                "SELECT title, status FROM threads WHERE id = ?1",
                [&thread],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or((None, "active".to_string()));
        let plain = custom_title.map_or(true, |t| t.is_empty()) && status == "active";

        if rest.is_empty() || (rest.len() == 1 && plain) {
            // Nothing left worth a row: the remaining session is a thread of one.
            tx.execute(
                "DELETE FROM thread_sessions WHERE thread_id = ?1",
                [&thread],
            )?;
            tx.execute("DELETE FROM threads WHERE id = ?1", [&thread])?;
        } else if thread == implicit_id(&key) {
            // The thread was named after the session that just left, which now
            // owns that id again on its own. Rename the thread after its
            // earliest remaining session.
            let renamed = implicit_id(&rest[0]);
            tx.execute(
                "UPDATE threads SET id = ?2 WHERE id = ?1",
                params![thread, renamed],
            )?;
            tx.execute(
                "UPDATE thread_sessions SET thread_id = ?2 WHERE thread_id = ?1",
                params![thread, renamed],
            )?;
        }
        // Keep it on its own: without a row it would rejoin its work's thread.
        tx.execute(
            "INSERT OR REPLACE INTO thread_sessions (session_key, thread_id) VALUES (?1, ?2)",
            params![key, alone],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Rename a thread and/or change its status. An empty title restores the
    /// derived one. Returns the thread id, or `None` if there is no such thread.
    pub fn set_thread(
        &self,
        id: &str,
        title: Option<&str>,
        status: Option<&str>,
    ) -> Result<Option<String>> {
        if let Some(s) = status {
            if !STATUSES.contains(&s) {
                bail!("unknown status `{s}` (expected one of {STATUSES:?})");
            }
        }
        let exists = self
            .conn
            .query_row("SELECT 1 FROM threads WHERE id = ?1", [id], |_| Ok(()))
            .optional()?
            .is_some();
        if !exists && self.grouped()?.members.contains_key(id) {
            // A thread formed automatically has no row until it is edited.
            let now = Utc::now().to_rfc3339();
            self.conn.execute(
                "INSERT OR IGNORE INTO threads (id, status, created_at, updated_at)
                 VALUES (?1, 'active', ?2, ?2)",
                params![id, now],
            )?;
        } else if !exists {
            // A thread of one has no row yet. Its id names its session.
            let Some(suffix) = id.strip_prefix("th_") else {
                return Ok(None);
            };
            let key = format!("sk_{suffix}");
            let known = self
                .conn
                .query_row(
                    "SELECT 1 FROM conversations WHERE session_key = ?1 LIMIT 1",
                    [&key],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !known || self.explicit_thread(&key)?.is_some() {
                return Ok(None);
            }
            self.materialize(&key)?;
        }
        let now = Utc::now().to_rfc3339();
        if let Some(t) = title {
            let t = t.trim();
            self.conn.execute(
                "UPDATE threads SET title = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, (!t.is_empty()).then_some(t), now],
            )?;
        }
        if let Some(s) = status {
            self.conn.execute(
                "UPDATE threads SET status = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, s, now],
            )?;
        }
        Ok(Some(id.to_string()))
    }

    // ---- detection ---------------------------------------------------------

    /// Fingerprints from the tail of a session as it stood at `before`: the
    /// opening of each of its last few substantial messages, with whether the
    /// assistant wrote it.
    fn tail_fingerprints(
        &self,
        conversation_id: &str,
        before: DateTime<Utc>,
    ) -> Result<Vec<(String, bool)>> {
        let mut stmt = self.conn.prepare(
            "SELECT role, content, timestamp FROM messages
             WHERE conversation_id = ?1 AND seq > 0 AND role IN ('user', 'assistant')
             ORDER BY seq DESC LIMIT 200",
        )?;
        let rows = stmt.query_map([conversation_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        let cutoff = before + Duration::minutes(1);
        let mut out = Vec::new();
        for row in rows {
            let (role, content, ts) = row?;
            // Ignore what the earlier session said after the later one began.
            if parse_ts(&ts).is_some_and(|t| t > cutoff) {
                continue;
            }
            let norm = normalize(&content);
            if norm.chars().count() < MIN_FINGERPRINT_CHARS {
                continue;
            }
            out.push((
                norm.chars().take(FINGERPRINT_CHARS).collect(),
                role == "assistant",
            ));
            if out.len() == FINGERPRINTS {
                break;
            }
        }
        Ok(out)
    }

    /// The normalised opening of a session.
    fn opening_text(&self, conversation_id: &str) -> Result<String> {
        let mut stmt = self.conn.prepare(
            "SELECT content FROM messages WHERE conversation_id = ?1 ORDER BY seq LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![conversation_id, OPENING_MESSAGES], |r| {
            r.get::<_, String>(0)
        })?;
        let mut out = String::new();
        for row in rows {
            if out.len() >= OPENING_CHARS {
                break;
            }
            out.push_str(&normalize(&row?));
            out.push('\n');
        }
        Ok(out)
    }

    /// Look for links among sessions not examined before. Deterministic; no
    /// model. Two signals, both within one repository:
    ///
    /// - **Copied content.** The later session opens with text from the tail of
    ///   the earlier one: the signature of a transcript transfer or a resume.
    ///   Applied as `continued`.
    /// - **Proximity.** A session in a different tool starts while the earlier
    ///   one is active or just after. Only suggested.
    ///
    /// Pairs the user rejected, and pairs already linked, are skipped.
    pub fn detect_links(&self) -> Result<DetectReport> {
        let scanned: HashSet<String> = {
            let mut stmt = self.conn.prepare("SELECT session_key FROM link_scans")?;
            let rows = stmt
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<HashSet<_>>>()?;
            rows
        };
        let all: Vec<SessionRef> = self.session_rows()?.into_iter().map(|(s, _)| s).collect();
        let mut by_repo: HashMap<&str, Vec<&SessionRef>> = HashMap::new();
        for s in &all {
            if let (Some(repo), Some(_)) = (s.repo_key.as_deref(), &s.started_at) {
                by_repo.entry(repo).or_default().push(s);
            }
        }

        let mut report = DetectReport::default();
        let mut fresh: Vec<&SessionRef> = all
            .iter()
            .filter(|s| !scanned.contains(&s.session_key))
            .collect();
        // Oldest first, so a chain A → B → C links in order.
        fresh.sort_by(|a, b| (&a.started_at, &a.session_key).cmp(&(&b.started_at, &b.session_key)));

        let now = Utc::now().to_rfc3339();
        let mut seen_pairs: BTreeSet<(String, String)> = BTreeSet::new();
        for later in fresh {
            report.scanned += 1;
            self.conn.execute(
                "INSERT OR REPLACE INTO link_scans (session_key, scanned_at) VALUES (?1, ?2)",
                params![later.session_key, now],
            )?;
            let (Some(repo), Some(began)) =
                (later.repo_key.as_deref(), parse_ts(&later.started_at))
            else {
                continue;
            };

            // Earlier sessions in the same repository that were still warm.
            let mut candidates: Vec<(&SessionRef, DateTime<Utc>)> = by_repo
                .get(repo)
                .into_iter()
                .flatten()
                .filter(|e| e.session_key != later.session_key)
                .filter_map(|e| {
                    let start = parse_ts(&e.started_at)?;
                    let last = parse_ts(&e.last_activity).unwrap_or(start);
                    (start <= began && began <= last + Duration::hours(OVERLAP_WINDOW_HOURS))
                        .then_some((*e, last))
                })
                .collect();
            candidates.sort_by(|a, b| b.0.started_at.cmp(&a.0.started_at));
            candidates.truncate(MAX_CANDIDATES);
            if candidates.is_empty() {
                continue;
            }

            let mut opening: Option<String> = None;
            let mut linked = false;
            let mut nearest: Option<&SessionRef> = None;
            for (earlier, last) in candidates {
                let pair = (earlier.session_key.clone(), later.session_key.clone());
                if !seen_pairs.insert(pair)
                    || self.rejected(&earlier.session_key, &later.session_key)?
                    || self.link_exists(&earlier.session_key, &later.session_key)?
                {
                    continue;
                }
                let prints = self.tail_fingerprints(&earlier.conversation_id, began)?;
                if prints.len() >= 3 {
                    let text = match &opening {
                        Some(t) => t,
                        None => opening.insert(self.opening_text(&later.conversation_id)?),
                    };
                    let hits: Vec<bool> = prints
                        .iter()
                        .filter(|(p, _)| text.contains(p.as_str()))
                        .map(|(_, assistant)| *assistant)
                        .collect();
                    // Shared boilerplate (a pasted prompt, an instructions file)
                    // is user text. Require the earlier assistant's words too.
                    if hits.len() >= 2 && hits.len() * 2 >= prints.len() && hits.contains(&true) {
                        self.put_link(
                            &earlier.session_key,
                            &later.session_key,
                            "continued",
                            "detected",
                            0.9,
                            true,
                        )?;
                        report.applied += 1;
                        linked = true;
                        break;
                    }
                }
                if nearest.is_none()
                    && earlier.tool != later.tool
                    && began <= last + Duration::minutes(PROXIMITY_MINUTES)
                {
                    nearest = Some(earlier);
                }
            }
            if let (false, Some(earlier)) = (linked, nearest) {
                self.put_link(
                    &earlier.session_key,
                    &later.session_key,
                    "continued",
                    "detected",
                    0.5,
                    false,
                )?;
                report.suggested += 1;
            }
        }
        Ok(report)
    }
}

/// Cut `s` to at most `max` bytes on a char boundary, marking the cut.
fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max.saturating_sub(2);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} …", s[..end].trim_end())
}

fn day(ts: &Option<String>) -> &str {
    ts.as_deref().and_then(|t| t.get(..10)).unwrap_or("undated")
}

/// A paste-ready brief for continuing a thread in another tool, or handing it
/// to another agent: what the work is, every session that touched it, the
/// original request, and where the latest session left off.
///
/// The budget goes mostly to the **end** of the latest session, because that
/// is where the work stands; the opening request gets a fixed share so the
/// goal is never lost. `first` is the earliest session's transcript and
/// `latest` the most recent; they may be the same.
pub fn render_handoff(
    t: &ThreadDetail,
    first: Option<&crate::StoredConversation>,
    latest: &crate::StoredConversation,
    max_chars: usize,
) -> crate::ContextBlock {
    let s = &t.summary;
    let mut md = format!("## Handoff: {}\n\n", s.title);
    let mut facts = Vec::new();
    if let Some(repo) = &s.repo_label {
        facts.push(format!("Repository: {repo}"));
    }
    if let Some(project) = &latest.project {
        facts.push(format!("Working directory: `{project}`"));
    }
    if let Some(branch) = s.branches.last() {
        facts.push(format!("Branch: `{branch}`"));
    }
    for f in facts {
        md.push_str(&format!("- {f}\n"));
    }
    md.push_str(
        "\nYou are picking up work that started in another session. Read where it left off, \
         check the current state of the files before trusting the summary, and continue.\n",
    );

    md.push_str("\n### Sessions so far\n");
    for (i, sess) in t.sessions.iter().enumerate() {
        md.push_str(&format!(
            "{}. {} · {} · {} messages — {}\n",
            i + 1,
            sess.tool,
            day(&sess.started_at),
            sess.message_count,
            clip(sess.title.as_deref().unwrap_or("(untitled)"), 90),
        ));
    }

    let mut sources = Vec::new();
    // The goal: the first thing the user asked, in the first session.
    let opening = first.unwrap_or(latest);
    if let Some(ask) = opening
        .messages
        .iter()
        .find(|m| m.role == "user" && !m.content.trim().is_empty())
    {
        md.push_str("\n### The original request\n\n");
        md.push_str(&clip(&ask.content, (max_chars / 5).clamp(300, 2000)));
        md.push('\n');
        sources.push(opening.id.clone());
    }

    // Where it stands: the tail of the latest session, newest kept first, then
    // put back in order.
    let budget = max_chars.saturating_sub(md.len() + 120);
    let mut tail: Vec<String> = Vec::new();
    let mut used = 0;
    let mut cut_short = false;
    for m in latest.messages.iter().rev() {
        let body = m.content.trim();
        if body.is_empty() || m.role == "tool" || m.role == "system" {
            continue;
        }
        let left = budget.saturating_sub(used);
        if left < 200 {
            cut_short = true;
            break;
        }
        let turn = format!("**{}:** {}\n\n", m.role, clip(body, left - 20));
        used += turn.len();
        tail.push(turn);
    }
    tail.reverse();
    md.push_str(&format!(
        "\n### Where it left off ({}, {})\n\n",
        latest.tool,
        day(&latest.started_at)
    ));
    if cut_short {
        md.push_str("_…earlier messages omitted…_\n\n");
    }
    for turn in tail {
        md.push_str(&turn);
    }
    if !sources.contains(&latest.id) {
        sources.push(latest.id.clone());
    }

    let chars = md.chars().count();
    crate::ContextBlock {
        markdown: md,
        sources,
        chars,
        token_estimate: chars / 4,
    }
}

impl Store {
    /// A handoff for a thread on this device (see [`render_handoff`]). `id` is
    /// a thread id or any of its sessions' conversation ids.
    pub fn thread_handoff(
        &self,
        id: &str,
        max_chars: usize,
    ) -> Result<Option<crate::ContextBlock>> {
        let Some(t) = self.thread(id)? else {
            return Ok(None);
        };
        let (Some(first), Some(last)) = (t.sessions.first(), t.sessions.last()) else {
            return Ok(None);
        };
        let first = self.get_conversation(&first.conversation_id)?;
        let Some(latest) = self.get_conversation(&last.conversation_id)? else {
            return Ok(None);
        };
        Ok(Some(render_handoff(&t, first.as_ref(), &latest, max_chars)))
    }
}
