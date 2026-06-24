//! `crossthreads metrics` — deterministic behavioral metrics ("how you work")
//! over your indexed sessions. The numbers the Work DNA / gaps / prompt-coach
//! insights are grounded in. Same data as the daemon `metrics` op and the
//! `crossthreads_metrics` MCP tool. Offline; no model.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::behavior;
use ct_store::{Filters, Store};

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut db: Option<PathBuf> = None;
    let mut filters = Filters::default();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tool" => filters.tool = it.next().cloned(),
            "--project" => filters.project = it.next().cloned(),
            "--since" => filters.since = it.next().cloned(),
            "--until" => filters.until = it.next().cloned(),
            "--db" => {
                db = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?,
                ))
            }
            other => bail!("unknown option for `metrics`: {other}"),
        }
    }

    let store = Store::open(crate::resolve_db(db)?)?;
    let (metrics, _per) = store.work_metrics(&filters)?;
    if metrics.sessions == 0 {
        println!("No sessions indexed yet — run `crossthreads index` first.");
        return Ok(ExitCode::SUCCESS);
    }
    println!("{}", behavior::summarize(&metrics));
    Ok(ExitCode::SUCCESS)
}
