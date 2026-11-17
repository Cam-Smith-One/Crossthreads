//! Threads that span devices, and the shared context agents get from them
//! (SPEC §6.7).
//!
//! The store keeps the shared state (names, status, parts, notes) and merges
//! it; this module decides what that means for the board and for agents, and
//! copies the state between the user's devices: every [`SYNC_EVERY`], and
//! straight away after a change made here.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use ct_store::{PartRef, SharedState, ThreadDetail, ThreadSummary};

use crate::live::urgency;
use crate::{Client, Daemon, Request, Response};

/// How often devices compare shared thread state.
const SYNC_EVERY: Duration = Duration::from_secs(15);
/// Where "send agents context at session start" is kept.
const AGENT_CONTEXT_KEY: &str = "agent_context";
/// Branches too generic to say two threads are the same work.
const GENERIC_BRANCHES: &[&str] = &["main", "master", "trunk", "develop", "dev", "HEAD"];
/// How close in time two threads must be to look like the same work.
const RELATED_WINDOW_HOURS: i64 = 48;
/// The longest brief handed to an agent at session start.
const BRIEF_CHARS: usize = 1500;

impl Daemon {
    /// This device's name: the federation name, or a stand-in when this
    /// device isn't connected to others.
    pub(crate) fn device_name(&self) -> String {
        self.federation
            .as_ref()
            .map(|f| f.device())
            .unwrap_or_else(|| "this-device".to_string())
    }

    /// The thread `id` on `device` belongs to: itself, or the thread it was
    /// joined to. Returns `(device, id)`.
    pub(crate) fn resolve_root(&self, device: Option<&str>, id: &str) -> (String, String) {
        let device = device
            .map(str::to_string)
            .unwrap_or_else(|| self.device_name());
        let parts = self.read_lock().shared_parts().unwrap_or_default();
        match parts.iter().find(|p| p.device == device && p.part == id) {
            Some(p) => (p.thread_device.clone(), p.thread_id.clone()),
            None => (device, id.to_string()),
        }
    }

    /// Fold threads that were joined into one row per piece of work, then
    /// note which remaining threads look like the same work on another
    /// device. Rows must have `device` set when the list spans devices.
    pub(crate) fn fold_threads(&self, rows: Vec<ThreadSummary>) -> Vec<ThreadSummary> {
        let me = self.device_name();
        let (parts, shared) = {
            let store = self.read_lock();
            (
                store.shared_parts().unwrap_or_default(),
                store.shared_threads().unwrap_or_default(),
            )
        };
        let root_of: HashMap<(String, String), (String, String)> = parts
            .into_iter()
            .map(|p| ((p.device, p.part), (p.thread_device, p.thread_id)))
            .collect();
        let shared: HashMap<String, ct_store::SharedThread> =
            shared.into_iter().map(|t| (t.id.clone(), t)).collect();

        let mut order: Vec<(String, String)> = Vec::new();
        let mut groups: HashMap<(String, String), Vec<ThreadSummary>> = HashMap::new();
        for row in rows {
            let dev = row.device.clone().unwrap_or_else(|| me.clone());
            let key = root_of
                .get(&(dev.clone(), row.id.clone()))
                .cloned()
                .unwrap_or((dev, row.id.clone()));
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(row);
        }

        let mut out: Vec<ThreadSummary> = order
            .into_iter()
            .filter_map(|key| {
                let rows = groups.remove(&key)?;
                Some(merge_group(&key, rows, shared.get(&key.1), &me))
            })
            .collect();
        mark_related(&mut out, &me);
        out.sort_by(|a, b| {
            (b.last_activity.is_some(), &b.last_activity, &b.id).cmp(&(
                a.last_activity.is_some(),
                &a.last_activity,
                &a.id,
            ))
        });
        out
    }

    /// One thread with every part of it, from whichever devices have them,
    /// plus its notes. `id` may be a part; the whole thread comes back.
    pub(crate) fn whole_thread(
        &self,
        id: &str,
        device: Option<&str>,
    ) -> Result<Option<ThreadDetail>> {
        let (root_dev, mut root) = self.resolve_root(device, id);
        let me = self.device_name();
        // `id` may be a session in the thread rather than the thread itself.
        let first = self.thread_on(&root, Some(&root_dev));
        if let Ok(Some(t)) = &first {
            if t.summary.id != root {
                let real = t.summary.id.clone();
                let (d, i) = self.resolve_root(Some(&root_dev), &real);
                if (d.as_str(), i.as_str()) != (root_dev.as_str(), real.as_str()) {
                    return self.whole_thread(&i, Some(&d));
                }
                root = real;
            }
        }
        let parts: Vec<ct_store::SharedPart> = self
            .read_lock()
            .shared_parts()?
            .into_iter()
            .filter(|p| p.thread_id == root && p.thread_device == root_dev)
            .collect();

        let mut base = match first {
            Ok(t) => t,
            Err(e) if !parts.is_empty() => {
                eprintln!("warn: thread {root} on {root_dev}: {e:#}");
                None
            }
            Err(e) => return Err(e),
        };
        if let Some(t) = &mut base {
            mark_sessions(t, &root_dev);
        }
        let mut refs = Vec::new();
        for p in &parts {
            let found = match self.thread_on(&p.part, Some(&p.device)) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("warn: part {} on {}: {e:#}", p.part, p.device);
                    None
                }
            };
            let Some(mut t) = found else { continue };
            mark_sessions(&mut t, &p.device);
            refs.push(PartRef {
                device: p.device.clone(),
                id: p.part.clone(),
                title: t.summary.title.clone(),
            });
            base = Some(match base {
                None => {
                    let mut t = t;
                    t.summary.id = root.clone();
                    t.summary.device = Some(root_dev.clone());
                    t
                }
                Some(mut b) => {
                    let s = std::mem::replace(&mut b.summary, t.summary.clone());
                    b.summary = merge_group(
                        &(root_dev.clone(), root.clone()),
                        vec![s, t.summary],
                        None,
                        &me,
                    );
                    b.sessions.extend(t.sessions);
                    b.links.extend(t.links);
                    b.suggestions.extend(t.suggestions);
                    b
                }
            });
        }
        let Some(mut t) = base else { return Ok(None) };
        t.sessions.sort_by(|a, b| {
            (&a.started_at, &a.conversation_id).cmp(&(&b.started_at, &b.conversation_id))
        });
        let store = self.read_lock();
        if let Some(sh) = store.shared_thread(&root)? {
            if sh.title_at > 0 {
                if let Some(title) = sh.title {
                    t.summary.title = title;
                }
            }
            if sh.status_at > 0 {
                if let Some(status) = sh.status {
                    t.summary.status = status;
                }
            }
        }
        t.notes = store.notes_for(&[root.clone()])?;
        t.parts = refs;
        t.summary.id = root;
        t.summary.device = Some(root_dev);
        Ok(Some(t))
    }

    /// Rename a thread or change its status, wherever it lives. The change is
    /// kept here and sent to the other devices; the device that has the
    /// thread applies it to its own copy too.
    pub(crate) fn edit_thread(
        &self,
        id: &str,
        device: Option<&str>,
        title: Option<&str>,
        status: Option<&str>,
    ) -> Result<Response> {
        let (root_dev, mut root) = self.resolve_root(device, id);
        let me = self.device_name();
        if root_dev == me {
            // A session id names its thread.
            if let Some(t) = self.read_lock().thread(&root)? {
                root = t.summary.id;
            }
        }
        let local = {
            let store = self.write_lock();
            store.share_thread_edit(&root, title, status, &me, ct_store::shared::now_ms())?;
            if root_dev == me {
                store.set_thread(&root, title, status)?
            } else {
                Some(root.clone())
            }
        };
        self.sync_soon();
        Ok(Response::ThreadId { id: local })
    }

    /// Join `part` (on `device`) to `into` (on `into_device`).
    pub(crate) fn join_thread(
        &self,
        part: &str,
        device: Option<&str>,
        into: &str,
        into_device: Option<&str>,
    ) -> Result<Response> {
        let me = self.device_name();
        let part_dev = device.map(str::to_string).unwrap_or_else(|| me.clone());
        let (into_dev, into) = self.resolve_root(into_device, into);
        if part_dev == into_dev && part == into {
            bail!("that's already the same thread");
        }
        if part_dev == into_dev && part_dev == me {
            // Both here: move the sessions across.
            let id = self.write_lock().merge_threads(part, &into)?;
            return Ok(Response::ThreadId { id });
        }
        {
            let store = self.write_lock();
            let now = ct_store::shared::now_ms();
            // Anything joined to `part` follows it into `into`.
            for p in store.shared_parts()? {
                if p.thread_id == part && p.thread_device == part_dev {
                    store.share_part(&p.device, &p.part, &into, &into_dev, false, &me, now)?;
                }
            }
            store.share_part(&part_dev, part, &into, &into_dev, false, &me, now)?;
        }
        self.sync_soon();
        Ok(Response::ThreadId { id: Some(into) })
    }

    /// Undo a join.
    pub(crate) fn split_thread(&self, part: &str, device: Option<&str>) -> Result<Response> {
        let me = self.device_name();
        let part_dev = device.map(str::to_string).unwrap_or_else(|| me.clone());
        let changed = {
            let store = self.write_lock();
            let found = store
                .shared_parts()?
                .into_iter()
                .find(|p| p.device == part_dev && p.part == part);
            match found {
                Some(p) => store.share_part(
                    &p.device,
                    &p.part,
                    &p.thread_id,
                    &p.thread_device,
                    true,
                    &me,
                    ct_store::shared::now_ms(),
                )?,
                None => false,
            }
        };
        self.sync_soon();
        Ok(Response::Ok { ok: changed })
    }

    /// The thread an agent means: the one named, else the one for `cwd`.
    fn thread_for(
        &self,
        thread: Option<&str>,
        device: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<(String, String)> {
        if let Some(id) = thread.filter(|t| !t.trim().is_empty()) {
            return Ok(self.resolve_root(device, id.trim()));
        }
        let Some(cwd) = cwd.filter(|c| !c.trim().is_empty()) else {
            bail!("say which thread, or give the working directory");
        };
        let local = {
            let store = self.read_lock();
            match store.latest_conversation_in(cwd)? {
                Some(conv) => store.thread(&conv)?.map(|t| t.summary.id),
                None => None,
            }
        };
        match local {
            Some(id) => Ok(self.resolve_root(None, &id)),
            None => bail!(
                "no indexed session in {cwd} yet; Crossthreads picks new sessions up within a minute"
            ),
        }
    }

    pub(crate) fn add_note(
        &self,
        thread: Option<&str>,
        device: Option<&str>,
        cwd: Option<&str>,
        kind: &str,
        text: &str,
        author: Option<&str>,
    ) -> Result<Response> {
        let (_, root) = self.thread_for(thread, device, cwd)?;
        let note = self.write_lock().add_note(
            &root,
            kind.trim(),
            text,
            author.unwrap_or("you"),
            &self.device_name(),
        )?;
        self.sync_soon();
        Ok(Response::ThreadNote { note })
    }

    pub(crate) fn edit_note(
        &self,
        id: &str,
        kind: Option<&str>,
        removed: bool,
    ) -> Result<Response> {
        let Some(note) = self.write_lock().edit_note(id, kind, removed)? else {
            bail!("no note with id {id}");
        };
        self.sync_soon();
        Ok(Response::ThreadNote { note })
    }

    pub(crate) fn agent_context_on(&self) -> bool {
        self.read_lock()
            .meta_get(AGENT_CONTEXT_KEY)
            .ok()
            .flatten()
            .map_or(true, |v| v != "off")
    }

    pub(crate) fn set_agent_context(&self, on: bool) -> Result<Response> {
        self.write_lock()
            .meta_set(AGENT_CONTEXT_KEY, if on { "on" } else { "off" })?;
        Ok(Response::Ok { ok: true })
    }

    /// What an agent should know about the thread it's working in.
    pub(crate) fn thread_context(
        &self,
        thread: Option<&str>,
        device: Option<&str>,
        cwd: Option<&str>,
        brief: bool,
    ) -> Result<Response> {
        let empty = |id: Option<String>, device: Option<String>| Response::ThreadContext {
            thread_id: id,
            device,
            markdown: String::new(),
        };
        if brief && !self.agent_context_on() {
            return Ok(empty(None, None));
        }
        let (root_dev, root) = match self.thread_for(thread, device, cwd) {
            Ok(found) => found,
            Err(_) if brief => return Ok(empty(None, None)),
            Err(e) => return Err(e),
        };
        let Some(t) = self.whole_thread(&root, Some(&root_dev))? else {
            if brief {
                return Ok(empty(None, None));
            }
            bail!("no thread {root} on {root_dev}");
        };
        let markdown = render_context(&t, brief, Utc::now());
        Ok(Response::ThreadContext {
            thread_id: Some(root),
            device: Some(root_dev),
            markdown,
        })
    }

    // ---- copying between devices -------------------------------------------

    /// A peer comparing notes: take what it sent, give back what changed
    /// here since `since`.
    pub(crate) fn peer_shared(
        &self,
        token: Option<&str>,
        from: &str,
        since: i64,
        push: Option<&SharedState>,
    ) -> Response {
        if let Err(message) = self.peer_auth(token) {
            return Response::Error { message };
        }
        if let Some(st) = push {
            if let Err(e) = self.take_shared(st) {
                eprintln!("warn: shared threads from {from}: {e:#}");
            }
        }
        match self.read_lock().shared_since(since) {
            Ok(state) => Response::Shared { state },
            Err(e) => crate::err(e),
        }
    }

    /// Merge state from another device, and apply name and status changes to
    /// this device's own copy of any thread that lives here.
    fn take_shared(&self, st: &SharedState) -> Result<usize> {
        let store = self.write_lock();
        let mut changed = 0;
        for t in st.threads.iter().take(ct_store::shared::MAX_BATCH) {
            if !store.merge_thread(t)? {
                continue;
            }
            changed += 1;
            let Some(cur) = store.shared_thread(&t.id)? else {
                continue;
            };
            let title = (cur.title_at > 0).then(|| cur.title.clone().unwrap_or_default());
            let status = cur.status.clone().filter(|_| cur.status_at > 0);
            if title.is_some() || status.is_some() {
                let _ = store.set_thread(&t.id, title.as_deref(), status.as_deref());
            }
        }
        for p in st.parts.iter().take(ct_store::shared::MAX_BATCH) {
            changed += store.merge_part(p)? as usize;
        }
        for n in st.notes.iter().take(ct_store::shared::MAX_BATCH) {
            changed += store.merge_note(n)? as usize;
        }
        Ok(changed)
    }

    /// Compare shared state with every approved device once.
    pub fn shared_sync_once(&self) -> usize {
        let Some(fed) = &self.federation else {
            return 0;
        };
        if fed.token().is_none() {
            return 0;
        }
        let me = fed.device();
        let mut total = 0;
        for peer in fed.peers() {
            let pulled_key = format!("shared_pulled:{}", peer.name);
            let pushed_key = format!("shared_pushed:{}", peer.name);
            let (since, push) = {
                let store = self.read_lock();
                let get = |k: &str| {
                    store
                        .meta_get(k)
                        .ok()
                        .flatten()
                        .and_then(|v| v.parse::<i64>().ok())
                        .unwrap_or(0)
                };
                let pushed = get(&pushed_key);
                (get(&pulled_key), store.shared_since(pushed).ok())
            };
            let req = Request::PeerShared {
                token: fed.token(),
                from: me.clone(),
                since,
                push: push.clone().filter(|p| !p.is_empty()),
            };
            match Client::new(peer.addr.clone()).call_timeout(&req, fed.timeout) {
                Ok(Response::Shared { state }) => {
                    match self.take_shared(&state) {
                        Ok(n) => total += n,
                        Err(e) => eprintln!("warn: shared threads from {}: {e:#}", peer.name),
                    }
                    let store = self.write_lock();
                    let _ = store.meta_set(&pulled_key, &state.upto.to_string());
                    if let Some(p) = &push {
                        let _ = store.meta_set(&pushed_key, &p.upto.to_string());
                    }
                }
                Ok(Response::Error { message }) => {
                    eprintln!("warn: {} refused shared threads: {message}", peer.name)
                }
                _ => {} // offline or older: try again next round
            }
        }
        total
    }

    /// Send a change made here to the other devices now, in the background.
    pub(crate) fn sync_soon(&self) {
        if self.federation.is_none() {
            return;
        }
        let this = self.clone();
        let _ = std::thread::Builder::new()
            .name("ct-shared-push".into())
            .spawn(move || {
                this.shared_sync_once();
            });
    }

    /// Compare shared thread state with peers now and every [`SYNC_EVERY`],
    /// and pick up handoff markers in newly indexed sessions.
    pub fn spawn_shared_sync(&self) {
        let this = self.clone();
        let _ = std::thread::Builder::new()
            .name("ct-shared-sync".into())
            .spawn(move || loop {
                this.join_from_handoffs();
                this.shared_sync_once();
                std::thread::sleep(SYNC_EVERY);
            });
    }

    /// A session that opened with a handoff brief from another device's
    /// thread joins that thread.
    pub fn join_from_handoffs(&self) {
        let me = self.device_name();
        let found = match self.write_lock().new_handoff_markers() {
            Ok(found) => found,
            Err(e) => {
                eprintln!("warn: reading handoff markers: {e:#}");
                return;
            }
        };
        for (conv, thread, device) in found {
            if device == me {
                // A session started here from this thread's brief (Continue
                // in…, or a pasted handoff) belongs to it.
                let attached = {
                    let store = self.write_lock();
                    match store.thread(&thread) {
                        Ok(Some(t)) if !t.sessions.iter().any(|s| s.conversation_id == conv) => {
                            store.attach_session(&conv, &t.summary.id)
                        }
                        _ => Ok(false),
                    }
                };
                if let Err(e) = attached {
                    eprintln!("warn: attaching a session to {thread}: {e:#}");
                }
                continue;
            }
            let part = match self.read_lock().thread(&conv) {
                Ok(Some(t)) => t.summary.id,
                _ => continue,
            };
            if let Err(e) = self.join_thread(&part, None, &thread, Some(&device)) {
                eprintln!("warn: joining {part} to {thread}@{device}: {e:#}");
            }
        }
    }
}

fn mark_sessions(t: &mut ThreadDetail, device: &str) {
    for s in &mut t.sessions {
        if s.device.is_none() {
            s.device = Some(device.to_string());
        }
    }
}

/// One row for a group of threads that are one piece of work.
fn merge_group(
    key: &(String, String),
    rows: Vec<ThreadSummary>,
    shared: Option<&ct_store::SharedThread>,
    me: &str,
) -> ThreadSummary {
    let (dev, id) = key;
    let mut rows = rows;
    // The thread itself first, when it's here; else the most recent part.
    let lead = rows
        .iter()
        .position(|r| &r.id == id && r.device.as_deref().unwrap_or(me) == dev)
        .unwrap_or(0);
    let mut out = rows.swap_remove(lead);
    let mut devices: BTreeSet<String> = BTreeSet::new();
    devices.insert(out.device.clone().unwrap_or_else(|| me.to_string()));
    for r in rows {
        if r.last_activity > out.last_activity && r.standing.is_some() {
            out.standing = r.standing.clone();
        }
        devices.insert(r.device.clone().unwrap_or_else(|| me.to_string()));
        for tool in r.tools {
            if !out.tools.contains(&tool) {
                out.tools.push(tool);
            }
        }
        for b in r.branches {
            if !out.branches.contains(&b) {
                out.branches.push(b);
            }
        }
        out.session_count += r.session_count;
        out.message_count += r.message_count;
        out.pending_suggestions += r.pending_suggestions;
        out.first_activity = [out.first_activity.take(), r.first_activity]
            .into_iter()
            .flatten()
            .min();
        out.last_activity = [out.last_activity.take(), r.last_activity]
            .into_iter()
            .flatten()
            .max();
        if out.repo_key.is_none() {
            out.repo_key = r.repo_key;
            out.repo_label = r.repo_label;
        }
        let rank = |l: &Option<ct_store::Live>| l.as_ref().map_or(0, |l| urgency(&l.state));
        if rank(&r.live) > rank(&out.live) {
            out.live = r.live;
        }
    }
    if let Some(sh) = shared {
        if sh.title_at > 0 {
            if let Some(t) = &sh.title {
                out.title = t.clone();
            }
        }
        if sh.status_at > 0 {
            if let Some(s) = &sh.status {
                out.status = s.clone();
            }
        }
        out.moved_to = sh.moved_to.clone();
        out.moved_tool = sh.moved_tool.clone();
    }
    out.id = id.clone();
    if out.device.is_some() || dev != me {
        out.device = Some(dev.clone());
    }
    out.devices = if devices.len() > 1 {
        devices.into_iter().collect()
    } else {
        Vec::new()
    };
    out
}

fn ts(s: &Option<String>) -> Option<DateTime<Utc>> {
    s.as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
}

/// Point out threads on different devices that look like one piece of work:
/// the same repository and (non-generic) branch, active within a couple of
/// days of each other.
fn mark_related(rows: &mut [ThreadSummary], me: &str) {
    let mut by_key: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, t) in rows.iter().enumerate() {
        let (Some(repo), Some(branch)) = (&t.repo_key, t.branches.last()) else {
            continue;
        };
        if GENERIC_BRANCHES.contains(&branch.as_str()) || t.status == "done" {
            continue;
        }
        by_key
            .entry((repo.clone(), branch.clone()))
            .or_default()
            .push(i);
    }
    let window = chrono::Duration::hours(RELATED_WINDOW_HOURS);
    for idx in by_key.values().filter(|v| v.len() > 1) {
        for &i in idx {
            for &j in idx {
                if i == j {
                    continue;
                }
                let devs = |t: &ThreadSummary| -> BTreeSet<String> {
                    if t.devices.is_empty() {
                        [t.device.clone().unwrap_or_else(|| me.to_string())].into()
                    } else {
                        t.devices.iter().cloned().collect()
                    }
                };
                if !devs(&rows[i]).is_disjoint(&devs(&rows[j])) {
                    continue;
                }
                let close = match (ts(&rows[i].last_activity), ts(&rows[j].last_activity)) {
                    (Some(a), Some(b)) => (a - b).abs() <= window,
                    _ => false,
                };
                if !close || rows[i].related.len() >= 3 {
                    continue;
                }
                let r = PartRef {
                    device: rows[j].device.clone().unwrap_or_else(|| me.to_string()),
                    id: rows[j].id.clone(),
                    title: rows[j].title.clone(),
                };
                rows[i].related.push(r);
            }
        }
    }
}

fn ago(when: &Option<String>, now: DateTime<Utc>) -> String {
    let Some(t) = ts(when) else {
        return "some time ago".into();
    };
    let d = now - t;
    if d.num_minutes() < 2 {
        "just now".into()
    } else if d.num_minutes() < 90 {
        format!("{} min ago", d.num_minutes())
    } else if d.num_hours() < 36 {
        format!("{} h ago", d.num_hours())
    } else {
        format!("{} days ago", d.num_days())
    }
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.chars().count() <= n {
        s
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// The context an agent gets about its thread. Brief: only when there's
/// something an agent couldn't know from its own session (notes, other tools
/// or devices, someone else working now), and short.
pub(crate) fn render_context(t: &ThreadDetail, brief: bool, now: DateTime<Utc>) -> String {
    let s = &t.summary;
    let tools: BTreeSet<&str> = t.sessions.iter().map(|s| s.tool.as_str()).collect();
    let devices: BTreeSet<&str> = t
        .sessions
        .iter()
        .filter_map(|s| s.device.as_deref())
        .collect();
    let busy: Vec<&ct_store::SessionRef> = t
        .sessions
        .iter()
        .filter(|s| {
            s.live
                .as_ref()
                .is_some_and(|l| matches!(l.state.as_str(), "running" | "needs_you" | "listening"))
        })
        .collect();
    let worth_it = !t.notes.is_empty() || tools.len() > 1 || devices.len() > 1 || busy.len() > 1;
    if brief && !worth_it {
        return String::new();
    }

    let mut md = format!("## Crossthreads: \"{}\"\n", s.title);
    let mut facts = vec![format!("thread `{}`", s.id), s.status.clone()];
    if let Some(repo) = &s.repo_label {
        facts.push(repo.clone());
    }
    if !tools.is_empty() {
        facts.push(format!(
            "tools: {}",
            tools.iter().copied().collect::<Vec<_>>().join(", ")
        ));
    }
    if devices.len() > 1 {
        facts.push(format!(
            "devices: {}",
            devices.iter().copied().collect::<Vec<_>>().join(", ")
        ));
    }
    md.push_str(&facts.join(" · "));
    md.push('\n');

    let notes_of = |kind: &str| -> Vec<&ct_store::ThreadNote> {
        t.notes.iter().filter(|n| n.kind == kind).collect()
    };
    let section = |md: &mut String, title: &str, notes: &[&ct_store::ThreadNote], with_id: bool| {
        if notes.is_empty() {
            return;
        }
        md.push_str(&format!("\n**{title}**\n"));
        for n in notes {
            let who = if n.author.is_empty() {
                ""
            } else {
                n.author.as_str()
            };
            let id = if with_id {
                format!(" [{}]", n.id)
            } else {
                String::new()
            };
            md.push_str(&format!(
                "- {}{} — {}\n",
                clip(&n.text, if brief { 160 } else { 600 }),
                id,
                who
            ));
        }
    };
    section(&mut md, "Decisions", &notes_of("decision"), false);
    section(&mut md, "Open todos", &notes_of("todo"), true);
    section(&mut md, "Context", &notes_of("context"), false);
    if !brief {
        section(&mut md, "Done", &notes_of("done"), false);
    }

    md.push_str("\n**Sessions**\n");
    let shown: Vec<&ct_store::SessionRef> = if brief {
        t.sessions.iter().rev().take(4).collect()
    } else {
        t.sessions.iter().rev().take(12).collect()
    };
    for sess in shown {
        let state = sess
            .live
            .as_ref()
            .filter(|l| l.state != "ended" && l.state != "idle")
            .map(|l| format!(" ({})", l.state.replace('_', " ")))
            .unwrap_or_default();
        let on = sess
            .device
            .as_deref()
            .filter(|_| devices.len() > 1)
            .map(|d| format!(" on {d}"))
            .unwrap_or_default();
        md.push_str(&format!(
            "- {}{on}, {}{state}: {}\n",
            sess.tool,
            ago(&sess.last_activity, now),
            clip(sess.title.as_deref().unwrap_or("(untitled)"), 90)
        ));
    }
    md.push_str(
        "\nRecord decisions and todos with crossthreads_note so other agents and devices see them; \
         crossthreads_thread_context has the full picture.\n",
    );
    if brief && md.chars().count() > BRIEF_CHARS {
        md = md.chars().take(BRIEF_CHARS).collect();
        md.push_str("…\n");
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, device: &str, repo: &str, branch: &str, last: &str) -> ThreadSummary {
        ThreadSummary {
            id: id.into(),
            title: format!("title {id}"),
            status: "active".into(),
            repo_key: Some(repo.into()),
            repo_label: Some(repo.into()),
            tools: vec![format!("tool-{id}")],
            branches: vec![branch.into()],
            session_count: 1,
            message_count: 10,
            first_activity: Some(last.into()),
            last_activity: Some(last.into()),
            pending_suggestions: 0,
            live: None,
            device: Some(device.into()),
            devices: vec![],
            related: vec![],
            standing: None,
            moved_to: None,
            moved_tool: None,
        }
    }

    #[test]
    fn merges_a_group_into_one_row() {
        let a = row("th_a", "laptop", "r", "feat", "2026-11-01T10:00:00Z");
        let b = row("th_b", "desktop", "r", "feat", "2026-11-02T10:00:00Z");
        let m = merge_group(
            &("laptop".into(), "th_a".into()),
            vec![b, a],
            None,
            "laptop",
        );
        assert_eq!(m.id, "th_a");
        assert_eq!(m.device.as_deref(), Some("laptop"));
        assert_eq!(m.devices, vec!["desktop".to_string(), "laptop".to_string()]);
        assert_eq!(m.session_count, 2);
        assert_eq!(m.last_activity.as_deref(), Some("2026-11-02T10:00:00Z"));
        assert_eq!(m.tools.len(), 2);
    }

    #[test]
    fn related_needs_same_branch_other_device_and_close_in_time() {
        let mut rows = vec![
            row("th_a", "laptop", "r", "feat", "2026-11-01T10:00:00Z"),
            row("th_b", "desktop", "r", "feat", "2026-11-02T09:00:00Z"),
            row("th_c", "desktop", "r", "main", "2026-11-01T10:00:00Z"),
            row("th_d", "laptop", "r", "feat", "2026-11-01T11:00:00Z"),
            row("th_e", "desktop", "r", "feat", "2026-10-01T10:00:00Z"),
        ];
        mark_related(&mut rows, "laptop");
        let ids = |t: &ThreadSummary| t.related.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&rows[0]), vec!["th_b"]);
        assert!(rows[2].related.is_empty());
        assert!(ids(&rows[1]).contains(&"th_a".to_string()));
        assert!(rows[4].related.is_empty());
    }

    #[test]
    fn brief_context_only_when_worth_it() {
        let summary = row("th_a", "laptop", "r", "feat", "2026-11-01T10:00:00Z");
        let mut t = ThreadDetail {
            summary,
            sessions: vec![],
            links: vec![],
            suggestions: vec![],
            notes: vec![],
            parts: vec![],
        };
        let now = Utc::now();
        assert_eq!(render_context(&t, true, now), "");
        t.notes.push(ct_store::ThreadNote {
            id: "n_1".into(),
            thread_id: "th_a".into(),
            kind: "decision".into(),
            text: "Use SQLite, not Postgres".into(),
            author: "claude".into(),
            device: "laptop".into(),
            created: 1,
            removed: false,
            updated: 1,
        });
        let md = render_context(&t, true, now);
        assert!(md.contains("Use SQLite"));
        assert!(md.contains("crossthreads_note"));
    }
}
