//! "Optimize how you work" — the closed loop on top of the deterministic
//! behavioral metrics ([`ct_store::behavior`]).
//!
//! Two deterministic pieces (no model needed):
//! - **Trends**: compare the current window to the previous one and report the
//!   direction of each metric.
//! - **The optimization loop**: score a catalog of behavioral *levers* by
//!   expected impact, prescribe the single biggest one, persist it, and on the
//!   next run **measure whether the targeted metric actually moved** (a
//!   validation-gated verdict) — recording a running optimization log.
//!
//! State lives in the index (`meta_kv["optimize"]`) so it survives re-indexing.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use ct_store::{behavior::WorkMetrics, Filters, Store};
use serde::{Deserialize, Serialize};

/// Default comparison window (days) for trends and lever evaluation.
pub const WINDOW_DAYS: i64 = 30;
/// A prescription must be at least this old before we grade it (enough sessions).
const MIN_GRADE_DAYS: i64 = 7;

const KEY: &str = "optimize";

// --- Metrics we reason about ---------------------------------------------

/// A behavioral metric we track / act on. Serialized so a prescription can
/// remember which metric it targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Metric {
    Correction,
    FirstPromptMiss,
    Abandonment,
    Specificity,
    TestMention,
    ContextSwitch,
    MedianTurns,
    SessionMinutes,
}

impl Metric {
    pub fn value(self, m: &WorkMetrics) -> f64 {
        match self {
            Metric::Correction => m.correction_rate,
            Metric::FirstPromptMiss => m.first_prompt_miss_rate,
            Metric::Abandonment => m.abandonment_rate,
            Metric::Specificity => m.specificity_rate,
            Metric::TestMention => m.test_mention_rate,
            Metric::ContextSwitch => m.avg_projects_per_active_day,
            Metric::MedianTurns => m.median_user_turns,
            Metric::SessionMinutes => m.median_session_minutes,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Metric::Correction => "rework rate",
            Metric::FirstPromptMiss => "first-prompt miss",
            Metric::Abandonment => "unresolved rate",
            Metric::Specificity => "prompt specificity",
            Metric::TestMention => "tests mentioned",
            Metric::ContextSwitch => "projects/active-day",
            Metric::MedianTurns => "median turns",
            Metric::SessionMinutes => "median session mins",
        }
    }
    /// For most metrics lower is better; specificity/tests are the exceptions.
    pub fn better_is_lower(self) -> bool {
        !matches!(self, Metric::Specificity | Metric::TestMention)
    }
    fn is_pct(self) -> bool {
        matches!(
            self,
            Metric::Correction
                | Metric::FirstPromptMiss
                | Metric::Abandonment
                | Metric::Specificity
                | Metric::TestMention
        )
    }
    pub fn fmt(self, v: f64) -> String {
        if self.is_pct() {
            format!("{:.0}%", v * 100.0)
        } else {
            format!("{v:.1}")
        }
    }
}

/// The metrics shown in the trends view, in display order.
const TREND_METRICS: [Metric; 7] = [
    Metric::Correction,
    Metric::FirstPromptMiss,
    Metric::Abandonment,
    Metric::Specificity,
    Metric::TestMention,
    Metric::ContextSwitch,
    Metric::MedianTurns,
];

/// One metric's movement between the previous window and the current one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendRow {
    pub label: String,
    pub current: f64,
    pub previous: f64,
    /// Signed relative change (current − previous) / |previous|; 0 if no prior.
    pub delta: f64,
    /// True when the change is in the good direction (given `better_is_lower`).
    pub improved: bool,
    pub is_pct: bool,
}

// --- Levers --------------------------------------------------------------

/// A concrete, testable change, tied to the metric it should move.
#[derive(Debug, Clone)]
pub struct Lever {
    pub id: &'static str,
    pub metric: Metric,
    pub title: String,
    pub prescription: String,
}

/// Score the catalog against `m` and return the single highest-impact lever, if
/// any threshold is crossed. Deterministic; no model.
pub fn top_lever(m: &WorkMetrics) -> Option<Lever> {
    let pct = |x: f64| format!("{:.0}%", x * 100.0);
    let mut cands: Vec<(f64, Lever)> = Vec::new();

    if m.first_prompt_miss_rate > 0.20 {
        cands.push((
            m.first_prompt_miss_rate,
            Lever {
                id: "front_load",
                metric: Metric::FirstPromptMiss,
                title: "Front-load constraints in your opening prompt".into(),
                prescription: format!(
                    "{} of your tasks needed a correction on the very next turn. Before you send \
                     the first message, state the constraints and what \u{201c}done\u{201d} looks \
                     like. Goal: bring first-prompt miss down.",
                    pct(m.first_prompt_miss_rate)
                ),
            },
        ));
    }
    if m.abandonment_rate > 0.25 {
        cands.push((
            m.abandonment_rate,
            Lever {
                id: "close_loops",
                metric: Metric::Abandonment,
                title: "Close the loop before switching tasks".into(),
                prescription: format!(
                    "{} of your sessions trail off unresolved. Before moving on, confirm the fix \
                     landed or jot the next step so you can pick it back up.",
                    pct(m.abandonment_rate)
                ),
            },
        ));
    }
    if m.correction_rate > 0.40 {
        cands.push((
            m.correction_rate * 0.9,
            Lever {
                id: "review_plan",
                metric: Metric::Correction,
                title: "Review the plan before the agent acts".into(),
                prescription: format!(
                    "{} of your sessions needed a correction. Ask the agent to outline its plan \
                     first and approve it before it edits — cheaper than undoing.",
                    pct(m.correction_rate)
                ),
            },
        ));
    }
    if m.test_mention_rate < 0.15 {
        cands.push((
            0.15 - m.test_mention_rate,
            Lever {
                id: "ask_tests",
                metric: Metric::TestMention,
                title: "Ask for a test with the change".into(),
                prescription: format!(
                    "Only {} of your coding sessions mention tests. Add \u{201c}and write a test \
                     that covers it\u{201d} to your requests — it catches regressions the agent \
                     would otherwise miss.",
                    pct(m.test_mention_rate)
                ),
            },
        ));
    }
    if m.avg_projects_per_active_day > 3.0 {
        cands.push((
            (m.avg_projects_per_active_day - 3.0) / 5.0,
            Lever {
                id: "batch_projects",
                metric: Metric::ContextSwitch,
                title: "Batch work by project".into(),
                prescription: format!(
                    "You touch ~{:.1} projects a day; each switch costs ramp-up. Try grouping a \
                     day (or half-day) around one project before moving on.",
                    m.avg_projects_per_active_day
                ),
            },
        ));
    }

    cands
        .into_iter()
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, l)| l)
}

// --- Persisted state -----------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Prescription {
    lever_id: String,
    metric: Metric,
    title: String,
    prescription: String,
    baseline: f64,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LogEntry {
    title: String,
    metric_label: String,
    baseline: f64,
    result: f64,
    verdict: String,
    created_at: String,
    evaluated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    active: Option<Prescription>,
    #[serde(default)]
    log: Vec<LogEntry>,
}

fn load(store: &Store) -> State {
    store
        .meta_get(KEY)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save(store: &Store, state: &State) -> Result<()> {
    store
        .meta_set(
            KEY,
            &serde_json::to_string(state).context("encoding state")?,
        )
        .context("saving optimize state")
}

// --- Windowed metrics + trends -------------------------------------------

fn date(d: DateTime<Utc>) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Metrics over the window `[now - back - span, now - back]`.
fn window(store: &Store, now: DateTime<Utc>, back: i64, span: i64) -> Result<WorkMetrics> {
    let f = Filters {
        since: Some(date(now - Duration::days(back + span))),
        until: Some(date(now - Duration::days(back))),
        ..Default::default()
    };
    Ok(store.work_metrics(&f)?.0)
}

/// Trend rows comparing the last `days` to the `days` before that.
pub fn trends(store: &Store, now: DateTime<Utc>, days: i64) -> Result<Vec<TrendRow>> {
    let cur = window(store, now, 0, days)?;
    let prev = window(store, now, days, days)?;
    Ok(TREND_METRICS
        .iter()
        .map(|&m| {
            let (c, p) = (m.value(&cur), m.value(&prev));
            let delta = if p.abs() > 1e-9 {
                (c - p) / p.abs()
            } else {
                0.0
            };
            let improved = if m.better_is_lower() { c < p } else { c > p };
            TrendRow {
                label: m.label().to_string(),
                current: c,
                previous: p,
                delta,
                improved: improved && (c - p).abs() > 1e-9,
                is_pct: m.is_pct(),
            }
        })
        .collect())
}

// --- The loop ------------------------------------------------------------

/// The rendered optimize report plus whether there's an active focus.
#[derive(Debug, Clone)]
pub struct Report {
    pub markdown: String,
    pub has_active: bool,
}

fn days_since(iso: &str, now: DateTime<Utc>) -> i64 {
    DateTime::parse_from_rfc3339(iso)
        .map(|t| (now - t.with_timezone(&Utc)).num_days())
        .unwrap_or(i64::MAX)
}

fn verdict(metric: Metric, baseline: f64, result: f64) -> &'static str {
    // Relative move of ≥10% counts; direction depends on the metric.
    let improved = if metric.better_is_lower() {
        result < baseline * 0.9
    } else {
        result > baseline * 1.1 || (baseline < 1e-9 && result > 0.0)
    };
    let worse = if metric.better_is_lower() {
        result > baseline * 1.1
    } else {
        result < baseline * 0.9
    };
    if improved {
        "improved"
    } else if worse {
        "regressed"
    } else {
        "no clear change"
    }
}

/// Convenience: [`run`] at the current time (for the CLI / cron).
pub fn run_now(store: &Store, force: bool) -> Result<Report> {
    run(store, Utc::now(), force)
}

/// Convenience: [`trends`] at the current time.
pub fn trends_now(store: &Store, days: i64) -> Result<Vec<TrendRow>> {
    trends(store, Utc::now(), days)
}

/// Run one turn of the loop: grade the active prescription when it's ripe, then
/// (if none is active) prescribe the current biggest lever. Persists state.
pub fn run(store: &Store, now: DateTime<Utc>, force: bool) -> Result<Report> {
    let cur = window(store, now, 0, WINDOW_DAYS)?;
    if cur.sessions == 0 {
        return Ok(Report {
            markdown: "No sessions in the last 30 days yet — nothing to optimize.".into(),
            has_active: false,
        });
    }
    let mut state = load(store);
    let mut graded: Option<LogEntry> = None;

    // Grade the active prescription once it's had time to take effect.
    if let Some(p) = state.active.clone() {
        if force || days_since(&p.created_at, now) >= MIN_GRADE_DAYS {
            let result = p.metric.value(&cur);
            let entry = LogEntry {
                title: p.title.clone(),
                metric_label: p.metric.label().to_string(),
                baseline: p.baseline,
                result,
                verdict: verdict(p.metric, p.baseline, result).to_string(),
                created_at: p.created_at.clone(),
                evaluated_at: now.to_rfc3339(),
            };
            state.log.push(entry.clone());
            if state.log.len() > 30 {
                let drop = state.log.len() - 30;
                state.log.drain(0..drop);
            }
            graded = Some(entry);
            state.active = None;
        }
    }

    // Prescribe the next lever if the field is clear.
    if state.active.is_none() {
        if let Some(l) = top_lever(&cur) {
            state.active = Some(Prescription {
                lever_id: l.id.to_string(),
                metric: l.metric,
                title: l.title.clone(),
                prescription: l.prescription.clone(),
                baseline: l.metric.value(&cur),
                created_at: now.to_rfc3339(),
            });
        }
    }

    save(store, &state)?;

    // Render.
    let mut md = String::new();
    if let Some(g) = &graded {
        md.push_str(&format!(
            "## Last change you tried\n**{}** — {}: {} → {} (**{}**).\n\n",
            g.title,
            g.metric_label,
            fmt_val(g.baseline, &g.metric_label),
            fmt_val(g.result, &g.metric_label),
            g.verdict,
        ));
    }
    match &state.active {
        Some(p) => md.push_str(&format!(
            "## Your focus now\n**{}**\n\n{}\n\n_Baseline {}: {}. I'll check back in ~{} days._\n\n",
            p.title,
            p.prescription,
            p.metric.label(),
            p.metric.fmt(p.baseline),
            MIN_GRADE_DAYS,
        )),
        None => md.push_str(
            "## Your focus now\nNothing is clearly costing you time right now — your metrics are \
             in good shape. Keep going.\n\n",
        ),
    }
    md.push_str("## Trends (last 30 days vs previous)\n");
    for r in trends(store, now, WINDOW_DAYS)? {
        let arrow = if r.delta.abs() < 1e-9 {
            "→"
        } else if r.improved {
            "↑ better"
        } else {
            "↓ worse"
        };
        let fmt = |v: f64| {
            if r.is_pct {
                format!("{:.0}%", v * 100.0)
            } else {
                format!("{v:.1}")
            }
        };
        md.push_str(&format!(
            "- {}: {} (was {}) {}\n",
            r.label,
            fmt(r.current),
            fmt(r.previous),
            arrow
        ));
    }
    if !state.log.is_empty() {
        md.push_str("\n## Optimization log\n");
        for e in state.log.iter().rev().take(6) {
            md.push_str(&format!(
                "- {}: {} → {} ({})\n",
                e.title,
                fmt_val(e.baseline, &e.metric_label),
                fmt_val(e.result, &e.metric_label),
                e.verdict
            ));
        }
    }

    Ok(Report {
        markdown: md,
        has_active: state.active.is_some(),
    })
}

/// Best-effort value formatting when we only kept the metric *label* (rates that
/// read as `%` vs plain numbers).
fn fmt_val(v: f64, label: &str) -> String {
    let is_pct = label.contains("rate")
        || label.contains("miss")
        || label.contains("specificity")
        || label.contains("tests");
    if is_pct {
        format!("{:.0}%", v * 100.0)
    } else {
        format!("{v:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_lever_picks_the_worst_metric() {
        // High first-prompt miss dominates → the front-load lever.
        let m = WorkMetrics {
            first_prompt_miss_rate: 0.5,
            abandonment_rate: 0.30,
            ..Default::default()
        };
        let l = top_lever(&m).expect("a lever should trigger");
        assert_eq!(l.id, "front_load");
        assert_eq!(l.metric, Metric::FirstPromptMiss);

        // Healthy metrics (good test coverage, low friction) → no lever.
        let healthy = WorkMetrics {
            test_mention_rate: 0.5,
            specificity_rate: 0.6,
            ..Default::default()
        };
        assert!(top_lever(&healthy).is_none());
    }

    #[test]
    fn verdict_is_direction_aware() {
        // Lower-is-better metric that dropped a lot → improved.
        assert_eq!(verdict(Metric::Correction, 0.50, 0.30), "improved");
        assert_eq!(verdict(Metric::Correction, 0.30, 0.50), "regressed");
        assert_eq!(verdict(Metric::Correction, 0.50, 0.49), "no clear change");
        // Higher-is-better metric that rose → improved.
        assert_eq!(verdict(Metric::TestMention, 0.10, 0.30), "improved");
    }
}
