//! A room's task board: one agent leads, others take tasks (SPEC §6.10).
//!
//! A task is a piece of the room's work with an owner (an agent name, or
//! `name@device` for an agent on another computer), a status and a result.
//! Tasks are shared with the user's other devices that may message, merged
//! like other shared state: the latest change (by time, then device) wins.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

use anyhow::{bail, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::Store;

/// What a task can be.
pub const TASK_STATUSES: &[&str] = &["todo", "doing", "blocked", "done", "cancelled"];
/// Limits on what agents write.
pub const MAX_TITLE: usize = 200;
pub const MAX_BRIEF: usize = 8_000;
pub const MAX_RESULT: usize = 8_000;
pub const MAX_FILES: usize = 50;

/// One task on a room's board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomTask {
    pub id: String,
    pub room: String,
    pub title: String,
    #[serde(default)]
    pub brief: String,
    /// Who it's assigned to; empty when nobody yet.
    #[serde(default)]
    pub owner: String,
    pub status: String,
    #[serde(default)]
    pub result: String,
    #[serde(default)]
    pub files: Vec<String>,
    /// The agent that planned it (the lead).
    #[serde(default)]
    pub created_by: String,
    /// The device it was planned on.
    #[serde(default)]
    pub device: String,
    pub created: i64,
    /// Epoch ms of the last change, and the device that made it.
    pub updated: i64,
    #[serde(default)]
    pub updated_by: String,
}

impl RoomTask {
    /// Still to be finished: not done, blocked or cancelled.
    pub fn open(&self) -> bool {
        matches!(self.status.as_str(), "todo" | "doing")
    }
}

/// A change to a task. `None` leaves a field as it is.
#[derive(Debug, Clone, Default)]
pub struct TaskChange<'a> {
    pub status: Option<&'a str>,
    pub result: Option<&'a str>,
    pub owner: Option<&'a str>,
    pub files: Option<&'a [String]>,
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

fn clean_files(files: &[String]) -> Vec<String> {
    files
        .iter()
        .map(|f| clip(f, 1000))
        .filter(|f| !f.is_empty())
        .take(MAX_FILES)
        .collect()
}

fn new_id() -> String {
    let mut h = RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    );
    format!("tk_{:010x}", h.finish() & 0xff_ffff_ffff)
}

impl Store {
    /// Add a task to a room's board.
    #[allow(clippy::too_many_arguments)]
    pub fn add_task(
        &self,
        room: &str,
        title: &str,
        brief: &str,
        owner: &str,
        files: &[String],
        created_by: &str,
        device: &str,
        at_ms: i64,
    ) -> Result<RoomTask> {
        let title = clip(title, MAX_TITLE);
        if title.is_empty() {
            bail!("a task needs a title");
        }
        let t = RoomTask {
            id: new_id(),
            room: room.to_string(),
            title,
            brief: clip(brief, MAX_BRIEF),
            owner: clip(owner, 120),
            status: "todo".into(),
            result: String::new(),
            files: clean_files(files),
            created_by: clip(created_by, 120),
            device: device.to_string(),
            created: at_ms,
            updated: at_ms,
            updated_by: device.to_string(),
        };
        self.merge_task(&t)?;
        Ok(t)
    }

    /// Change a task. Returns it, or `None` if there's no such task.
    pub fn change_task(
        &self,
        id: &str,
        change: &TaskChange,
        device: &str,
        at_ms: i64,
    ) -> Result<Option<RoomTask>> {
        let Some(mut t) = self.task(id)? else {
            return Ok(None);
        };
        if let Some(s) = change.status {
            if !TASK_STATUSES.contains(&s) {
                bail!("unknown status `{s}` (expected one of {TASK_STATUSES:?})");
            }
            t.status = s.to_string();
        }
        if let Some(r) = change.result {
            t.result = clip(r, MAX_RESULT);
        }
        if let Some(o) = change.owner {
            t.owner = clip(o, 120);
        }
        if let Some(f) = change.files {
            t.files = clean_files(f);
        }
        t.updated = at_ms.max(t.updated + 1);
        t.updated_by = device.to_string();
        self.merge_task(&t)?;
        Ok(Some(t))
    }

    /// Merge a task from here or another device. Returns true if it changed.
    pub fn merge_task(&self, t: &RoomTask) -> Result<bool> {
        if !TASK_STATUSES.contains(&t.status.as_str()) || t.id.is_empty() || t.id.len() > 64 {
            return Ok(false);
        }
        if let Some(cur) = self.task(&t.id)? {
            if (t.updated, &t.updated_by) <= (cur.updated, &cur.updated_by) {
                return Ok(false);
            }
        }
        self.conn.execute(
            "INSERT INTO room_tasks (id, room, title, brief, owner, status, result, files,
                 created_by, device, created, updated, updated_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET title = ?3, brief = ?4, owner = ?5, status = ?6,
                 result = ?7, files = ?8, updated = ?12, updated_by = ?13",
            params![
                t.id,
                t.room,
                clip(&t.title, MAX_TITLE),
                clip(&t.brief, MAX_BRIEF),
                clip(&t.owner, 120),
                t.status,
                clip(&t.result, MAX_RESULT),
                serde_json::to_string(&clean_files(&t.files))?,
                clip(&t.created_by, 120),
                t.device,
                t.created,
                t.updated,
                t.updated_by
            ],
        )?;
        Ok(true)
    }

    pub fn task(&self, id: &str) -> Result<Option<RoomTask>> {
        Ok(self
            .conn
            .query_row(&format!("{SELECT} WHERE id = ?1"), [id], row_task)
            .optional()?)
    }

    /// A room's tasks, oldest first.
    pub fn room_tasks(&self, room: &str) -> Result<Vec<RoomTask>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT} WHERE room = ?1 ORDER BY created, id"))?;
        let rows = stmt.query_map([room], row_task)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Tasks changed after `since` (epoch ms), for another device.
    pub fn tasks_since(&self, since: i64, limit: usize) -> Result<Vec<RoomTask>> {
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT} WHERE updated > ?1 ORDER BY updated LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![since, limit as i64], row_task)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

const SELECT: &str = "SELECT id, room, title, brief, owner, status, result, files, created_by,
    device, created, updated, updated_by FROM room_tasks";

fn row_task(r: &rusqlite::Row) -> rusqlite::Result<RoomTask> {
    let files: String = r.get(7)?;
    Ok(RoomTask {
        id: r.get(0)?,
        room: r.get(1)?,
        title: r.get(2)?,
        brief: r.get(3)?,
        owner: r.get(4)?,
        status: r.get(5)?,
        result: r.get(6)?,
        files: serde_json::from_str(&files).unwrap_or_default(),
        created_by: r.get(8)?,
        device: r.get(9)?,
        created: r.get(10)?,
        updated: r.get(11)?,
        updated_by: r.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_change_and_merge_between_devices() {
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();
        let t = a
            .add_task(
                "r1",
                "Write the tests",
                "cover retry",
                "codex@desk",
                &["/x/a.rs".into()],
                "claude",
                "lap",
                100,
            )
            .unwrap();
        assert_eq!(t.status, "todo");
        assert!(b.merge_task(&t).unwrap());
        assert!(!b.merge_task(&t).unwrap(), "same change twice is a no-op");

        let done = b
            .change_task(
                &t.id,
                &TaskChange {
                    status: Some("done"),
                    result: Some("12 tests pass"),
                    ..Default::default()
                },
                "desk",
                200,
            )
            .unwrap()
            .unwrap();
        assert!(a.merge_task(&done).unwrap());
        let seen = a.task(&t.id).unwrap().unwrap();
        assert_eq!(
            (seen.status.as_str(), seen.result.as_str()),
            ("done", "12 tests pass")
        );
        assert!(!seen.open());

        // An older change arriving late loses.
        assert!(!a.merge_task(&t).unwrap());
        assert_eq!(a.room_tasks("r1").unwrap().len(), 1);
        assert_eq!(a.tasks_since(150, 10).unwrap().len(), 1);
        assert!(a.tasks_since(10_000, 10).unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_input() {
        let a = Store::open_in_memory().unwrap();
        assert!(a.add_task("r", "  ", "", "", &[], "x", "d", 1).is_err());
        let t = a.add_task("r", "t", "", "", &[], "x", "d", 1).unwrap();
        assert!(a
            .change_task(
                &t.id,
                &TaskChange {
                    status: Some("lost"),
                    ..Default::default()
                },
                "d",
                2
            )
            .is_err());
        assert!(a
            .change_task("tk_nope", &TaskChange::default(), "d", 2)
            .unwrap()
            .is_none());
        let many: Vec<String> = (0..80).map(|i| format!("/f{i}")).collect();
        let t = a.add_task("r", "t", "", "", &many, "x", "d", 1).unwrap();
        assert_eq!(t.files.len(), MAX_FILES);
    }
}
