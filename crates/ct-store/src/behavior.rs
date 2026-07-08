//! Behavioral metrics over the indexed sessions — the deterministic foundation
//! for "how you work" insights. No model involved: this counts and aggregates
//! signals from your *own* messages and the shape of each interaction (turns,
//! corrections, specificity, tempo), which the LLM layer then interprets.
//!
//! [`analyze`] is pure (takes already-loaded sessions); `Store::work_metrics`
//! does the querying and calls it.

use serde::{Deserialize, Serialize};

/// One message, the input granularity for [`analyze`].
#[derive(Debug, Clone)]
pub struct RawMessage {
    pub role: String,
    /// May be truncated by the caller.
    pub content: String,
    /// Agent-action names recorded on this message (e.g. `["Edit","Bash"]`).
    pub tools: Vec<String>,
    /// ISO-8601 timestamp of this message, when known.
    pub ts: Option<String>,
}

/// One session reduced to its messages + light metadata, the input to [`analyze`].
#[derive(Debug, Clone)]
pub struct RawSession {
    pub id: String,
    pub title: Option<String>,
    pub tool: String,
    pub project: Option<String>,
    pub model: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub messages: Vec<RawMessage>,
}

/// Per-session behavioral metrics (used to pick friction extremes, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetrics {
    pub id: String,
    pub title: Option<String>,
    pub tool: String,
    pub project: Option<String>,
    pub user_turns: i64,
    pub corrections: i64,
    pub first_prompt_chars: i64,
    /// Composite friction score: turns + 2·corrections (higher = rougher).
    pub friction: f64,
    /// Heuristic: did the thread end on a resolved note (assistant last) vs.
    /// trail off on an unanswered user turn.
    pub resolved: bool,
}

/// Aggregate "Work DNA" numbers across the analyzed sessions. All rates are
/// fractions in `0.0..=1.0`. Serialized for the daemon/MCP/UI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkMetrics {
    /// Sessions analyzed (thread kind).
    pub sessions: i64,
    /// Per-tool session counts, busiest first.
    pub by_tool: Vec<(String, i64)>,
    /// Distinct projects touched.
    pub projects: i64,
    /// Median / mean user turns per session.
    pub median_user_turns: f64,
    pub mean_user_turns: f64,
    /// Fraction of sessions with at least one correction ("no", "actually", …).
    pub correction_rate: f64,
    /// Fraction where a correction landed within the first two user turns
    /// (an under-specified opening prompt).
    pub first_prompt_miss_rate: f64,
    /// Fraction that trailed off unresolved (ended on a user turn).
    pub abandonment_rate: f64,
    /// Average characters in the opening user message.
    pub avg_first_prompt_chars: f64,
    /// Fraction of opening prompts that state explicit constraints
    /// (must / should / only / don't / never / exactly …).
    pub specificity_rate: f64,
    /// Fraction of sessions where the user pasted code.
    pub code_paste_rate: f64,
    /// Fraction where the user pasted an error / stack trace.
    pub error_paste_rate: f64,
    /// Fraction of sessions that mention tests.
    pub test_mention_rate: f64,
    /// Fraction that mention commits / git.
    pub commit_mention_rate: f64,
    /// Session counts by hour-of-day (0–23), for tempo.
    pub by_hour: Vec<(i64, i64)>,
    /// Projects with the highest average user-turns (friction), top first.
    pub high_friction_projects: Vec<(String, f64)>,
    // --- width signals (model / actions / languages / duration) ---
    /// Average agent actions (tool calls) per session — a delegation signal.
    pub avg_tool_actions: f64,
    /// Most-used agent actions, busiest first: `[("Edit", 320), ("Bash", 210)]`.
    pub top_actions: Vec<(String, i64)>,
    /// Languages you work in (from fenced code blocks), most first.
    pub top_languages: Vec<(String, i64)>,
    /// Model usage mix, most first.
    pub by_model: Vec<(String, i64)>,
    /// Median session length in minutes (from started_at→ended_at), when known.
    pub median_session_minutes: f64,
    // --- sharper diagnostics (v0.11) ---
    /// Median seconds from the opening user message to the first assistant
    /// reply — a responsiveness / prompt-clarity signal. 0 when unknown.
    pub median_time_to_first_response_secs: f64,
    /// Of sessions where an error/stack trace was pasted, the fraction that
    /// still ended resolved — an error-recovery signal.
    pub error_recovery_rate: f64,
    /// Mean distinct projects touched per active day — a context-switching cost
    /// signal (higher = more switching).
    pub avg_projects_per_active_day: f64,
}

/// Words that, opening a user turn after the first, signal a correction / rework.
const CORRECTION_MARKERS: &[&str] = &[
    "no,",
    "no.",
    "nope",
    "actually",
    "that's wrong",
    "thats wrong",
    "that's not",
    "thats not",
    "not what i",
    "not quite",
    "incorrect",
    "undo",
    "revert",
    "rollback",
    "go back",
    "try again",
    "that didn't work",
    "that didnt work",
    "doesn't work",
    "doesnt work",
    "still broken",
    "instead",
    "i meant",
];

/// Constraint words that make an opening prompt specific.
const CONSTRAINT_MARKERS: &[&str] = &[
    "must",
    "should",
    "only",
    "don't",
    "dont",
    "never",
    "always",
    "exactly",
    "ensure",
    "require",
    "without",
    "instead of",
    "make sure",
];

fn is_user(role: &str) -> bool {
    let r = role.to_lowercase();
    r == "user" || r == "human"
}

fn has_correction(text: &str) -> bool {
    let t = text.to_lowercase();
    CORRECTION_MARKERS.iter().any(|m| t.contains(m))
}

fn has_constraint(text: &str) -> bool {
    let t = text.to_lowercase();
    CONSTRAINT_MARKERS.iter().any(|m| t.contains(m))
}

fn has_code(text: &str) -> bool {
    text.contains("```") || text.matches("    ").count() > 3 || text.contains(");\n")
}

fn has_error(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("error")
        || t.contains("traceback")
        || t.contains("exception")
        || t.contains("panicked")
        || t.contains("stack trace")
        || t.contains("undefined")
        || t.contains("cannot find")
}

fn mentions(text: &str, needles: &[&str]) -> bool {
    let t = text.to_lowercase();
    needles.iter().any(|n| t.contains(n))
}

/// Hour-of-day (0–23) parsed from an ISO `started_at`, if present.
fn hour_of(started_at: Option<&str>) -> Option<i64> {
    let s = started_at?;
    // `2026-06-24T09:12:00Z` → take the two digits after 'T'.
    let t = s.split('T').nth(1)?;
    t.get(..2)?
        .parse::<i64>()
        .ok()
        .filter(|h| (0..24).contains(h))
}

/// Minutes between two ISO timestamps, if both parse and the span is sane
/// (0 < mins ≤ 24h — guards against clock skew / multi-day idle sessions).
fn span_minutes(start: Option<&str>, end: Option<&str>) -> Option<f64> {
    span_seconds(start, end)
        .map(|s| s / 60.0)
        .filter(|m| *m <= 24.0 * 60.0)
}

/// Seconds between two ISO timestamps, if both parse and end ≥ start.
fn span_seconds(start: Option<&str>, end: Option<&str>) -> Option<f64> {
    use chrono::DateTime;
    let s = DateTime::parse_from_rfc3339(start?).ok()?;
    let e = DateTime::parse_from_rfc3339(end?).ok()?;
    let secs = (e - s).num_seconds() as f64;
    (secs >= 0.0).then_some(secs)
}

/// The ISO date prefix (`YYYY-MM-DD`) of a timestamp.
fn day_of(started_at: Option<&str>) -> Option<String> {
    started_at.map(|s| s.get(..10).unwrap_or(s).to_string())
}

/// Languages named on fenced code blocks in `text` (the token right after an
/// opening ```), lowercased. e.g. "```rust\n…" → "rust".
fn fenced_languages(text: &str, out: &mut std::collections::BTreeSet<String>) {
    // Even fences are openers; the language is the first token on that line.
    for (i, seg) in text.split("```").enumerate() {
        if i == 0 || i % 2 == 0 {
            continue; // i==0 is pre-fence text; even i are *after* a closing fence
        }
        let lang: String = seg
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '+' || *c == '#')
            .collect::<String>()
            .to_lowercase();
        if !lang.is_empty() && lang.len() <= 16 {
            out.insert(lang);
        }
    }
}

/// Compute per-session and aggregate metrics. Pure and deterministic.
pub fn analyze(sessions: &[RawSession]) -> (WorkMetrics, Vec<SessionMetrics>) {
    let mut per = Vec::with_capacity(sessions.len());
    let mut tool_counts: std::collections::BTreeMap<String, i64> = Default::default();
    let mut hour_counts: std::collections::BTreeMap<i64, i64> = Default::default();
    let mut project_turns: std::collections::BTreeMap<String, (f64, i64)> = Default::default();
    let mut projects: std::collections::BTreeSet<String> = Default::default();

    let mut user_turn_samples: Vec<i64> = Vec::new();
    let (mut corrected, mut first_miss, mut abandoned) = (0i64, 0i64, 0i64);
    let (mut first_chars_sum, mut specific, mut code, mut error) = (0i64, 0i64, 0i64, 0i64);
    let (mut tests, mut commits) = (0i64, 0i64);
    // Width accumulators (model / actions / languages / duration).
    let mut action_counts: std::collections::BTreeMap<String, i64> = Default::default();
    let mut lang_counts: std::collections::BTreeMap<String, i64> = Default::default();
    let mut model_counts: std::collections::BTreeMap<String, i64> = Default::default();
    let mut total_actions = 0i64;
    let mut duration_samples: Vec<f64> = Vec::new();
    // Sharper diagnostics (v0.11).
    let mut ttfr_samples: Vec<f64> = Vec::new(); // time-to-first-response, secs
    let (mut error_sessions, mut error_recovered) = (0i64, 0i64);
    let mut day_projects: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        Default::default();

    for s in sessions {
        *tool_counts.entry(s.tool.clone()).or_default() += 1;
        if let Some(p) = &s.project {
            projects.insert(p.clone());
        }
        if let Some(h) = hour_of(s.started_at.as_deref()) {
            *hour_counts.entry(h).or_default() += 1;
        }
        if let Some(m) = s.model.as_deref().filter(|m| !m.is_empty()) {
            *model_counts.entry(m.to_string()).or_default() += 1;
        }
        if let Some(mins) = span_minutes(s.started_at.as_deref(), s.ended_at.as_deref()) {
            duration_samples.push(mins);
        }
        // Context switching: distinct projects touched per calendar day.
        if let (Some(day), Some(p)) = (day_of(s.started_at.as_deref()), s.project.clone()) {
            day_projects.entry(day).or_default().insert(p);
        }
        // Time-to-first-response: first user message ts → first later assistant ts.
        let first_user_ts = s
            .messages
            .iter()
            .find(|m| is_user(&m.role))
            .and_then(|m| m.ts.clone());
        let first_asst_ts = s
            .messages
            .iter()
            .skip_while(|m| !is_user(&m.role))
            .find(|m| m.role.eq_ignore_ascii_case("assistant"))
            .and_then(|m| m.ts.clone());
        if let Some(secs) = span_seconds(first_user_ts.as_deref(), first_asst_ts.as_deref()) {
            if secs <= 3600.0 {
                ttfr_samples.push(secs); // ignore idle-overnight outliers
            }
        }

        let user_msgs: Vec<&str> = s
            .messages
            .iter()
            .filter(|m| is_user(&m.role))
            .map(|m| m.content.as_str())
            .collect();
        let user_turns = user_msgs.len() as i64;
        user_turn_samples.push(user_turns);

        // Width: agent actions (tool calls) and languages across the session.
        let mut langs = std::collections::BTreeSet::new();
        for m in &s.messages {
            for t in &m.tools {
                *action_counts.entry(t.clone()).or_default() += 1;
                total_actions += 1;
            }
            fenced_languages(&m.content, &mut langs);
        }
        for l in langs {
            *lang_counts.entry(l).or_default() += 1;
        }

        // Corrections: a correction marker in any user turn *after* the first.
        let corrections = user_msgs
            .iter()
            .skip(1)
            .filter(|c| has_correction(c))
            .count() as i64;
        if corrections > 0 {
            corrected += 1;
        }
        // First-prompt miss: a correction in the 2nd user turn (the first reply
        // to the opening prompt).
        if user_msgs.get(1).map(|c| has_correction(c)).unwrap_or(false) {
            first_miss += 1;
        }

        let first = user_msgs.first().copied().unwrap_or("");
        first_chars_sum += first.chars().count() as i64;
        if has_constraint(first) {
            specific += 1;
        }

        let whole: String = user_msgs.join("\n");
        if has_code(&whole) {
            code += 1;
        }
        let had_error = has_error(&whole);
        if had_error {
            error += 1;
        }
        if mentions(
            &whole,
            &["test", "pytest", "jest", "cargo test", "unit test"],
        ) {
            tests += 1;
        }
        if mentions(&whole, &["commit", "git add", "git push"]) {
            commits += 1;
        }

        // Resolved heuristic: the last message is the assistant's (it answered),
        // not a trailing user turn.
        let resolved = s
            .messages
            .last()
            .map(|m| !is_user(&m.role))
            .unwrap_or(false);
        if !resolved {
            abandoned += 1;
        }
        // Error recovery: of sessions where an error was pasted, did it resolve?
        if had_error {
            error_sessions += 1;
            if resolved {
                error_recovered += 1;
            }
        }

        if let Some(p) = &s.project {
            let e = project_turns.entry(p.clone()).or_default();
            e.0 += user_turns as f64;
            e.1 += 1;
        }

        per.push(SessionMetrics {
            id: s.id.clone(),
            title: s.title.clone(),
            tool: s.tool.clone(),
            project: s.project.clone(),
            user_turns,
            corrections,
            first_prompt_chars: first.chars().count() as i64,
            friction: user_turns as f64 + corrections as f64 * 2.0,
            resolved,
        });
    }

    let n = sessions.len().max(1) as f64;
    let mut by_tool: Vec<(String, i64)> = tool_counts.into_iter().collect();
    by_tool.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut high_friction_projects: Vec<(String, f64)> = project_turns
        .into_iter()
        .filter(|(_, (_, c))| *c >= 3) // enough samples to be meaningful
        .map(|(p, (sum, c))| (p, sum / c as f64))
        .collect();
    high_friction_projects
        .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    high_friction_projects.truncate(6);

    // Width signals: sort by count desc, keep the top handful.
    let top = |m: std::collections::BTreeMap<String, i64>, k: usize| -> Vec<(String, i64)> {
        let mut v: Vec<(String, i64)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.truncate(k);
        v
    };
    let mut durations = duration_samples;

    let metrics = WorkMetrics {
        sessions: sessions.len() as i64,
        by_tool,
        projects: projects.len() as i64,
        median_user_turns: median(&mut user_turn_samples),
        mean_user_turns: user_turn_samples.iter().sum::<i64>() as f64 / n,
        correction_rate: corrected as f64 / n,
        first_prompt_miss_rate: first_miss as f64 / n,
        abandonment_rate: abandoned as f64 / n,
        avg_first_prompt_chars: first_chars_sum as f64 / n,
        specificity_rate: specific as f64 / n,
        code_paste_rate: code as f64 / n,
        error_paste_rate: error as f64 / n,
        test_mention_rate: tests as f64 / n,
        commit_mention_rate: commits as f64 / n,
        by_hour: hour_counts.into_iter().collect(),
        high_friction_projects,
        avg_tool_actions: total_actions as f64 / n,
        top_actions: top(action_counts, 8),
        top_languages: top(lang_counts, 8),
        by_model: top(model_counts, 6),
        median_session_minutes: median_f64(&mut durations),
        median_time_to_first_response_secs: median_f64(&mut ttfr_samples),
        error_recovery_rate: if error_sessions > 0 {
            error_recovered as f64 / error_sessions as f64
        } else {
            0.0
        },
        avg_projects_per_active_day: if day_projects.is_empty() {
            0.0
        } else {
            day_projects.values().map(|s| s.len() as f64).sum::<f64>() / day_projects.len() as f64
        },
    };
    (metrics, per)
}

fn median_f64(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    if v.len() % 2 == 0 {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    }
}

fn median(v: &mut [i64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable();
    let mid = v.len() / 2;
    if v.len() % 2 == 0 {
        (v[mid - 1] + v[mid]) as f64 / 2.0
    } else {
        v[mid] as f64
    }
}

/// Render the aggregate metrics as a compact, human/LLM-readable block — the
/// quantitative backbone the behavioral insights interpret.
pub fn summarize(m: &WorkMetrics) -> String {
    let pct = |x: f64| format!("{:.0}%", x * 100.0);
    let tools = m
        .by_tool
        .iter()
        .map(|(t, n)| format!("{t} {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    let busiest = m
        .by_hour
        .iter()
        .max_by_key(|(_, n)| *n)
        .map(|(h, _)| format!("{h:02}:00"))
        .unwrap_or_else(|| "—".into());
    let friction = m
        .high_friction_projects
        .iter()
        .take(3)
        .map(|(p, a)| format!("{p} ({a:.1} turns)"))
        .collect::<Vec<_>>()
        .join(", ");
    let join = |v: &[(String, i64)], n: usize| -> String {
        let s = v
            .iter()
            .take(n)
            .map(|(k, c)| format!("{k} {c}"))
            .collect::<Vec<_>>()
            .join(", ");
        if s.is_empty() {
            "—".into()
        } else {
            s
        }
    };
    let dur = if m.median_session_minutes > 0.0 {
        format!("{:.0} min", m.median_session_minutes)
    } else {
        "—".into()
    };
    format!(
        "Sessions analyzed: {sessions} across {projects} project(s).\n\
         Tools: {tools}.\n\
         User turns per session: median {median:.0}, mean {mean:.1}.\n\
         Correction rate: {corr} (first-prompt miss: {miss}).\n\
         Abandoned/unresolved: {aband}.\n\
         Opening prompt: avg {fpc:.0} chars, {spec} state explicit constraints.\n\
         Pasted code: {code}; pasted errors: {err}.\n\
         Mentions tests: {tests}; mentions commits/git: {commits}.\n\
         Agent actions per session: avg {actions:.1} — top: {top_actions}.\n\
         Languages: {langs}.\n\
         Models: {models}.\n\
         Median session length: {dur}.\n\
         Time to first response: {ttfr}.\n\
         Error-recovery rate: {recov}.\n\
         Context switching: {switch:.1} projects/active-day.\n\
         Busiest hour: {busiest}.\n\
         Highest-friction projects (avg user turns): {friction}.",
        sessions = m.sessions,
        projects = m.projects,
        median = m.median_user_turns,
        mean = m.mean_user_turns,
        corr = pct(m.correction_rate),
        miss = pct(m.first_prompt_miss_rate),
        aband = pct(m.abandonment_rate),
        fpc = m.avg_first_prompt_chars,
        spec = pct(m.specificity_rate),
        code = pct(m.code_paste_rate),
        err = pct(m.error_paste_rate),
        tests = pct(m.test_mention_rate),
        commits = pct(m.commit_mention_rate),
        actions = m.avg_tool_actions,
        top_actions = join(&m.top_actions, 5),
        langs = join(&m.top_languages, 6),
        models = join(&m.by_model, 4),
        ttfr = if m.median_time_to_first_response_secs > 0.0 {
            format!("{:.0}s", m.median_time_to_first_response_secs)
        } else {
            "—".into()
        },
        recov = pct(m.error_recovery_rate),
        switch = m.avg_projects_per_active_day,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(id: &str, project: &str, msgs: &[(&str, &str)]) -> RawSession {
        RawSession {
            id: id.into(),
            title: None,
            tool: "claude-code".into(),
            project: Some(project.into()),
            model: Some("claude-opus".into()),
            started_at: Some("2026-06-24T09:00:00Z".into()),
            ended_at: Some("2026-06-24T09:12:00Z".into()),
            messages: msgs
                .iter()
                .map(|(r, c)| RawMessage {
                    role: r.to_string(),
                    content: c.to_string(),
                    tools: Vec::new(),
                    ts: None,
                })
                .collect(),
        }
    }

    #[test]
    fn counts_turns_corrections_and_rates() {
        let sessions = vec![
            sess(
                "a",
                "/p",
                &[
                    ("user", "add a retry with backoff, it must be idempotent"),
                    ("assistant", "done"),
                    ("user", "no, that's wrong — revert"),
                    ("assistant", "fixed"),
                ],
            ),
            sess(
                "b",
                "/p",
                &[("user", "write tests"), ("assistant", "ok done")],
            ),
        ];
        let (m, per) = analyze(&sessions);
        assert_eq!(m.sessions, 2);
        assert_eq!(m.median_user_turns, 1.5);
        assert!(m.correction_rate > 0.4 && m.correction_rate < 0.6); // 1 of 2
        assert!(m.specificity_rate > 0.4); // "must" in session a's opener
        assert!(m.test_mention_rate > 0.4); // session b mentions tests
        assert_eq!(per.len(), 2);
        let a = per.iter().find(|p| p.id == "a").unwrap();
        assert_eq!(a.corrections, 1);
        assert!(a.resolved); // ends on assistant
    }

    #[test]
    fn width_signals_actions_languages_model_duration() {
        let mut s = sess(
            "a",
            "/p",
            &[
                ("user", "add a function\n```rust\nfn x() {}\n```"),
                ("assistant", "done"),
            ],
        );
        // Attach agent actions to the assistant turn.
        s.messages[1].tools = vec!["Edit".into(), "Bash".into()];
        let (m, _) = analyze(&[s]);
        assert!((m.avg_tool_actions - 2.0).abs() < 1e-9);
        assert!(m.top_actions.iter().any(|(k, _)| k == "Edit"));
        assert!(m.top_languages.iter().any(|(k, _)| k == "rust"));
        assert!(m.by_model.iter().any(|(k, _)| k == "claude-opus"));
        assert!((m.median_session_minutes - 12.0).abs() < 0.5); // 09:00 → 09:12
        assert!(summarize(&m).contains("Languages: rust"));
    }

    #[test]
    fn empty_is_safe() {
        let (m, per) = analyze(&[]);
        assert_eq!(m.sessions, 0);
        assert_eq!(m.median_user_turns, 0.0);
        assert!(per.is_empty());
        assert!(summarize(&m).contains("Sessions analyzed: 0"));
    }
}
