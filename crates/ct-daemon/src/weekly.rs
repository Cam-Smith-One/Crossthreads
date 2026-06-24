//! Proactive weekly review — "here's how you worked this week, and one thing to
//! try." Generated from the last 7 days of [`behavior`] metrics plus a sample of
//! recent sessions, cached in the index (`meta_kv`) so the UI can surface it
//! without regenerating, and refreshed lazily (or in the background on daemon
//! start) once it's a week stale. Degrades to the deterministic metrics when no
//! model is configured.

use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use ct_store::{behavior, Filters, Store};
use serde::{Deserialize, Serialize};

/// The cached weekly review. Stored as JSON under `meta_kv["weekly_review"]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeeklyReview {
    /// Markdown body.
    pub markdown: String,
    /// When this review was generated (RFC3339).
    pub generated_at: String,
    /// Inclusive lower bound of the window (YYYY-MM-DD).
    pub period_start: String,
    /// Whether a model was used (vs. deterministic metrics only).
    pub llm_used: bool,
}

const KEY: &str = "weekly_review";

/// How many days old a cached review may be before it's considered stale.
const STALE_DAYS: i64 = 7;

fn week_ago_date() -> String {
    (Utc::now() - Duration::days(STALE_DAYS))
        .format("%Y-%m-%d")
        .to_string()
}

/// The cached review, if any.
pub fn cached(store: &Store) -> Result<Option<WeeklyReview>> {
    let Some(raw) = store.meta_get(KEY)? else {
        return Ok(None);
    };
    Ok(serde_json::from_str(&raw).ok())
}

/// True when there's no cached review or it's at least [`STALE_DAYS`] old.
pub fn is_stale(store: &Store) -> bool {
    match cached(store) {
        Ok(Some(r)) => {
            let cutoff = Utc::now() - Duration::days(STALE_DAYS);
            chrono::DateTime::parse_from_rfc3339(&r.generated_at)
                .map(|t| t.with_timezone(&Utc) < cutoff)
                .unwrap_or(true)
        }
        _ => true,
    }
}

/// Data gathered for a review (no store handle held; the model call follows).
pub struct Gathered {
    summary: String,
    corpus: String,
    sessions: i64,
    period_start: String,
}

/// Gather the last-7-days metrics + sample sessions (read-only).
pub fn gather(store: &Store) -> Result<Gathered> {
    let period_start = week_ago_date();
    let filters = Filters {
        since: Some(period_start.clone()),
        ..Default::default()
    };
    let (metrics, _) = store.work_metrics(&filters)?;
    let summary = behavior::summarize(&metrics);

    // Recent sessions for color (most are within the window).
    let samples = store.recent_conversations(30, 1000)?;
    let mut corpus = String::new();
    for c in &samples {
        corpus.push_str(&format!(
            "\n### [{}] {}\n{}\n",
            c.tool,
            c.title.as_deref().unwrap_or("(untitled)"),
            c.text.trim()
        ));
        if corpus.len() > 24_000 {
            break;
        }
    }
    Ok(Gathered {
        summary,
        corpus,
        sessions: metrics.sessions,
        period_start,
    })
}

/// Synthesize the review from gathered data (calls the model; no store lock).
pub fn synthesize(g: Gathered) -> WeeklyReview {
    let Gathered {
        summary,
        corpus,
        sessions,
        period_start,
    } = g;

    let (markdown, llm_used) = if sessions == 0 {
        (
            "No sessions in the last 7 days — nothing to review.".to_string(),
            false,
        )
    } else if ct_llm::available() {
        let system = "You write a developer's weekly review: warm, specific, and short. You \
                      ground every claim in the metrics provided and end with exactly one \
                      concrete thing to try next week.";
        let user = format!(
            "This developer's metrics for the last 7 days:\n\n{summary}\n\n\
             A sample of this week's sessions:\n{corpus}\n\n\
             Write a WEEKLY REVIEW in Markdown with three short parts: (1) what you worked on, \
             (2) how you worked — cite 2–3 of the figures above, (3) **One thing to try next \
             week** — a single concrete, achievable change. Keep it under ~200 words."
        );
        match ct_llm::complete_with(system, &user, 1024) {
            Ok(md) => (md, true),
            // Model failed — fall back to the deterministic summary.
            Err(_) => (format!("## This week, by the numbers\n\n{summary}"), false),
        }
    } else {
        (
            format!(
                "## This week, by the numbers\n\n{summary}\n\n\
                 _Add a model in Settings → Models for a narrative review and a suggestion._"
            ),
            false,
        )
    };

    WeeklyReview {
        markdown,
        generated_at: Utc::now().to_rfc3339(),
        period_start,
        llm_used,
    }
}

/// Persist a review to the index cache (writer only).
pub fn persist(store: &Store, review: &WeeklyReview) -> Result<()> {
    store
        .meta_set(
            KEY,
            &serde_json::to_string(review).context("encoding review")?,
        )
        .context("caching weekly review")
}
