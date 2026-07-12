//! The **menu-bar glance** — a single, cheap, deterministic bundle for the
//! desktop tray / popover (and anyone who wants an at-a-glance status).
//!
//! It fuses two things no other tool combines:
//! - **Usage** — how much you're leaning on each AI tool right now (sessions
//!   today vs. your own baseline; this week's volume). Real, local, no quota
//!   scraping. A `quota` slot carries a live rate-limit window when an on-device
//!   reader supplies one via a documented sidecar ([`read_quota_from`]); it's
//!   `None` where unavailable.
//! - **Insight** — the behavioral read: your fluency score (and weakest
//!   dimension), how many recent threads are unresolved, and the single focus
//!   the optimize loop is tracking.
//!
//! Everything here is deterministic (no model) and computed under a read lock.

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use ct_store::{behavior::WorkMetrics, Filters, Store};
use serde::{Deserialize, Serialize};

use crate::{fluency, optimize};

/// Days used to establish a per-tool "typical day" baseline.
const BASELINE_DAYS: i64 = 30;

/// A live rate-limit / quota window for a provider. Populated only when a local
/// on-device reader supplies it ([`read_quota_from`]); serialized so the tray
/// can draw a reset countdown when present.
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

/// How long after `resets_at` a sidecar window is still considered fresh enough
/// to show. Providers' rate-limit windows are undocumented and unstable, so we
/// never scrape them; instead a window is only shown if an on-device reader
/// recently wrote it. Past this grace period we assume the reader is gone/stale.
const QUOTA_STALE_AFTER: Duration = Duration::hours(24);

/// The directory holding per-tool quota sidecars: `$CROSSTHREADS_QUOTA_DIR` when
/// set, else `~/.crossthreads/quota`. `None` if no home dir is resolvable.
fn quota_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CROSSTHREADS_QUOTA_DIR") {
        return Some(PathBuf::from(d));
    }
    dirs::home_dir().map(|h| h.join(".crossthreads").join("quota"))
}

/// Live quota reader for a tool. Crossthreads does **not** scrape provider
/// internals (their rate-limit formats are undocumented and version-unstable);
/// instead it reads a documented sidecar — `<dir>/<tool>.json` shaped like
/// [`QuotaWindow`] — that any on-device helper can write (e.g. from
/// `anthropic-ratelimit-*` response headers). Missing, malformed, or stale
/// (>[`QUOTA_STALE_AFTER`] past `resets_at`) files yield `None`, so the tray
/// shows a real window only when one genuinely exists.
fn read_quota_from(dir: &Path, tool: &str, now: DateTime<Utc>) -> Option<QuotaWindow> {
    let raw = std::fs::read_to_string(dir.join(format!("{tool}.json"))).ok()?;
    let q: QuotaWindow = serde_json::from_str(&raw).ok()?;
    if q.limit <= 0.0 || !q.used.is_finite() || !q.limit.is_finite() {
        return None;
    }
    // Drop windows whose reset is far in the past — the reader has stopped.
    if let Ok(resets) = DateTime::parse_from_rfc3339(&q.resets_at) {
        if resets.with_timezone(&Utc) + QUOTA_STALE_AFTER < now {
            return None;
        }
    }
    Some(q)
}

/// Assemble the glance at `now`, reading quota sidecars from the default
/// [`quota_dir`]. Deterministic; read-only.
pub fn build(store: &Store, now: DateTime<Utc>) -> Result<Glance> {
    build_in(store, now, quota_dir().as_deref())
}

/// Like [`build`], but reads quota sidecars from an explicit directory (`None`
/// disables quota). Exposed so tests and embedders can supply a fixture dir
/// without touching global state.
pub fn build_in(store: &Store, now: DateTime<Utc>, quota_dir: Option<&Path>) -> Result<Glance> {
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
                quota: quota_dir.and_then(|d| read_quota_from(d, &tool, now)),
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

    fn write(dir: &Path, tool: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{tool}.json")), body).unwrap();
    }

    #[test]
    fn quota_absent_when_no_sidecar() {
        let dir = std::env::temp_dir().join(format!("ct-quota-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(read_quota_from(&dir, "claude-code", Utc::now()).is_none());
    }

    #[test]
    fn quota_read_from_fresh_sidecar() {
        let now = Utc::now();
        let dir = std::env::temp_dir().join(format!("ct-quota-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let resets = (now + Duration::hours(3)).to_rfc3339();
        write(
            &dir,
            "claude-code",
            &format!(r#"{{"used":120,"limit":500,"resets_at":"{resets}","source":"my-reader"}}"#),
        );
        let q = read_quota_from(&dir, "claude-code", now).expect("fresh window is read");
        assert_eq!(q.used, 120.0);
        assert_eq!(q.limit, 500.0);
        assert_eq!(q.source, "my-reader");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quota_dropped_when_stale_or_invalid() {
        let now = Utc::now();
        let dir = std::env::temp_dir().join(format!("ct-quota-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Reset far in the past → reader has stopped → ignored.
        let old = (now - Duration::days(5)).to_rfc3339();
        write(
            &dir,
            "claude-code",
            &format!(r#"{{"used":1,"limit":2,"resets_at":"{old}","source":"x"}}"#),
        );
        assert!(read_quota_from(&dir, "claude-code", now).is_none());
        // Nonsensical limit → ignored (no fabricated/garbage windows).
        write(
            &dir,
            "codex",
            r#"{"used":1,"limit":0,"resets_at":"2999-01-01T00:00:00Z","source":"x"}"#,
        );
        assert!(read_quota_from(&dir, "codex", now).is_none());
        // Malformed JSON → ignored, never panics.
        write(&dir, "aider", "not json");
        assert!(read_quota_from(&dir, "aider", now).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
