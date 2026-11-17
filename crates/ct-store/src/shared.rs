//! Thread state shared across the user's devices (SPEC §6.7).
//!
//! A thread's sessions live on the machines that ran them, but the way the
//! user organises the work should be the same everywhere: its name, its status,
//! which sessions on which machines are one piece of work, and the notes kept
//! on it. That small amount of state lives here, on every device, and the
//! daemon copies it between devices.
//!
//! Merging is per field, last writer wins: every field records when it was set
//! (epoch milliseconds) and by which device, and a newer value replaces an
//! older one. Ties go to the larger device name, so every device settles on the
//! same answer whatever order changes arrive in. Nothing is ever deleted:
//! a removed part or note is kept with `removed` set, so the removal travels.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

use anyhow::{bail, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::Store;

/// The longest thread name kept.
pub const MAX_TITLE: usize = 200;
/// The longest note kept.
pub const MAX_NOTE: usize = 4000;
/// The most items of each kind sent in one sync batch.
pub const MAX_BATCH: usize = 2000;
/// Kinds of note.
pub const NOTE_KINDS: &[&str] = &["decision", "todo", "done", "context"];

/// The shared name and status of a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedThread {
    pub id: String,
    pub title: Option<String>,
    pub title_at: i64,
    #[serde(default)]
    pub title_by: String,
    pub status: Option<String>,
    pub status_at: i64,
    #[serde(default)]
    pub status_by: String,
    pub updated: i64,
    /// Sent to another device to continue there: (device, tool), or empty
    /// once picked up.
    #[serde(default)]
    pub moved_to: Option<String>,
    #[serde(default)]
    pub moved_tool: Option<String>,
    #[serde(default)]
    pub moved_at: i64,
    #[serde(default)]
    pub moved_by: String,
}

/// One device's thread, attached to a shared thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedPart {
    /// The device the part lives on.
    pub device: String,
    /// That device's thread id.
    pub part: String,
    /// The thread it belongs to.
    pub thread_id: String,
    /// The device that thread lives on.
    #[serde(default)]
    pub thread_device: String,
    pub removed: bool,
    pub at: i64,
    #[serde(default)]
    pub by: String,
}

/// A note kept on a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadNote {
    pub id: String,
    pub thread_id: String,
    /// `decision` | `todo` | `done` | `context`.
    pub kind: String,
    pub text: String,
    /// Who wrote it: an agent's name, or `you`.
    #[serde(default)]
    pub author: String,
    /// The device it was written on.
    #[serde(default)]
    pub device: String,
    pub created: i64,
    pub removed: bool,
    pub updated: i64,
}

/// Everything changed since some time, as sent between devices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedState {
    #[serde(default)]
    pub threads: Vec<SharedThread>,
    #[serde(default)]
    pub parts: Vec<SharedPart>,
    #[serde(default)]
    pub notes: Vec<ThreadNote>,
    /// The newest change included; ask for changes after it next time.
    #[serde(default)]
    pub upto: i64,
}

impl SharedState {
    pub fn is_empty(&self) -> bool {
        self.threads.is_empty() && self.parts.is_empty() && self.notes.is_empty()
    }
}

/// What a handoff brief says about where it came from.
pub const HANDOFF_MARKER: &str = "Crossthreads thread: ";
const MARKER_SCAN_KEY: &str = "handoff_marker_scan";

/// The handoff marker line for a thread on a device.
pub fn handoff_marker(thread_id: &str, device: &str) -> String {
    format!("{HANDOFF_MARKER}`{thread_id}@{device}`")
}

/// Find a handoff marker in a message: `(thread id, device)`.
pub fn parse_marker(text: &str) -> Option<(String, String)> {
    let at = text.find(HANDOFF_MARKER)?;
    let rest = text[at + HANDOFF_MARKER.len()..].trim_start_matches('`');
    let token: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@'))
        .collect();
    let (id, device) = token.split_once('@')?;
    (id.starts_with("th_") && !device.is_empty() && id.len() <= 80 && device.len() <= 80)
        .then(|| (id.to_string(), device.to_string()))
}

/// Does (at, by) win over (cur_at, cur_by)?
fn newer(at: i64, by: &str, cur_at: i64, cur_by: &str) -> bool {
    (at, by) > (cur_at, cur_by)
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// 16 random-looking hex characters (time plus a per-process random key).
pub(crate) fn random_hex() -> String {
    random_suffix()
}

fn random_suffix() -> String {
    let mut h = RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    );
    format!("{:016x}", h.finish())
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Store {
    /// The shared record for a thread, if any.
    pub fn shared_thread(&self, id: &str) -> Result<Option<SharedThread>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, title, title_at, title_by, status, status_at, status_by, updated,
                        moved_to, moved_tool, moved_at, moved_by
                 FROM shared_threads WHERE id = ?1",
                [id],
                row_thread,
            )
            .optional()?)
    }

    /// Every shared thread record.
    pub fn shared_threads(&self) -> Result<Vec<SharedThread>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, title_at, title_by, status, status_at, status_by, updated,
                    moved_to, moved_tool, moved_at, moved_by
             FROM shared_threads",
        )?;
        let rows = stmt.query_map([], row_thread)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Set a thread's shared name and/or status, as `device` at `at`. An empty
    /// title clears it (the thread goes back to its derived name).
    pub fn share_thread_edit(
        &self,
        id: &str,
        title: Option<&str>,
        status: Option<&str>,
        device: &str,
        at: i64,
    ) -> Result<()> {
        if let Some(s) = status {
            if !crate::threads::STATUSES.contains(&s) {
                bail!("unknown status `{s}`");
            }
        }
        let mut t = self.shared_thread(id)?.unwrap_or(SharedThread {
            id: id.to_string(),
            title: None,
            title_at: 0,
            title_by: String::new(),
            status: None,
            status_at: 0,
            status_by: String::new(),
            updated: 0,
            moved_to: None,
            moved_tool: None,
            moved_at: 0,
            moved_by: String::new(),
        });
        if let Some(title) = title {
            let title = clip(title, MAX_TITLE);
            t.title = (!title.is_empty()).then_some(title);
            t.title_at = at;
            t.title_by = device.to_string();
        }
        if let Some(s) = status {
            t.status = Some(s.to_string());
            t.status_at = at;
            t.status_by = device.to_string();
        }
        t.updated = at;
        self.merge_thread(&t)?;
        Ok(())
    }

    /// Send a thread to another device to continue there (`to` = None clears
    /// it, once picked up).
    pub fn share_thread_move(
        &self,
        id: &str,
        to: Option<&str>,
        tool: Option<&str>,
        device: &str,
        at: i64,
    ) -> Result<()> {
        let mut t = self.shared_thread(id)?.unwrap_or(SharedThread {
            id: id.to_string(),
            title: None,
            title_at: 0,
            title_by: String::new(),
            status: None,
            status_at: 0,
            status_by: String::new(),
            updated: 0,
            moved_to: None,
            moved_tool: None,
            moved_at: 0,
            moved_by: String::new(),
        });
        t.moved_to = to.map(str::to_string);
        t.moved_tool = tool.map(str::to_string);
        t.moved_at = at;
        t.moved_by = device.to_string();
        self.merge_thread(&t)?;
        Ok(())
    }

    /// Merge one shared thread record. Returns true when anything changed.
    pub fn merge_thread(&self, t: &SharedThread) -> Result<bool> {
        let cur = self.shared_thread(&t.id)?;
        let mut out = cur.clone().unwrap_or(SharedThread {
            id: t.id.clone(),
            title: None,
            title_at: 0,
            title_by: String::new(),
            status: None,
            status_at: 0,
            status_by: String::new(),
            updated: 0,
            moved_to: None,
            moved_tool: None,
            moved_at: 0,
            moved_by: String::new(),
        });
        if newer(t.title_at, &t.title_by, out.title_at, &out.title_by) {
            out.title = t.title.as_deref().map(|s| clip(s, MAX_TITLE));
            out.title_at = t.title_at;
            out.title_by = t.title_by.clone();
        }
        if newer(t.status_at, &t.status_by, out.status_at, &out.status_by) {
            if let Some(s) = &t.status {
                if crate::threads::STATUSES.contains(&s.as_str()) {
                    out.status = Some(s.clone());
                    out.status_at = t.status_at;
                    out.status_by = t.status_by.clone();
                }
            }
        }
        if newer(t.moved_at, &t.moved_by, out.moved_at, &out.moved_by) {
            out.moved_to = t.moved_to.clone().filter(|d| !d.is_empty());
            out.moved_tool = t.moved_tool.clone().filter(|d| !d.is_empty());
            out.moved_at = t.moved_at;
            out.moved_by = t.moved_by.clone();
        }
        if cur.as_ref() == Some(&out) {
            return Ok(false);
        }
        out.updated = out.title_at.max(out.status_at).max(out.moved_at);
        self.conn.execute(
            "INSERT INTO shared_threads (id, title, title_at, title_by, status, status_at, status_by,
                 updated, moved_to, moved_tool, moved_at, moved_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET title = ?2, title_at = ?3, title_by = ?4,
                 status = ?5, status_at = ?6, status_by = ?7, updated = ?8,
                 moved_to = ?9, moved_tool = ?10, moved_at = ?11, moved_by = ?12",
            params![
                out.id, out.title, out.title_at, out.title_by, out.status, out.status_at,
                out.status_by, out.updated, out.moved_to, out.moved_tool, out.moved_at,
                out.moved_by
            ],
        )?;
        Ok(true)
    }

    /// Attach (or, with `removed`, detach) device `device`'s thread `part` to
    /// the shared thread `thread_id`.
    #[allow(clippy::too_many_arguments)]
    pub fn share_part(
        &self,
        device: &str,
        part: &str,
        thread_id: &str,
        thread_device: &str,
        removed: bool,
        by: &str,
        at: i64,
    ) -> Result<bool> {
        if !removed && part == thread_id {
            bail!("a thread can't be joined to itself");
        }
        self.merge_part(&SharedPart {
            device: device.to_string(),
            part: part.to_string(),
            thread_id: thread_id.to_string(),
            thread_device: thread_device.to_string(),
            removed,
            at,
            by: by.to_string(),
        })
    }

    /// Merge one part record. Returns true when anything changed.
    pub fn merge_part(&self, p: &SharedPart) -> Result<bool> {
        let cur: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT at, by FROM shared_thread_parts WHERE device = ?1 AND part = ?2",
                params![p.device, p.part],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((at, by)) = cur {
            if !newer(p.at, &p.by, at, &by) {
                return Ok(false);
            }
        }
        self.conn.execute(
            "INSERT INTO shared_thread_parts (device, part, thread_id, thread_device, removed, at, by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(device, part) DO UPDATE SET thread_id = ?3, thread_device = ?4,
                 removed = ?5, at = ?6, by = ?7",
            params![p.device, p.part, p.thread_id, p.thread_device, p.removed, p.at, p.by],
        )?;
        Ok(true)
    }

    /// Every attached part (not removed ones).
    pub fn shared_parts(&self) -> Result<Vec<SharedPart>> {
        let mut stmt = self.conn.prepare(
            "SELECT device, part, thread_id, thread_device, removed, at, by FROM shared_thread_parts
             WHERE removed = 0",
        )?;
        let rows = stmt.query_map([], row_part)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Add a note to a thread. Returns it.
    pub fn add_note(
        &self,
        thread_id: &str,
        kind: &str,
        text: &str,
        author: &str,
        device: &str,
    ) -> Result<ThreadNote> {
        if !NOTE_KINDS.contains(&kind) {
            bail!("unknown note kind `{kind}` (expected one of {NOTE_KINDS:?})");
        }
        let text = clip(text, MAX_NOTE);
        if text.is_empty() {
            bail!("a note needs some text");
        }
        let at = now_ms();
        let note = ThreadNote {
            id: format!("n_{}", random_suffix()),
            thread_id: thread_id.to_string(),
            kind: kind.to_string(),
            text,
            author: clip(author, 80),
            device: device.to_string(),
            created: at,
            removed: false,
            updated: at,
        };
        self.merge_note(&note)?;
        Ok(note)
    }

    /// Change a note's kind (e.g. a todo to done) or remove it.
    pub fn edit_note(
        &self,
        id: &str,
        kind: Option<&str>,
        removed: bool,
    ) -> Result<Option<ThreadNote>> {
        let Some(mut n) = self.note(id)? else {
            return Ok(None);
        };
        if let Some(k) = kind {
            if !NOTE_KINDS.contains(&k) {
                bail!("unknown note kind `{k}`");
            }
            n.kind = k.to_string();
        }
        n.removed = removed;
        n.updated = now_ms().max(n.updated + 1);
        self.merge_note(&n)?;
        Ok(Some(n))
    }

    fn note(&self, id: &str) -> Result<Option<ThreadNote>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, thread_id, kind, text, author, device, created, removed, updated
                 FROM thread_notes WHERE id = ?1",
                [id],
                row_note,
            )
            .optional()?)
    }

    /// Merge one note. Returns true when anything changed.
    pub fn merge_note(&self, n: &ThreadNote) -> Result<bool> {
        if !NOTE_KINDS.contains(&n.kind.as_str()) {
            return Ok(false);
        }
        if let Some(cur) = self.note(&n.id)? {
            if !newer(n.updated, &n.device, cur.updated, &cur.device) {
                return Ok(false);
            }
        }
        self.conn.execute(
            "INSERT INTO thread_notes (id, thread_id, kind, text, author, device, created, removed, updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET kind = ?3, removed = ?8, updated = ?9",
            params![
                n.id,
                n.thread_id,
                n.kind,
                clip(&n.text, MAX_NOTE),
                clip(&n.author, 80),
                n.device,
                n.created,
                n.removed,
                n.updated
            ],
        )?;
        Ok(true)
    }

    /// The notes on any of these threads, oldest first.
    pub fn notes_for(&self, thread_ids: &[String]) -> Result<Vec<ThreadNote>> {
        let mut out = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT id, thread_id, kind, text, author, device, created, removed, updated
             FROM thread_notes WHERE thread_id = ?1 AND removed = 0",
        )?;
        for id in thread_ids {
            let rows = stmt.query_map([id], row_note)?;
            for r in rows {
                out.push(r?);
            }
        }
        out.sort_by_key(|n| n.created);
        Ok(out)
    }

    /// Handoff markers (`Crossthreads thread: th_…@device`) that appeared in
    /// user messages indexed since the last call: `(conversation_id, thread
    /// id, device)`. A session that opens with a handoff from another device
    /// belongs to that device's thread.
    pub fn new_handoff_markers(&self) -> Result<Vec<(String, String, String)>> {
        let upto: i64 = self
            .meta_get(MARKER_SCAN_KEY)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut stmt = self.conn.prepare(
            "SELECT rowid, conversation_id, content FROM messages
             WHERE rowid > ?1 AND role = 'user' AND instr(content, ?2) > 0
             ORDER BY rowid LIMIT 500",
        )?;
        let rows = stmt
            .query_map(params![upto, HANDOFF_MARKER], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let max_rowid: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(rowid), 0) FROM messages", [], |r| {
                    r.get(0)
                })?;
        let mut out = Vec::new();
        let mut last = max_rowid;
        for (rowid, conv, content) in &rows {
            if let Some((id, device)) = parse_marker(content) {
                out.push((conv.clone(), id, device));
            }
            last = *rowid;
        }
        // A full page means there may be more: carry on from the last one seen.
        let next = if rows.len() == 500 { last } else { max_rowid };
        self.meta_set(MARKER_SCAN_KEY, &next.to_string())?;
        Ok(out)
    }

    /// The most recently active session that ran in `cwd` (or a folder
    /// containing it), as a conversation id.
    pub fn latest_conversation_in(&self, cwd: &str) -> Result<Option<String>> {
        let cwd = cwd.trim_end_matches(['/', '\\']);
        if cwd.is_empty() {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM conversations
                 WHERE project IS NOT NULL AND project != ''
                   AND (project = ?1 OR ?1 LIKE project || '/%' OR ?1 LIKE project || '\\%')
                 ORDER BY COALESCE(ended_at, started_at) DESC, length(project) DESC LIMIT 1",
                [cwd],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Everything changed after `since` (epoch ms), for another device.
    pub fn shared_since(&self, since: i64) -> Result<SharedState> {
        let mut st = SharedState::default();
        let mut stmt = self.conn.prepare(
            "SELECT id, title, title_at, title_by, status, status_at, status_by, updated,
                    moved_to, moved_tool, moved_at, moved_by
             FROM shared_threads WHERE updated > ?1 ORDER BY updated LIMIT ?2",
        )?;
        st.threads = stmt
            .query_map(params![since, MAX_BATCH as i64], row_thread)?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = self.conn.prepare(
            "SELECT device, part, thread_id, thread_device, removed, at, by FROM shared_thread_parts
             WHERE at > ?1 ORDER BY at LIMIT ?2",
        )?;
        st.parts = stmt
            .query_map(params![since, MAX_BATCH as i64], row_part)?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = self.conn.prepare(
            "SELECT id, thread_id, kind, text, author, device, created, removed, updated
             FROM thread_notes WHERE updated > ?1 ORDER BY updated LIMIT ?2",
        )?;
        st.notes = stmt
            .query_map(params![since, MAX_BATCH as i64], row_note)?
            .collect::<rusqlite::Result<_>>()?;
        // A full batch may have left later rows behind; only promise up to the
        // newest change every list got through.
        let mut upto = i64::MAX;
        let mut any = false;
        let mut bound = |full: bool, last: Option<i64>| {
            if let Some(l) = last {
                any = true;
                if full {
                    upto = upto.min(l);
                }
            }
        };
        bound(
            st.threads.len() >= MAX_BATCH,
            st.threads.last().map(|t| t.updated),
        );
        bound(st.parts.len() >= MAX_BATCH, st.parts.last().map(|p| p.at));
        bound(
            st.notes.len() >= MAX_BATCH,
            st.notes.last().map(|n| n.updated),
        );
        let newest = [
            st.threads.last().map(|t| t.updated),
            st.parts.last().map(|p| p.at),
            st.notes.last().map(|n| n.updated),
        ]
        .into_iter()
        .flatten()
        .max();
        st.upto = if !any {
            since
        } else if upto == i64::MAX {
            newest.unwrap_or(since)
        } else {
            // Something was cut off at `upto`: ask again from just before it so
            // nothing in between is lost (repeats are harmless).
            upto - 1
        };
        Ok(st)
    }

    /// Merge a batch from another device. Returns how many records changed.
    pub fn merge_shared(&self, st: &SharedState) -> Result<usize> {
        let mut changed = 0;
        for t in st.threads.iter().take(MAX_BATCH) {
            changed += self.merge_thread(t)? as usize;
        }
        for p in st.parts.iter().take(MAX_BATCH) {
            changed += self.merge_part(p)? as usize;
        }
        for n in st.notes.iter().take(MAX_BATCH) {
            changed += self.merge_note(n)? as usize;
        }
        Ok(changed)
    }
}

fn row_thread(r: &rusqlite::Row) -> rusqlite::Result<SharedThread> {
    Ok(SharedThread {
        id: r.get(0)?,
        title: r.get(1)?,
        title_at: r.get(2)?,
        title_by: r.get(3)?,
        status: r.get(4)?,
        status_at: r.get(5)?,
        status_by: r.get(6)?,
        updated: r.get(7)?,
        moved_to: r.get(8)?,
        moved_tool: r.get(9)?,
        moved_at: r.get(10)?,
        moved_by: r.get(11)?,
    })
}

fn row_part(r: &rusqlite::Row) -> rusqlite::Result<SharedPart> {
    Ok(SharedPart {
        device: r.get(0)?,
        part: r.get(1)?,
        thread_id: r.get(2)?,
        thread_device: r.get(3)?,
        removed: r.get(4)?,
        at: r.get(5)?,
        by: r.get(6)?,
    })
}

fn row_note(r: &rusqlite::Row) -> rusqlite::Result<ThreadNote> {
    Ok(ThreadNote {
        id: r.get(0)?,
        thread_id: r.get(1)?,
        kind: r.get(2)?,
        text: r.get(3)?,
        author: r.get(4)?,
        device: r.get(5)?,
        created: r.get(6)?,
        removed: r.get(7)?,
        updated: r.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn latest_edit_wins_per_field_whatever_the_order() {
        let a = store();
        let b = store();
        a.share_thread_edit("th_1", Some("Fix login"), None, "laptop", 100)
            .unwrap();
        b.share_thread_edit("th_1", None, Some("done"), "desktop", 200)
            .unwrap();
        b.share_thread_edit("th_1", Some("Old name"), None, "desktop", 50)
            .unwrap();
        // Exchange in both directions.
        let from_a = a.shared_since(0).unwrap();
        let from_b = b.shared_since(0).unwrap();
        a.merge_shared(&from_b).unwrap();
        b.merge_shared(&from_a).unwrap();
        for s in [&a, &b] {
            let t = s.shared_thread("th_1").unwrap().unwrap();
            assert_eq!(t.title.as_deref(), Some("Fix login"));
            assert_eq!(t.status.as_deref(), Some("done"));
        }
        // Merging again changes nothing.
        assert_eq!(a.merge_shared(&from_b).unwrap(), 0);
    }

    #[test]
    fn parts_and_notes_travel_and_removals_win() {
        let a = store();
        let b = store();
        a.share_part("desktop", "th_x", "th_1", "laptop", false, "laptop", 10)
            .unwrap();
        let n = a
            .add_note("th_1", "todo", "write the migration", "claude", "laptop")
            .unwrap();
        b.merge_shared(&a.shared_since(0).unwrap()).unwrap();
        assert_eq!(b.shared_parts().unwrap().len(), 1);
        assert_eq!(b.notes_for(&["th_1".into()]).unwrap().len(), 1);

        b.share_part("desktop", "th_x", "th_1", "laptop", true, "desktop", 20)
            .unwrap();
        b.edit_note(&n.id, Some("done"), false).unwrap();
        a.merge_shared(&b.shared_since(0).unwrap()).unwrap();
        assert!(a.shared_parts().unwrap().is_empty());
        assert_eq!(a.notes_for(&["th_1".into()]).unwrap()[0].kind, "done");
        assert!(a
            .share_part("d", "th_1", "th_1", "d", false, "d", 30)
            .is_err());
    }

    #[test]
    fn since_only_returns_newer_changes() {
        let a = store();
        a.share_thread_edit("th_1", Some("one"), None, "laptop", 100)
            .unwrap();
        let first = a.shared_since(0).unwrap();
        assert_eq!(first.upto, 100);
        assert!(a.shared_since(first.upto).unwrap().is_empty());
        a.share_thread_edit("th_2", Some("two"), None, "laptop", 150)
            .unwrap();
        assert_eq!(a.shared_since(first.upto).unwrap().threads.len(), 1);
    }

    #[test]
    fn reads_handoff_markers() {
        let line = handoff_marker("th_abc123", "work-laptop");
        assert_eq!(
            parse_marker(&format!("## Handoff\n- {line}\n")),
            Some(("th_abc123".into(), "work-laptop".into()))
        );
        assert_eq!(parse_marker("Crossthreads thread: nothing here"), None);
    }

    #[test]
    fn rejects_bad_input() {
        let a = store();
        assert!(a.add_note("th_1", "rant", "x", "me", "d").is_err());
        assert!(a.add_note("th_1", "todo", "   ", "me", "d").is_err());
        assert!(a
            .share_thread_edit("th_1", None, Some("lost"), "d", 1)
            .is_err());
        let long = "x".repeat(MAX_NOTE * 2);
        let n = a.add_note("th_1", "context", &long, "me", "d").unwrap();
        assert_eq!(n.text.chars().count(), MAX_NOTE);
    }
}
