//! Crews: one agent leads, others take tasks (SPEC §6.10).
//!
//! The lead puts tasks on the room's board with `plan`, each assigned to an
//! agent here (`codex`) or on another device (`codex@desk`). Each assignee is
//! sent its brief as a direct message. Workers move their task along with
//! `task` (doing, blocked, done, with a result), which also tells the lead. The
//! lead waits with `gather` until its tasks are all done or blocked. The board
//! travels to the user's other devices that may message.

use std::time::{Duration, Instant};

use anyhow::Result;
use ct_store::{RoomTask, TaskChange};
use serde::{Deserialize, Serialize};

use super::{norm, now, room_id, room_label, DEFAULT_WAIT};
use crate::Daemon;

/// A task as the lead writes it in `plan`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub title: String,
    #[serde(default)]
    pub brief: String,
    /// Who should do it: an agent name, or `name@device`.
    #[serde(default)]
    pub to: Option<String>,
    /// Files it covers (absolute paths), so workers know what to claim.
    #[serde(default)]
    pub files: Vec<String>,
}

/// At most this many tasks in one `plan`.
const MAX_PLAN: usize = 25;

fn ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// How a task reads on the board.
fn board_line(t: &RoomTask) -> String {
    let owner = if t.owner.is_empty() {
        "unassigned".to_string()
    } else {
        format!("→ {}", t.owner)
    };
    let mut s = format!("- {} [{}] {} {owner}", t.id, t.status, t.title);
    if !t.files.is_empty() {
        s.push_str(&format!(" (files: {})", t.files.join(", ")));
    }
    if !t.result.is_empty() {
        let r: String = t.result.chars().take(300).collect();
        s.push_str(&format!("\n    result: {}", r.replace('\n', " ")));
    }
    s
}

impl Daemon {
    /// The board changed: wake anyone waiting and refresh the app.
    pub(crate) fn rooms_changed(&self) {
        self.rooms.notify();
    }

    /// The board as text, for agents.
    pub(crate) fn room_board(&self, room: &str) -> Result<String> {
        let room = room_id(room);
        let tasks = self.read_lock().room_tasks(&room)?;
        Ok(board_text(&room, &tasks))
    }

    /// `plan`: put tasks on the board and send each assignee its brief.
    pub(crate) fn room_plan(&self, me: &str, room: &str, tasks: &[TaskSpec]) -> Result<String> {
        let Some(me) = norm(me) else {
            return Ok("Error: `me` is required.".into());
        };
        if tasks.is_empty() {
            return Ok("Error: give at least one task.".into());
        }
        if tasks.len() > MAX_PLAN {
            return Ok(format!("Error: at most {MAX_PLAN} tasks in one plan."));
        }
        let room = room_id(room);
        let device = self.device_name();
        let mut created = Vec::new();
        let mut sent = Vec::new();
        {
            let store = self.write_lock();
            for spec in tasks {
                let owner = spec.to.as_deref().and_then(norm).unwrap_or_default();
                let t = store.add_task(
                    &room,
                    &spec.title,
                    &spec.brief,
                    &owner,
                    &spec.files,
                    &me,
                    &device,
                    ms(),
                )?;
                if !owner.is_empty() {
                    let mut text = format!("Task {} for you: {}", t.id, t.title);
                    if !t.brief.is_empty() {
                        text.push_str(&format!("\n{}", t.brief));
                    }
                    if !t.files.is_empty() {
                        text.push_str(&format!(
                            "\nFiles: {} (claim them before editing).",
                            t.files.join(", ")
                        ));
                    }
                    text.push_str(&format!(
                        "\nWhen you start, call crossthreads_task(me, room, id=\"{}\", status=\"doing\"); \
                         when finished, status=\"done\" with a short result (or \"blocked\" with why).",
                        t.id
                    ));
                    let id = store.room_post(&room, &me, &text, Some(&owner), false, now())?;
                    sent.push(id);
                }
                created.push(t);
            }
            store.room_touch(&room, &me, now())?;
        }
        self.rooms_changed();
        for id in sent {
            self.relay(id);
        }
        self.sync_soon();
        let all = self.read_lock().room_tasks(&room)?;
        Ok(format!(
            "Planned {} task(s) in '{}'.\n{}\nCall crossthreads_gather(me, room) to wait for them.",
            created.len(),
            room_label(&room),
            board_text(&room, &all)
        ))
    }

    /// `task`: move a task along (status, result, owner, files).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn room_task_update(
        &self,
        me: &str,
        room: &str,
        id: &str,
        status: Option<&str>,
        result: Option<&str>,
        owner: Option<&str>,
        files: Option<&[String]>,
    ) -> Result<String> {
        let Some(me) = norm(me) else {
            return Ok("Error: `me` is required.".into());
        };
        let device = self.device_name();
        let owner_norm = owner.map(|o| norm(o).unwrap_or_default());
        let (t, note) = {
            let store = self.write_lock();
            let Some(t) = store.change_task(
                id.trim(),
                &TaskChange {
                    status: status.map(str::trim),
                    result,
                    owner: owner_norm.as_deref(),
                    files,
                },
                &device,
                ms(),
            )?
            else {
                return Ok(format!("Error: no task {id}."));
            };
            // Tell the lead (here, or on the device it planned from).
            let lead = if t.device == device || t.device.is_empty() {
                t.created_by.clone()
            } else {
                format!("{}@{}", t.created_by, t.device)
            };
            let mut note = None;
            if !lead.is_empty() && lead != me && (status.is_some() || result.is_some()) {
                let mut text = format!("{} is {}: {}", t.id, t.status, t.title);
                if !t.result.is_empty() {
                    text.push_str(&format!("\n{}", t.result));
                }
                let room_key = if room.trim().is_empty() {
                    t.room.clone()
                } else {
                    room_id(room)
                };
                note = Some(store.room_post(&room_key, &me, &text, Some(&lead), false, now())?);
                store.room_touch(&room_key, &me, now())?;
            }
            (t, note)
        };
        self.rooms_changed();
        if let Some(id) = note {
            self.relay(id);
        }
        self.sync_soon();
        Ok(format!("Updated.\n{}", board_line(&t)))
    }

    /// `gather`: wait until every task `me` planned in the room is done,
    /// blocked or cancelled, or `timeout` seconds pass. Returns the messages
    /// that came in meanwhile and the board.
    pub(crate) fn room_gather(&self, me: &str, room: &str, timeout: u64) -> Result<String> {
        let Some(me) = norm(me) else {
            return Ok("Error: `me` is required.".into());
        };
        let room = room_id(room);
        let device = self.device_name();
        let timeout = if timeout == 0 { DEFAULT_WAIT } else { timeout };
        let deadline = Instant::now() + Duration::from_secs(timeout.min(super::MAX_WAIT));
        let mine = |tasks: &[RoomTask]| -> Vec<RoomTask> {
            tasks
                .iter()
                .filter(|t| t.created_by == me && (t.device == device || t.device.is_empty()))
                .cloned()
                .collect()
        };
        let mut got: Vec<String> = Vec::new();
        loop {
            let tasks = mine(&self.read_lock().room_tasks(&room)?);
            if tasks.is_empty() {
                return Ok(format!(
                    "You have no tasks planned in '{}'. Plan some with crossthreads_plan.",
                    room_label(&room)
                ));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if tasks.iter().all(|t| !t.open()) || left.is_zero() {
                let open: Vec<String> = tasks
                    .iter()
                    .filter(|t| t.open())
                    .map(|t| {
                        format!(
                            "{} ({}, {})",
                            t.id,
                            if t.owner.is_empty() {
                                "unassigned"
                            } else {
                                &t.owner
                            },
                            t.status
                        )
                    })
                    .collect();
                let mut out = Vec::new();
                if !got.is_empty() {
                    out.push(got.join("\n"));
                }
                out.push(board_text(&room, &tasks));
                out.push(if open.is_empty() {
                    "All your tasks are finished. Check the work (git diff, the files), then send \
                     follow-ups or finish with a message starting DONE."
                        .to_string()
                } else {
                    format!(
                        "Still waiting on: {}. Call crossthreads_gather(me, room) again to keep waiting.",
                        open.join(", ")
                    )
                });
                return Ok(out.join("\n\n"));
            }
            let slice = left.as_secs().clamp(1, 15);
            let text = self.collect(&room, &me, slice)?;
            if !text.is_empty() {
                got.push(text);
            }
        }
    }
}

/// The board as text, for agents.
pub(crate) fn board_text(room: &str, tasks: &[RoomTask]) -> String {
    if tasks.is_empty() {
        return format!("No tasks in '{}' yet.", room_label(room));
    }
    let done = tasks.iter().filter(|t| t.status == "done").count();
    let mut s = format!(
        "Tasks in '{}' ({done}/{} done):\n",
        room_label(room),
        tasks.len()
    );
    s.push_str(&tasks.iter().map(board_line).collect::<Vec<_>>().join("\n"));
    s
}
