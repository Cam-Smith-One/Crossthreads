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
                    ..Default::default()
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

// ---- a worktree per task, and starting agents (with the user's approval) ----

/// The short name an agent of `tool` goes by in rooms.
fn agent_name(tool: &str) -> &'static str {
    match tool {
        "codex" => "codex",
        "grok-build" | "grok" => "grok",
        _ => "claude",
    }
}

/// Run git with arguments (never a shell). Ok(stdout) or Err(stderr).
fn git(dir: &std::path::Path, args: &[&str]) -> std::result::Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Where a task's worktree goes: in Crossthreads' own folder, not the
/// user's project, one folder per repository.
fn worktree_dir(root: &std::path::Path, id: &str) -> Option<std::path::PathBuf> {
    use sha2::{Digest, Sha256};
    let name = root.file_name()?.to_string_lossy().into_owned();
    let hash = Sha256::digest(root.to_string_lossy().as_bytes());
    let short: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();
    // CROSSTHREADS_WORKTREES moves them elsewhere (an absolute folder).
    let base = std::env::var_os("CROSSTHREADS_WORKTREES")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            Some(
                dirs::data_local_dir()?
                    .join("crossthreads")
                    .join("worktrees"),
            )
        })?;
    Some(base.join(format!("{name}-{short}")).join(id))
}

/// A task id is `tk_` and hex: safe in a path and a branch name.
fn valid_task_id(id: &str) -> bool {
    id.len() <= 32 && id.starts_with("tk_") && id[3..].chars().all(|c| c.is_ascii_hexdigit())
}

impl Daemon {
    /// `worktree`: give a task its own branch (`ct/<task id>`) and folder on
    /// this computer, or (`remove`) take the folder away again (the branch
    /// stays for the lead to merge).
    pub(crate) fn room_worktree(
        &self,
        me: &str,
        room: &str,
        id: &str,
        remove: bool,
    ) -> Result<String> {
        let Some(me) = norm(me) else {
            return Ok("Error: `me` is required.".into());
        };
        let id = id.trim();
        if !valid_task_id(id) {
            return Ok(format!("Error: `{id}` isn't a task id."));
        }
        let dir = std::path::Path::new(room.trim());
        if !dir.is_dir() {
            return Ok("Error: pass your repository's root folder as `room` (git rev-parse --show-toplevel).".into());
        }
        let root = match git(dir, &["rev-parse", "--show-toplevel"]) {
            Ok(r) => std::path::PathBuf::from(r),
            Err(e) => {
                return Ok(format!(
                    "Error: {} isn't a git repository: {e}",
                    dir.display()
                ))
            }
        };
        let Some(task) = self.read_lock().task(id)? else {
            return Ok(format!("Error: no task {id}."));
        };
        if task.room != room_id(room) {
            return Ok(format!("Error: task {id} belongs to another room."));
        }
        let device = self.device_name();
        let branch = format!("ct/{id}");
        let Some(path) = worktree_dir(&root, id) else {
            return Ok("Error: couldn't find a folder for worktrees on this computer.".into());
        };
        let path_s = path.to_string_lossy().into_owned();

        if remove {
            if path.exists() {
                if let Err(e) = git(&root, &["worktree", "remove", &path_s]) {
                    return Ok(format!(
                        "Couldn't remove the worktree (uncommitted changes?): {e}\nCommit or discard them in {path_s} first."
                    ));
                }
            }
            self.write_lock().change_task(
                id,
                &TaskChange {
                    worktree: Some((&branch, "", "")),
                    ..Default::default()
                },
                &device,
                ms(),
            )?;
            self.rooms_changed();
            self.sync_soon();
            return Ok(format!(
                "Removed the worktree for {id}. Branch {branch} is kept for merging."
            ));
        }

        if !path.join(".git").exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let have_branch = git(
                &root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ],
            )
            .is_ok();
            let made = if have_branch {
                git(&root, &["worktree", "add", &path_s, &branch])
            } else {
                git(&root, &["worktree", "add", "-b", &branch, &path_s, "HEAD"])
            };
            if let Err(e) = made {
                return Ok(format!("Couldn't create the worktree: {e}"));
            }
        }
        self.write_lock().change_task(
            id,
            &TaskChange {
                worktree: Some((&branch, &path_s, &device)),
                ..Default::default()
            },
            &device,
            ms(),
        )?;
        self.rooms_changed();
        self.sync_soon();
        Ok(format!(
            "Task {id} has its own worktree at {path_s} on branch {branch} (from the current HEAD of {}).\n\
             Work and commit there (not in the main checkout); other agents can't collide with you. \
             When done, mark the task done; {} reviews and merges {branch}.",
            root.display(),
            if task.created_by.is_empty() || task.created_by == me { "the lead".to_string() } else { task.created_by.clone() }
        ))
    }

    /// `request_agent`: ask for an agent to be started for a task, in `tool`,
    /// on `device` (default: this one). Nothing starts until the user allows
    /// it in the app on that device.
    pub(crate) fn room_request_agent(
        &self,
        me: &str,
        id: &str,
        tool: &str,
        device: Option<&str>,
    ) -> Result<String> {
        let Some(_me) = norm(me) else {
            return Ok("Error: `me` is required.".into());
        };
        let tool = match tool.trim() {
            "claude" | "claude-code" => "claude-code",
            "codex" => "codex",
            "grok" | "grok-build" => "grok-build",
            other => {
                return Ok(format!(
                    "Error: start claude-code, codex or grok-build, not `{other}`."
                ))
            }
        };
        let here = self.device_name();
        let device = device
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or(&here)
            .to_string();
        if device != here
            && !self
                .federation
                .as_ref()
                .is_some_and(|f| f.room_peer(&device).is_some())
        {
            return Ok(format!(
                "Error: `{device}` isn't a device whose agents may message this one."
            ));
        }
        let owner = if device == here {
            agent_name(tool).to_string()
        } else {
            format!("{}@{device}", agent_name(tool))
        };
        let Some(t) = self.write_lock().change_task(
            id.trim(),
            &TaskChange {
                owner: Some(&owner),
                launch: Some((tool, &device, "requested")),
                ..Default::default()
            },
            &here,
            ms(),
        )?
        else {
            return Ok(format!("Error: no task {id}."));
        };
        self.rooms_changed();
        self.sync_soon();
        Ok(format!(
            "Asked to start {} on {device} for {} (it will be `{owner}`). The user sees the request in \
             Crossthreads on {device} and decides; carry on, and crossthreads_gather will show when it's done.",
            crate::launch::Tool::from_id(tool).map(|t| t.display_name()).unwrap_or(tool),
            t.id
        ))
    }

    /// The user's answer, on this device, to a request to start an agent.
    pub(crate) fn room_launch(&self, id: &str, allow: bool) -> Result<String> {
        let here = self.device_name();
        let Some(t) = self.read_lock().task(id.trim())? else {
            anyhow::bail!("no task {id}");
        };
        if t.launch_state != "requested" || t.launch_device != here {
            anyhow::bail!("there's no request waiting on this computer for {id}");
        }
        let mut message = String::new();
        let state = if allow {
            let Some(tool) = crate::launch::Tool::from_id(&t.launch_tool) else {
                anyhow::bail!("unknown tool `{}`", t.launch_tool);
            };
            let Some(cwd) = self.room_folder(&t.room) else {
                anyhow::bail!(
                    "couldn't find this repository on this computer: open a session in it here once, then try again"
                );
            };
            let name = agent_name(&t.launch_tool);
            let mut prompt = format!(
                "You're {} on {here}, joining a crew in Crossthreads. The lead is {}.\n\
                 Your name in the room is `{name}`. Your task {}: {}\n",
                tool.display_name(),
                if t.device == here || t.device.is_empty() {
                    t.created_by.clone()
                } else {
                    format!("{}@{}", t.created_by, t.device)
                },
                t.id,
                t.title
            );
            if !t.brief.is_empty() {
                prompt.push_str(&format!("{}\n", t.brief));
            }
            if !t.files.is_empty() {
                prompt.push_str(&format!("Files: {}\n", t.files.join(", ")));
            }
            prompt.push_str(&format!(
                "\nUse the Crossthreads room for this repository (room = the repository root): \
                 call crossthreads_task(me=\"{name}\", room, id=\"{}\", status=\"doing\"), do the work \
                 (call crossthreads_worktree for the task's own branch if several agents share this \
                 checkout), then status=\"done\" with a short result, and keep listening with \
                 crossthreads_room_wait.",
                t.id
            ));
            message = crate::launch::launch(&crate::launch::LaunchPlan {
                tool,
                cwd,
                prompt: Some(prompt),
                resume: None,
            })?;
            "started"
        } else {
            "declined"
        };
        let (tool, device) = (t.launch_tool.clone(), t.launch_device.clone());
        let note = {
            let store = self.write_lock();
            store.change_task(
                &t.id,
                &TaskChange {
                    launch: Some((&tool, &device, state)),
                    ..Default::default()
                },
                &here,
                ms(),
            )?;
            let lead = if t.device == here || t.device.is_empty() {
                t.created_by.clone()
            } else {
                format!("{}@{}", t.created_by, t.device)
            };
            let text = format!(
                "The user {} starting {} on {here} for {}.",
                if allow { "allowed" } else { "declined" },
                agent_name(&tool),
                t.id
            );
            store.room_post(&t.room, "you", &text, Some(&lead), true, now())?
        };
        self.rooms_changed();
        self.relay(note);
        self.sync_soon();
        Ok(if allow {
            message
        } else {
            format!("Declined starting {} for {}.", agent_name(&tool), t.id)
        })
    }

    /// This computer's folder for a room: the room itself when it's a folder,
    /// else the folder of a recent session in that repository.
    fn room_folder(&self, room: &str) -> Option<std::path::PathBuf> {
        let p = std::path::Path::new(room);
        if p.is_absolute() && p.is_dir() {
            return Some(p.to_path_buf());
        }
        let mut sessions: Vec<ct_store::SessionRef> = self
            .read_lock()
            .threads_with_sessions(&ct_store::ThreadFilters::default())
            .ok()?
            .into_iter()
            .flat_map(|(_, s)| s)
            .filter(|s| s.repo_key.as_deref() == Some(room))
            .collect();
        sessions.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
        sessions
            .into_iter()
            .filter_map(|s| s.project)
            .map(std::path::PathBuf::from)
            .find(|p| p.is_dir())
    }
}
