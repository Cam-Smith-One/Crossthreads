//! The **4D AI-Fluency** view — a frame on top of the deterministic behavioral
//! metrics ([`ct_store::behavior`]).
//!
//! Inspired by the 4D AI-Fluency framework (Delegation, Description,
//! Discernment, Diligence), this organizes the flat bag of metrics into four
//! named dimensions, scores each 0–100, and — crucially — surfaces a
//! **reflective question, not a prescription**. Where [`crate::optimize`] tells
//! you *"do X, measured"*, this asks *"is X something you want to keep doing
//! yourself?"* — the agency/intentionality half of understanding how you work.
//!
//! Everything here is deterministic (no model). Scores are transparent blends of
//! metrics we already compute; each dimension reports which signals fed it.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use ct_store::{behavior::WorkMetrics, Filters, Store};
use serde::{Deserialize, Serialize};

/// Default window (days) the fluency scores are computed over.
pub const WINDOW_DAYS: i64 = 30;

// --- Normalization helpers ------------------------------------------------

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}
/// Health where **higher input is better**: map `[lo, hi]` → `[0, 1]`.
fn up(v: f64, lo: f64, hi: f64) -> f64 {
    clamp01((v - lo) / (hi - lo))
}
/// Health where **lower input is better**: map `[lo, hi]` → `[1, 0]`.
fn down(v: f64, lo: f64, hi: f64) -> f64 {
    clamp01((hi - v) / (hi - lo))
}
fn to100(x: f64) -> u8 {
    (x * 100.0).round().clamp(0.0, 100.0) as u8
}
fn pct(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

// --- The four dimensions --------------------------------------------------

/// One of the four fluency dimensions, scored and explained.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimension {
    /// Stable key: `delegation` | `description` | `discernment` | `diligence`.
    pub key: String,
    /// Display name.
    pub label: String,
    /// One-line definition of what the dimension measures.
    pub blurb: String,
    /// 0–100 health score for the current window.
    pub score: u8,
    /// Score-point change vs. the previous window (`None` when no prior data).
    pub delta: Option<i16>,
    /// A short, data-grounded read of what's driving the score.
    pub read: String,
    /// The metrics that fed the score: `(label, formatted value)`.
    pub components: Vec<(String, String)>,
}

/// A reflective prompt — an open question raised by the data, meant to be sat
/// with (or discussed via `ask`), **not** a prescription to execute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reflection {
    /// The dimension the question springs from.
    pub dimension: String,
    /// The open question itself.
    pub prompt: String,
}

/// The full fluency report for a window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FluencyReport {
    pub sessions: i64,
    pub window_days: i64,
    /// Mean of the four dimension scores.
    pub overall: u8,
    pub dimensions: Vec<Dimension>,
    /// A single reflective question, when the data raises one worth asking.
    pub reflection: Option<Reflection>,
    /// A rendered Markdown view (for the CLI / MCP / UI).
    pub markdown: String,
}

/// Score the four dimensions from a window's metrics. Returns them in display
/// order with their component breakdown. `delta` is filled in by [`report`].
fn score_dimensions(m: &WorkMetrics) -> Vec<Dimension> {
    // --- Delegation: what/how much you hand to the agent, and how focused. ---
    // Breadth of automation (agent actions), falling back to tool diversity when
    // a tool doesn't record actions; balanced against context-switching.
    let breadth = if m.avg_tool_actions > 0.0 {
        up(m.avg_tool_actions, 0.0, 8.0)
    } else {
        up(m.by_tool.len() as f64, 1.0, 3.0)
    };
    let focus = down(m.avg_projects_per_active_day, 1.0, 5.0);
    let delegation = (breadth + focus) / 2.0;
    let delegation_read = format!(
        "You average {:.1} agent action(s) per session across {} tool(s), touching {:.1} \
         project(s) a day. Mature delegation is broad but deliberate — leaning on the agent \
         without thrashing between contexts.",
        m.avg_tool_actions,
        m.by_tool.len(),
        m.avg_projects_per_active_day,
    );

    // --- Description: how well you instruct the agent. ---
    let spec = up(m.specificity_rate, 0.0, 0.5);
    let miss = down(m.first_prompt_miss_rate, 0.0, 0.4);
    let description = (spec + miss) / 2.0;
    let description_read = format!(
        "{} of your opening prompts state explicit constraints, and {} miss on the very next \
         turn. Clear, constrained first messages are the cheapest lever you have.",
        pct(m.specificity_rate),
        pct(m.first_prompt_miss_rate),
    );

    // --- Discernment: how well you evaluate what comes back. ---
    let corr = down(m.correction_rate, 0.0, 0.6);
    let aband = down(m.abandonment_rate, 0.0, 0.5);
    let discernment = (corr + aband) / 2.0;
    let discernment_read = format!(
        "{} of sessions needed a correction and {} trailed off unresolved. Catching a wrong \
         turn early — and closing the loop — is the discernment half of working with an agent.",
        pct(m.correction_rate),
        pct(m.abandonment_rate),
    );

    // --- Diligence: responsible, verified use. ---
    let tests = up(m.test_mention_rate, 0.0, 0.4);
    let has_errors = m.error_paste_rate >= 0.05;
    let diligence = if has_errors {
        // Weight test coverage, add error-recovery as a real signal when errors appear.
        (tests * 2.0 + clamp01(m.error_recovery_rate)) / 3.0
    } else {
        tests
    };
    let diligence_read = if has_errors {
        format!(
            "{} of sessions mention a test, and of the ones where an error surfaced, {} still \
             ended resolved. Verification is what keeps speed from turning into silent regressions.",
            pct(m.test_mention_rate),
            pct(m.error_recovery_rate),
        )
    } else {
        format!(
            "{} of sessions mention a test. Asking for a test alongside the change is the \
             diligence step that's easiest to skip when moving fast.",
            pct(m.test_mention_rate),
        )
    };

    vec![
        Dimension {
            key: "delegation".into(),
            label: "Delegation".into(),
            blurb: "What you hand to the agent, and how deliberately.".into(),
            score: to100(delegation),
            delta: None,
            read: delegation_read,
            components: vec![
                (
                    "agent actions/session".into(),
                    format!("{:.1}", m.avg_tool_actions),
                ),
                ("tools used".into(), m.by_tool.len().to_string()),
                (
                    "projects/active-day".into(),
                    format!("{:.1}", m.avg_projects_per_active_day),
                ),
            ],
        },
        Dimension {
            key: "description".into(),
            label: "Description".into(),
            blurb: "How clearly you instruct the agent.".into(),
            score: to100(description),
            delta: None,
            read: description_read,
            components: vec![
                ("prompt specificity".into(), pct(m.specificity_rate)),
                ("first-prompt miss".into(), pct(m.first_prompt_miss_rate)),
            ],
        },
        Dimension {
            key: "discernment".into(),
            label: "Discernment".into(),
            blurb: "How well you evaluate what comes back.".into(),
            score: to100(discernment),
            delta: None,
            read: discernment_read,
            components: vec![
                ("rework rate".into(), pct(m.correction_rate)),
                ("unresolved rate".into(), pct(m.abandonment_rate)),
            ],
        },
        Dimension {
            key: "diligence".into(),
            label: "Diligence".into(),
            blurb: "Responsible, verified use.".into(),
            score: to100(diligence),
            delta: None,
            read: diligence_read,
            components: vec![
                ("tests mentioned".into(), pct(m.test_mention_rate)),
                ("error-recovery".into(), pct(m.error_recovery_rate)),
            ],
        },
    ]
}

/// Pick a single reflective question from the current metrics. The rules are
/// ordered by salience; the first that fires wins. Unlike a lever, a reflection
/// never tells the user what to do — it hands them an open question about
/// *agency*: what to keep owning even when the agent could do it faster.
fn reflect(m: &WorkMetrics) -> Option<Reflection> {
    // Leaning hard on the agent while skipping verification → keep-a-skill question.
    if m.avg_tool_actions >= 4.0 && m.test_mention_rate < 0.15 {
        return Some(Reflection {
            dimension: "diligence".into(),
            prompt: format!(
                "You lean on the agent to write and run code ({:.1} actions/session), but only {} \
                 of your sessions mention a test. What's one part of the work — reviewing, testing, \
                 understanding the change — you want to keep doing yourself, even if the agent \
                 could do it faster?",
                m.avg_tool_actions,
                pct(m.test_mention_rate),
            ),
        });
    }
    // Lots of context switching → what actually needed you?
    if m.avg_projects_per_active_day > 3.0 {
        return Some(Reflection {
            dimension: "delegation".into(),
            prompt: format!(
                "You moved across ~{:.1} projects a day this month. Looking back, which of those \
                 genuinely needed you in the moment — and which could have waited until you had a \
                 clear run at them?",
                m.avg_projects_per_active_day,
            ),
        });
    }
    // Under-specified openers that keep needing corrections → is that how you think?
    if m.specificity_rate < 0.2 && m.correction_rate > 0.4 {
        return Some(Reflection {
            dimension: "description".into(),
            prompt: format!(
                "Your opening prompts rarely state constraints ({} do), and {} of sessions need a \
                 correction. Do you want to slow down that first message — or is the back-and-forth \
                 actually how you figure out what you want?",
                pct(m.specificity_rate),
                pct(m.correction_rate),
            ),
        });
    }
    // Threads trailing off → what's pulling you away mid-task?
    if m.abandonment_rate > 0.3 {
        return Some(Reflection {
            dimension: "discernment".into(),
            prompt: format!(
                "{} of your sessions trail off without a clear resolution. When you leave a thread \
                 hanging, is it usually because the work was done, or because something else pulled \
                 you away before you could confirm it?",
                pct(m.abandonment_rate),
            ),
        });
    }
    // Healthy: the keep-doing-it-yourself question, unconditionally worthwhile.
    Some(Reflection {
        dimension: "delegation".into(),
        prompt: "Your fluency looks balanced this month. What's one thing you did with the agent \
                 that you'd want to keep doing yourself — even if it could do it faster — because \
                 the doing is where you learn or where your judgment lives?"
            .into(),
    })
}

/// Metrics over the window `[now - back - span, now - back]`.
fn window(store: &Store, now: DateTime<Utc>, back: i64, span: i64) -> Result<WorkMetrics> {
    let d = |dt: DateTime<Utc>| dt.format("%Y-%m-%d").to_string();
    let f = Filters {
        since: Some(d(now - Duration::days(back + span))),
        until: Some(d(now - Duration::days(back))),
        ..Default::default()
    };
    Ok(store.work_metrics(&f)?.0)
}

/// Build the full fluency report over the last `window_days`, with each
/// dimension's delta vs. the prior window and a reflective question.
pub fn report(store: &Store, now: DateTime<Utc>, window_days: i64) -> Result<FluencyReport> {
    let days = window_days.clamp(1, 365);
    let cur = window(store, now, 0, days)?;

    if cur.sessions == 0 {
        return Ok(FluencyReport {
            sessions: 0,
            window_days: days,
            overall: 0,
            dimensions: Vec::new(),
            reflection: None,
            markdown: format!("No sessions in the last {days} days yet — nothing to reflect on.\n"),
        });
    }

    let mut dims = score_dimensions(&cur);

    // Deltas vs. the previous window (only when it had sessions).
    let prev = window(store, now, days, days)?;
    if prev.sessions > 0 {
        let prev_dims = score_dimensions(&prev);
        for (d, p) in dims.iter_mut().zip(prev_dims.iter()) {
            d.delta = Some(d.score as i16 - p.score as i16);
        }
    }

    let overall =
        (dims.iter().map(|d| d.score as u32).sum::<u32>() as f64 / dims.len() as f64).round() as u8;
    let reflection = reflect(&cur);
    let markdown = render(&dims, overall, cur.sessions, days, reflection.as_ref());

    Ok(FluencyReport {
        sessions: cur.sessions,
        window_days: days,
        overall,
        dimensions: dims,
        reflection,
        markdown,
    })
}

/// [`report`] at the current time (for the CLI / cron, which needn't pull chrono).
pub fn report_now(store: &Store, window_days: i64) -> Result<FluencyReport> {
    report(store, Utc::now(), window_days)
}

fn render(
    dims: &[Dimension],
    overall: u8,
    sessions: i64,
    days: i64,
    reflection: Option<&Reflection>,
) -> String {
    let mut md = format!(
        "## Your AI-fluency\nOverall **{overall}/100** across {sessions} session(s) in the last \
         {days} days — the four dimensions of working well with an agent.\n\n"
    );
    for d in dims {
        let arrow = match d.delta {
            Some(x) if x > 0 => format!(" (▲ +{x})"),
            Some(x) if x < 0 => format!(" (▼ {x})"),
            Some(_) => " (→)".to_string(),
            None => String::new(),
        };
        md.push_str(&format!(
            "### {} — {}/100{}\n{}\n{}\n\n",
            d.label, d.score, arrow, d.blurb, d.read
        ));
    }
    if let Some(r) = reflection {
        md.push_str(&format!(
            "## A question to sit with\n{}\n\n_Not a to-do — a prompt for reflection. Ask \
             Crossthreads to talk it through, or just let it sit._\n",
            r.prompt
        ));
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_are_direction_aware() {
        // Strong describer/discerner: specific prompts, few corrections/abandons.
        let good = WorkMetrics {
            specificity_rate: 0.6,
            first_prompt_miss_rate: 0.05,
            correction_rate: 0.1,
            abandonment_rate: 0.05,
            ..Default::default()
        };
        let dims = score_dimensions(&good);
        let by = |k: &str| dims.iter().find(|d| d.key == k).unwrap().score;
        assert!(by("description") >= 80, "clear prompts should score high");
        assert!(by("discernment") >= 80, "low rework should score high");

        // Weak: vague prompts, lots of rework.
        let bad = WorkMetrics {
            specificity_rate: 0.0,
            first_prompt_miss_rate: 0.4,
            correction_rate: 0.6,
            abandonment_rate: 0.5,
            ..Default::default()
        };
        let dims = score_dimensions(&bad);
        let by = |k: &str| dims.iter().find(|d| d.key == k).unwrap().score;
        assert!(by("description") <= 20);
        assert!(by("discernment") <= 20);
    }

    #[test]
    fn diligence_ignores_recovery_when_no_errors() {
        // No errors pasted → error-recovery shouldn't drag the score down.
        let m = WorkMetrics {
            test_mention_rate: 0.4,
            error_paste_rate: 0.0,
            error_recovery_rate: 0.0,
            ..Default::default()
        };
        let dims = score_dimensions(&m);
        let dil = dims.iter().find(|d| d.key == "diligence").unwrap();
        assert!(dil.score >= 95, "full test coverage → high diligence");
    }

    #[test]
    fn reflection_is_a_question_not_a_prescription() {
        // Heavy delegation + low tests → the keep-a-skill question, on diligence.
        let m = WorkMetrics {
            avg_tool_actions: 6.0,
            test_mention_rate: 0.05,
            ..Default::default()
        };
        let r = reflect(&m).expect("a reflection should surface");
        assert_eq!(r.dimension, "diligence");
        assert!(r.prompt.contains('?'), "a reflection is a question");

        // Balanced metrics still yield the unconditional keep-doing-it question.
        let healthy = WorkMetrics {
            avg_tool_actions: 2.0,
            test_mention_rate: 0.5,
            specificity_rate: 0.5,
            ..Default::default()
        };
        let r = reflect(&healthy).unwrap();
        assert!(r.prompt.contains('?'));
    }
}
