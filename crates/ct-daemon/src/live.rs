//! What each session is doing right now (SPEC §6.2).
//!
//! Two sources, in order of trust:
//!
//! - **Hooks.** `crossthreads hooks install` makes Claude Code and Codex call
//!   `crossthreads hook …` on session events, which lands here as a
//!   [`HookEvent`]. This is precise: it can tell "running" from "waiting on a
//!   permission prompt".
//! - **The transcript file.** For a session no hook has reported, a transcript
//!   written in the last few seconds means the agent is working. This is a
//!   guess, is labelled `inferred`, and cannot see "needs you".
//!
//! State is held in memory only. A restarted daemon starts blank and relearns
//! from the next event, which is the right failure mode for something that
//! describes the present.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use ct_store::{Live, SessionRef, ThreadSummary};
use serde::{Deserialize, Serialize};

/// A hook that has been silent this long while "running" is presumed idle.
const RUNNING_QUIET_MINUTES: i64 = 20;
/// Entries are forgotten after this long without an event.
const FORGET_HOURS: i64 = 12;
/// A transcript written this recently means the session is working.
const INFER_RUNNING_SECS: u64 = 45;
/// Only sessions active this recently are checked at all.
const RECENT_HOURS: i64 = 24;

/// Tools that keep one transcript file per session, so the file's modification
/// time says something about that session. (Editors that keep every chat in
/// one database are left out: the file changes whenever anything does.)
const FILE_PER_SESSION: [&str; 3] = ["claude-code", "codex", "gemini-cli"];

/// One event from a tool's hook, as `crossthreads hook` forwards it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HookEvent {
    /// Tool slug (`claude-code`, `codex`).
    pub tool: String,
    /// The tool's event name (`UserPromptSubmit`, `Stop`, …).
    pub event: String,
    /// The transcript file for the session. Events without one are ignored:
    /// it is how a live session is tied to its indexed record.
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Notification text, when the event carries one.
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Running,
    NeedsYou,
    Idle,
    /// Parked in a room, waiting for another agent's message.
    Listening,
    Ended,
}

impl State {
    fn slug(self) -> &'static str {
        match self {
            State::Running => "running",
            State::NeedsYou => "needs_you",
            State::Idle => "idle",
            State::Listening => "listening",
            State::Ended => "ended",
        }
    }
}

/// How pressing a state is, for picking one state to show for a whole thread.
pub(crate) fn urgency(state: &str) -> u8 {
    match state {
        "needs_you" => 3,
        "running" => 2,
        "listening" => 1,
        "idle" => 1,
        _ => 0,
    }
}

/// What an event means for the session's state. `None` = the session is alive
/// but its state is unchanged.
fn transition(event: &str, message: Option<&str>) -> Option<State> {
    match event.to_ascii_lowercase().as_str() {
        "sessionstart" => Some(State::Idle),
        "userpromptsubmit" | "pretooluse" | "posttooluse" => Some(State::Running),
        "stop" => Some(State::Idle),
        "sessionend" => Some(State::Ended),
        // Sent by our own Stop hook around a wait in a room.
        "roomlisten" => Some(State::Listening),
        "roomwake" => Some(State::Running),
        "notification" => {
            let text = message.unwrap_or("").to_ascii_lowercase();
            if text.contains("permission") || text.contains("approv") {
                Some(State::NeedsYou)
            } else if text.contains("waiting for your input") {
                Some(State::Idle)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct Entry {
    state: State,
    /// When the state last changed.
    since: DateTime<Utc>,
    /// When any event last arrived.
    seen: DateTime<Utc>,
}

/// Same file, however it was spelled.
fn canonical(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

/// In-memory record of what hooks have reported, keyed by transcript path.
#[derive(Debug, Default)]
pub struct LiveRegistry {
    entries: Mutex<HashMap<String, Entry>>,
}

impl LiveRegistry {
    /// Apply a hook event. Returns `false` when it carried no transcript path
    /// and so could not be tied to a session.
    pub fn record(&self, ev: &HookEvent, now: DateTime<Utc>) -> bool {
        let Some(path) = ev
            .transcript_path
            .as_deref()
            .filter(|p| !p.trim().is_empty())
        else {
            return false;
        };
        let next = transition(&ev.event, ev.message.as_deref());
        let mut entries = self.entries.lock().expect("live registry poisoned");
        entries.retain(|_, e| now - e.seen < Duration::hours(FORGET_HOURS));
        let entry = entries.entry(canonical(path)).or_insert(Entry {
            state: next.unwrap_or(State::Idle),
            since: now,
            seen: now,
        });
        entry.seen = now;
        if let Some(state) = next {
            if entry.state != state {
                entry.state = state;
                entry.since = now;
            }
        }
        true
    }

    fn is_empty(&self) -> bool {
        self.entries
            .lock()
            .expect("live registry poisoned")
            .is_empty()
    }

    /// What hooks last said about the session whose transcript is `path`.
    fn reported(&self, path: &str, now: DateTime<Utc>) -> Option<Live> {
        let entries = self.entries.lock().expect("live registry poisoned");
        let e = entries
            .get(path)
            .or_else(|| entries.get(&canonical(path)))?;
        if now - e.seen >= Duration::hours(FORGET_HOURS) {
            return None;
        }
        // A hook that said "running" and then went quiet most likely missed
        // its stop (a crash, a closed terminal). Don't claim it is still going.
        if e.state == State::Running && now - e.seen > Duration::minutes(RUNNING_QUIET_MINUTES) {
            return Some(Live {
                state: State::Idle.slug().to_string(),
                inferred: true,
                since: Some(e.seen.to_rfc3339()),
            });
        }
        Some(Live {
            state: e.state.slug().to_string(),
            inferred: false,
            since: Some(e.since.to_rfc3339()),
        })
    }

    /// The state of one session: what hooks reported, else a guess from its
    /// transcript file, else nothing.
    pub fn of_session(&self, s: &SessionRef, now: DateTime<Utc>) -> Option<Live> {
        if s.source_path.is_empty() {
            return None;
        }
        if !self.is_empty() {
            if let Some(live) = self.reported(&s.source_path, now) {
                return Some(live);
            }
        }
        if !FILE_PER_SESSION.contains(&s.tool.as_str()) {
            return None;
        }
        let modified = Path::new(&s.source_path).metadata().ok()?.modified().ok()?;
        let age = SystemTime::now().duration_since(modified).ok()?;
        (age.as_secs() <= INFER_RUNNING_SECS).then(|| Live {
            state: State::Running.slug().to_string(),
            inferred: true,
            since: None,
        })
    }

    /// Fill in `live` on a thread's sessions and on the thread itself (the most
    /// pressing state among them). Sessions that have been quiet for a day are
    /// skipped without touching the filesystem.
    pub fn annotate(
        &self,
        thread: &mut ThreadSummary,
        sessions: &mut [SessionRef],
        now: DateTime<Utc>,
    ) {
        let cutoff = (now - Duration::hours(RECENT_HOURS)).to_rfc3339();
        // With no hooks reporting, only a recently active session can be live.
        let hooks = !self.is_empty();
        for s in sessions.iter_mut() {
            let recent = s
                .last_activity
                .as_deref()
                .is_some_and(|t| t >= cutoff.as_str());
            if recent || hooks {
                s.live = self.of_session(s, now);
            }
        }
        thread.live = sessions
            .iter()
            .filter_map(|s| s.live.clone())
            .max_by_key(|l| urgency(&l.state));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: &str, path: &str, message: Option<&str>) -> HookEvent {
        HookEvent {
            tool: "claude-code".into(),
            event: event.into(),
            transcript_path: Some(path.into()),
            message: message.map(str::to_string),
            ..Default::default()
        }
    }

    fn session(tool: &str, path: &str, last: DateTime<Utc>) -> SessionRef {
        SessionRef {
            conversation_id: "cv_x".into(),
            session_key: "sk_x".into(),
            tool: tool.into(),
            project: None,
            repo_key: None,
            title: None,
            started_at: Some(last.to_rfc3339()),
            last_activity: Some(last.to_rfc3339()),
            branch: None,
            message_count: 1,
            source_path: path.into(),
            live: None,
            device: None,
        }
    }

    fn state(reg: &LiveRegistry, path: &str, now: DateTime<Utc>) -> Option<(String, bool)> {
        reg.of_session(&session("cursor", path, now), now)
            .map(|l| (l.state, l.inferred))
    }

    #[test]
    fn follows_a_session_through_its_events() {
        let reg = LiveRegistry::default();
        let t0 = Utc::now();
        let p = "/no/such/transcript.jsonl";
        let said = |s: &str| Some((s.to_string(), false));

        assert!(reg.record(&ev("SessionStart", p, None), t0));
        assert_eq!(state(&reg, p, t0), said("idle"));
        reg.record(&ev("UserPromptSubmit", p, None), t0);
        assert_eq!(state(&reg, p, t0), said("running"));
        reg.record(&ev("PreToolUse", p, None), t0);
        reg.record(
            &ev(
                "Notification",
                p,
                Some("Claude needs your permission to use Bash"),
            ),
            t0,
        );
        assert_eq!(state(&reg, p, t0), said("needs_you"));
        // Approved: the tool runs, so the session is working again.
        reg.record(&ev("PostToolUse", p, None), t0);
        assert_eq!(state(&reg, p, t0), said("running"));
        reg.record(&ev("Stop", p, None), t0);
        assert_eq!(state(&reg, p, t0), said("idle"));
        // An unrelated notification keeps the state.
        reg.record(&ev("Notification", p, Some("Task finished")), t0);
        assert_eq!(state(&reg, p, t0), said("idle"));
        reg.record(&ev("SessionEnd", p, None), t0);
        assert_eq!(state(&reg, p, t0), said("ended"));
    }

    #[test]
    fn an_event_without_a_transcript_is_ignored() {
        let reg = LiveRegistry::default();
        let mut e = ev("Stop", "", None);
        assert!(!reg.record(&e, Utc::now()));
        e.transcript_path = None;
        assert!(!reg.record(&e, Utc::now()));
        assert!(reg.is_empty());
    }

    #[test]
    fn a_running_session_that_goes_quiet_is_presumed_idle_then_forgotten() {
        let reg = LiveRegistry::default();
        let t0 = Utc::now();
        let p = "/no/such/transcript.jsonl";
        reg.record(&ev("UserPromptSubmit", p, None), t0);
        assert_eq!(
            state(&reg, p, t0 + Duration::minutes(5)),
            Some(("running".into(), false))
        );
        assert_eq!(
            state(&reg, p, t0 + Duration::minutes(RUNNING_QUIET_MINUTES + 1)),
            Some(("idle".into(), true))
        );
        assert_eq!(state(&reg, p, t0 + Duration::hours(FORGET_HOURS)), None);
        // A blocked session stays blocked however long it waits (until forgotten).
        reg.record(&ev("Notification", p, Some("needs your permission")), t0);
        assert_eq!(
            state(&reg, p, t0 + Duration::hours(3)),
            Some(("needs_you".into(), false))
        );
    }

    #[test]
    fn infers_running_from_a_freshly_written_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("s.jsonl");
        std::fs::write(&file, "{}\n").unwrap();
        let path = file.to_str().unwrap();
        let reg = LiveRegistry::default();
        let now = Utc::now();

        let live = reg
            .of_session(&session("claude-code", path, now), now)
            .unwrap();
        assert_eq!((live.state.as_str(), live.inferred), ("running", true));
        // One shared database for every chat says nothing about any one of them.
        assert!(reg.of_session(&session("cursor", path, now), now).is_none());
        // A hook report beats the guess.
        reg.record(&ev("Stop", path, None), now);
        let live = reg
            .of_session(&session("claude-code", path, now), now)
            .unwrap();
        assert_eq!((live.state.as_str(), live.inferred), ("idle", false));
    }

    #[test]
    fn a_thread_shows_its_most_pressing_session() {
        let reg = LiveRegistry::default();
        let now = Utc::now();
        reg.record(&ev("UserPromptSubmit", "/t/a.jsonl", None), now);
        reg.record(
            &ev("Notification", "/t/b.jsonl", Some("needs your permission")),
            now,
        );
        let mut sessions = vec![
            session("claude-code", "/t/a.jsonl", now),
            session("codex", "/t/b.jsonl", now),
            session("codex", "/t/c.jsonl", now - Duration::days(3)),
        ];
        let mut thread = ThreadSummary {
            id: "th_x".into(),
            title: "t".into(),
            status: "active".into(),
            repo_key: None,
            repo_label: None,
            tools: vec![],
            branches: vec![],
            session_count: 3,
            message_count: 3,
            first_activity: None,
            last_activity: None,
            pending_suggestions: 0,
            live: None,
            device: None,
            devices: vec![],
            related: vec![],
            standing: None,
        };
        reg.annotate(&mut thread, &mut sessions, now);
        assert_eq!(thread.live.unwrap().state, "needs_you");
        assert_eq!(sessions[0].live.as_ref().unwrap().state, "running");
        assert!(sessions[2].live.is_none());
    }
}
