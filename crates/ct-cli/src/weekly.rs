//! `crossthreads weekly` — your proactive weekly review: what you worked on,
//! how you worked (grounded in the metrics), and one thing to try next week.
//! Cached in the index and regenerated when a week stale; `--force` regenerates
//! now. `--deliver` writes the review to a file and fires a desktop
//! notification — this is what the `crossthreads schedule` launchd/cron entry
//! runs. Same engine as the daemon `weekly_review` op and the
//! `crossthreads_weekly_review` MCP tool.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::weekly;
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut db: Option<PathBuf> = None;
    let mut force = false;
    let mut deliver = false;
    let mut to: Option<PathBuf> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--force" | "-f" => force = true,
            "--deliver" => deliver = true,
            "--to" => {
                to = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--to needs a directory"))?,
                ))
            }
            "--db" => {
                db = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?,
                ))
            }
            other => bail!("unknown option for `weekly`: {other}"),
        }
    }

    let store = Store::open(crate::resolve_db(db)?)?;
    // Delivery always ensures a current review (the scheduler's whole job);
    // interactive runs reuse a fresh cache unless --force.
    let review = weekly::ensure_current(&store, force || deliver)?;

    if deliver {
        let dir = to.unwrap_or_else(reviews_dir);
        let d = weekly::deliver(&store, &review, &dir)?;
        if !d.already_delivered {
            notify(
                "Your Crossthreads weekly review",
                &headline(&review.markdown),
            );
        }
        println!("{}", d.path.display());
        eprintln!(
            "delivered week of {}{}{}",
            review.period_start,
            if review.llm_used {
                ""
            } else {
                " — metrics only"
            },
            if d.already_delivered {
                " (already delivered; refreshed)"
            } else {
                ""
            },
        );
        return Ok(ExitCode::SUCCESS);
    }

    println!("{}", review.markdown);
    eprintln!(
        "\n(week of {}{})",
        review.period_start,
        if review.llm_used {
            ""
        } else {
            " — metrics only"
        }
    );
    Ok(ExitCode::SUCCESS)
}

/// Default delivery directory: `<data dir>/crossthreads/reviews`.
pub(crate) fn reviews_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("crossthreads")
        .join("reviews")
}

/// A short one-line headline for the notification: the first non-heading,
/// non-empty line of the review, trimmed to a sensible length.
fn headline(markdown: &str) -> String {
    let line = markdown
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or("A fresh look at how you worked this week.");
    if line.chars().count() > 140 {
        let mut s: String = line.chars().take(139).collect();
        s.push('…');
        s
    } else {
        line.to_string()
    }
}

/// Best-effort desktop notification. Uses `osascript` on macOS and `notify-send`
/// elsewhere; silently does nothing if neither is available (e.g. a headless
/// server) — delivery still wrote the file, which is the durable artifact.
fn notify(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    let cmd = {
        let script = format!("display notification {:?} with title {:?}", body, title);
        let mut c = std::process::Command::new("osascript");
        c.arg("-e").arg(script);
        c
    };
    #[cfg(not(target_os = "macos"))]
    let cmd = {
        let mut c = std::process::Command::new("notify-send");
        c.arg(title).arg(body);
        c
    };

    let mut cmd = cmd;
    // Detach stdio and swallow the result: notifications are a nicety, never a
    // reason to fail the scheduled job.
    let _ = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}
