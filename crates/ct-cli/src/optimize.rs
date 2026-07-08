//! `crossthreads optimize` — the "optimize how you work" loop: the single
//! highest-impact change to make now, a measured verdict on the last change you
//! tried, and your trends. Deterministic (no model). Same engine as the daemon
//! `optimize` op and the `crossthreads_optimize` MCP tool. Great in a weekly cron.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::optimize;
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
            other => bail!("unknown option for `optimize`: {other}"),
        }
    }

    let store = Store::open(crate::resolve_db(db)?)?;
    let report = optimize::run_now(&store, force)?;
    println!("{}", report.markdown);
    Ok(ExitCode::SUCCESS)
}
