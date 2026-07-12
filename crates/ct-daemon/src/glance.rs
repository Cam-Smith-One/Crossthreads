//! The **menu-bar glance** — a single, cheap, deterministic bundle for the
//! desktop tray / popover (and anyone who wants an at-a-glance status).
//!
//! It fuses two things no other tool combines:
//! - **Usage** — how much you're leaning on each AI tool right now (sessions
//!   today vs. your own baseline; this week's volume). Real, local, no quota
//!   scraping. A `quota` slot is reserved for live rate-limit windows that an
//!   on-device reader fills in ([`read_quota`]); it's `None` where unavailable.
//! - **Insight** — the behavioral read: your fluency score (and weakest
//!   dimension), how many recent threads are unresolved, and the single focus
//!   the optimize loop is tracking.
//!
//! Everything here is deterministic (no model) and computed under a read lock.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use ct_store::{behavior::WorkMetrics, Filters, Store};
use serde::{Deserialize, Serialize};

use crate::{fluency, optimize};

/// Days used to establish a per-tool "typical day" baseline.
const BASELINE_DAYS: i64 = 30;

/// A live rate-limit / quota window for a provider. Populated only when a local
/// on-device reader can find it ([`read_quota`]); serialized so the tray can
/// draw a reset countdown when present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaWindow {
    /// Amount consumed in the current window (units are provider-specific).
    pub used: f64,
    /// The window's limit, in the same units.
    pub limit: f64,
    /// ISO-8601 instant the window resets.
    pub resets_at: String,
    /// Where the number came from (e.g. `claude-code-cli`), for transparency.
    pub source: String,
}

/// Per-tool usage: how much you're using this tool now vs. your own norm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub tool: String,
    pub today_sessions: i64,
    pub week_sessions: i64,
    /// Your typical sessions/day for this tool over the baseline window.
    pub avg_daily_sessions: f64,
    /// Live quota window, when an on-device reader can supply it. Usually `None`.
    pub quota: Option<QuotaWindow>,
}

/// The full at-a-glance bundle for the tray popover.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Glance {
    pub generated_at: String,
    /// Sessions started today (all tools).
    pub today_sessions: i64,
    /// Distinct projects touched today.
    pub today_projects: i64,
    /// Sessions in the last 7 days (all tools).
    pub week_sessions: i64,
    /// Per-tool usage, busiest-today first.
    pub providers: Vec<ProviderUsage>,
    /// AI-fluency overall score (0–100) over the last 30 days.
    pub fluency_overall: u8,
    /// The weakest fluency dimension right now: `(label, score)`.
    pub fluency_weakest: Option<(String, u8)>,
    /// Estimated count of recent threads left unresolved (deterministic proxy
    /// for "open loops": abandonment rate × sessions over the last 7 days).
    pub unresolved: i64,
    /// The optimize loop's current focus (its prescription title), if any.
    pub focus: Option<String>,
}

fn date(d: DateTime<Utc>) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Metrics over `[since, until]` inclusive (calendar dates).
fn window(store: &Store, since: &str, until: &str) -> Result<WorkMetrics> {
    let f = Filters {
        since: Some(since.to_string()),
        until: Some(until.to_string()),
        ..Default::default()
    };
    Ok(store.work_metrics(&f)?.0)
}

/// Best-effort live quota reader for a tool. Rate-limit windows live in
/// provider-specific local state whose formats aren't stable across versions
/// and can't be captured in a headless environment, so this returns `None`
/// today and is the single drop-in point for an on-device reader. Kept as a
/// function (not a stub inline) so wiring a real source is a one-place change.
fn read_quota(_tool: &str) -> Option<QuotaWindow> {
    None
}

/// Assemble the glance at `now`. Deterministic; read-only.
pub fn build(store: &Store, now: DateTime<Utc>) -> Result<Glance> {
    let today = date(now);
    let week_start = date(now - Duration::days(6));
    let base_start = date(now - Duration::days(BASELINE_DAYS - 1));

    let today_m = window(store, &today, &today)?;
    let week_m = window(store, &week_start, &today)?;
    let base_m = window(store, &base_start, &today)?;

    // Per-tool usage, keyed off today's tools then anything active this week.
    let today_by: std::collections::BTreeMap<String, i64> =
        today_m.by_tool.iter().cloned().collect();
    let week_by: std::collections::BTreeMap<String, i64> = week_m.by_tool.iter().cloned().collect();
    let base_by: std::collections::BTreeMap<String, i64> = base_m.by_tool.iter().cloned().collect();

    let mut tools: Vec<String> = week_by.keys().cloned().collect();
    // Stable order: busiest today first, then busiest this week, then name.
    tools.sort_by(|a, b| {
        today_by
            .get(b)
            .unwrap_or(&0)
            .cmp(today_by.get(a).unwrap_or(&0))
            .then_with(|| {
                week_by
                    .get(b)
                    .unwrap_or(&0)
                    .cmp(week_by.get(a).unwrap_or(&0))
            })
            .then_with(|| a.cmp(b))
    });

    let providers = tools
        .into_iter()
        .map(|tool| {
            let avg = *base_by.get(&tool).unwrap_or(&0) as f64 / BASELINE_DAYS as f64;
            ProviderUsage {
                today_sessions: *today_by.get(&tool).unwrap_or(&0),
                week_sessions: *week_by.get(&tool).unwrap_or(&0),
                avg_daily_sessions: avg,
                quota: read_quota(&tool),
                tool,
            }
        })
        .collect();

    // Insight half.
    let f = fluency::report(store, now, fluency::WINDOW_DAYS)?;
    let fluency_weakest = f
        .dimensions
        .iter()
        .min_by_key(|d| d.score)
        .map(|d| (d.label.clone(), d.score));
    let unresolved = (week_m.abandonment_rate * week_m.sessions as f64).round() as i64;

    Ok(Glance {
        generated_at: now.to_rfc3339(),
        today_sessions: today_m.sessions,
        today_projects: today_m.projects,
        week_sessions: week_m.sessions,
        providers,
        fluency_overall: f.overall,
        fluency_weakest,
        unresolved,
        focus: optimize::active_focus(store),
    })
}

/// [`build`] at the current time.
pub fn build_now(store: &Store) -> Result<Glance> {
    build(store, Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_is_absent_without_an_on_device_reader() {
        // The headless default: no fabricated quota numbers.
        assert!(read_quota("claude-code").is_none());
        assert!(read_quota("codex").is_none());
    }
}
