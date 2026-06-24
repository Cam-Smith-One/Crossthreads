//! `crossthreads weekly` — your proactive weekly review: what you worked on,
//! how you worked (grounded in the metrics), and one thing to try next week.
//! Cached in the index and regenerated when a week stale; `--force` regenerates
//! now. Same engine as the daemon `weekly_review` op and the
//! `crossthreads_weekly_review` MCP tool. Good to wire into a weekly cron/launchd.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::weekly;
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut db: Option<PathBuf> = None;
    let mut force = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--force" | "-f" => force = true,
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
    // Use the cached review when it's still fresh, unless --force.
    let review = if !force && !weekly::is_stale(&store) {
        weekly::cached(&store)?.expect("not stale implies a cached review exists")
    } else {
        let g = weekly::gather(&store)?;
        let r = weekly::synthesize(g);
        weekly::persist(&store, &r)?;
        r
    };

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
