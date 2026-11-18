//! Starting, continuing and moving work from the app (SPEC §6.9).
//!
//! - **New thread**: a name, an optional goal, and the folder it's in.
//! - **Continue in Claude Code / Codex**: opens a terminal in the thread's
//!   folder on this computer, resuming the latest session when it was in that
//!   tool, else starting a new one briefed with the thread's handoff. The
//!   brief carries the thread's marker, so the new session joins the thread.
//! - **Send to another computer**: marks the thread for that computer, where it
//!   shows at the top of Home until someone continues it there. Nothing is
//!   started remotely.
//!
//! Only this computer's own clients can do any of this: none of it is a peer
//! request, and phones can't (see `phone::allowed`).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::launch::{LaunchPlan, Tool};
use crate::{Daemon, Response};

impl Daemon {
    pub(crate) fn create_thread(
        &self,
        title: &str,
        goal: Option<&str>,
        cwd: &str,
    ) -> Result<Response> {
        let dir = PathBuf::from(cwd.trim());
        let dir = dir
            .canonicalize()
            .with_context(|| format!("no folder {}", dir.display()))?;
        if !dir.is_dir() {
            bail!("{} isn't a folder", dir.display());
        }
        let project = dir.to_string_lossy().into_owned();
        let repo_key = ct_core::repo::repo_key_for(&project);
        let id = {
            let store = self.write_lock();
            let id = store.create_thread(title, goal, &project, repo_key.as_deref())?;
            if let Some(g) = goal.map(str::trim).filter(|g| !g.is_empty()) {
                store.add_note(
                    &id,
                    "context",
                    &format!("Goal: {g}"),
                    "you",
                    &self.device_name(),
                )?;
            }
            id
        };
        self.sync_soon();
        Ok(Response::ThreadId { id: Some(id) })
    }

    pub(crate) fn recent_folders(&self) -> Result<Response> {
        let mut sessions: Vec<ct_store::SessionRef> = self
            .read_lock()
            .threads_with_sessions(&ct_store::ThreadFilters::default())?
            .into_iter()
            .flat_map(|(_, s)| s)
            .collect();
        sessions.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
        let mut folders: Vec<String> = Vec::new();
        for s in sessions {
            let Some(p) = s.project.filter(|p| !p.is_empty()) else {
                continue;
            };
            if !folders.contains(&p) && Path::new(&p).is_dir() {
                folders.push(p);
            }
            if folders.len() == 15 {
                break;
            }
        }
        Ok(Response::Folders { folders })
    }

    /// Open `tool` in a terminal to continue a thread on this computer.
    pub(crate) fn continue_thread(
        &self,
        id: &str,
        device: Option<&str>,
        tool: &str,
        fresh: bool,
    ) -> Result<Response> {
        let Some(tool) = Tool::from_id(tool) else {
            bail!("continue in claude-code, codex or grok-build, not `{tool}`");
        };
        let Some(t) = self.whole_thread(id, device)? else {
            bail!("no thread {id}");
        };
        let me = self.device_name();
        let here = |s: &&ct_store::SessionRef| s.device.as_deref().map_or(true, |d| d == me);
        let latest = t
            .sessions
            .iter()
            .max_by(|a, b| a.last_activity.cmp(&b.last_activity));
        let latest_here = t
            .sessions
            .iter()
            .filter(here)
            .max_by(|a, b| a.last_activity.cmp(&b.last_activity));

        let cwd = self
            .folder_for(&t, latest_here)
            .context("this thread's folder isn't on this computer (open a session in it here first, or continue it on the computer that has it)")?;

        // Pick up where it was when the latest session overall was here, in
        // this tool; otherwise start fresh with the handoff.
        let resume = match (latest, latest_here) {
            (Some(l), Some(h))
                if !fresh
                    && l.conversation_id == h.conversation_id
                    && Tool::from_id(&h.tool) == Some(tool) =>
            {
                crate::resume::native_id(&h.tool, &h.source_path)
            }
            _ => None,
        };
        let prompt = if resume.is_some() {
            None
        } else {
            Some(self.brief_for(&t)?)
        };
        let text = crate::launch::launch(&LaunchPlan {
            tool,
            cwd,
            prompt,
            resume,
        })?;

        // Picked up here: it's no longer waiting for this computer.
        if t.summary.moved_to.as_deref() == Some(me.as_str()) {
            self.write_lock().share_thread_move(
                &t.summary.id,
                None,
                None,
                &me,
                ct_store::shared::now_ms(),
            )?;
            self.sync_soon();
        }
        Ok(Response::Text { text })
    }

    /// Mark a thread for another computer (or `to` = None to clear it).
    pub(crate) fn move_thread(
        &self,
        id: &str,
        device: Option<&str>,
        to: Option<&str>,
        tool: Option<&str>,
    ) -> Result<Response> {
        if let Some(d) = to {
            let known = self
                .federation
                .as_ref()
                .is_some_and(|f| f.peers().iter().any(|p| p.name == d) || f.device() == d);
            if !known {
                bail!("no device named `{d}` (add it in Devices first)");
            }
        }
        if let Some(t) = tool {
            if Tool::from_id(t).is_none() {
                bail!("continue in claude-code, codex or grok-build, not `{t}`");
            }
        }
        let (_, root) = self.resolve_root(device, id);
        self.write_lock().share_thread_move(
            &root,
            to,
            tool,
            &self.device_name(),
            ct_store::shared::now_ms(),
        )?;
        self.sync_soon();
        Ok(Response::ThreadId { id: Some(root) })
    }

    /// Where to run a tool for this thread on this computer: the folder of
    /// its latest session here, else the folder it was started in, else any
    /// folder here with the same repository.
    fn folder_for(
        &self,
        t: &ct_store::ThreadDetail,
        latest_here: Option<&ct_store::SessionRef>,
    ) -> Option<PathBuf> {
        let exists = |p: &str| Path::new(p).is_dir().then(|| PathBuf::from(p));
        if let Some(p) = latest_here
            .and_then(|s| s.project.as_deref())
            .and_then(exists)
        {
            return Some(p);
        }
        let store = self.read_lock();
        if let Ok(Some((p, _))) = store.thread_origin(&t.summary.id) {
            if let Some(p) = exists(&p) {
                return Some(p);
            }
        }
        let repo = t.summary.repo_key.as_deref()?;
        store
            .threads_with_sessions(&ct_store::ThreadFilters::default())
            .ok()?
            .into_iter()
            .flat_map(|(_, sessions)| sessions)
            .filter(|s| s.repo_key.as_deref() == Some(repo))
            .filter_map(|s| s.project.as_deref().and_then(exists))
            .next()
    }

    /// The prompt a new session starts with: the thread's handoff when it has
    /// sessions, else its goal and notes. Either way it carries the marker
    /// that puts the new session in the thread.
    fn brief_for(&self, t: &ct_store::ThreadDetail) -> Result<String> {
        let home = t
            .summary
            .device
            .clone()
            .unwrap_or_else(|| self.device_name());
        if !t.sessions.is_empty() {
            if let Ok(Response::Context { markdown, .. }) =
                self.thread_handoff(&t.summary.id, Some(&home), 8000)
            {
                return Ok(markdown);
            }
        }
        let mut md = format!("## {}\n\n", t.summary.title);
        if let Ok(Some((_, Some(goal)))) = self.read_lock().thread_origin(&t.summary.id) {
            md.push_str(&format!("Goal: {goal}\n\n"));
        }
        for n in t.notes.iter().filter(|n| n.kind != "done") {
            if !n.text.starts_with("Goal: ") {
                md.push_str(&format!("- {}: {}\n", n.kind, n.text));
            }
        }
        md.push_str(
            "\nThis is a new piece of work tracked in Crossthreads. Look at the repository, then \
             start on the goal. Record decisions and open todos with crossthreads_note.\n",
        );
        md.push_str(&format!(
            "\n---\n{}\n",
            ct_store::shared::handoff_marker(&t.summary.id, &home)
        ));
        Ok(md)
    }
}
