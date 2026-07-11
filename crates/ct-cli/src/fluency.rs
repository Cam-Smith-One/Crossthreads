//! `crossthreads fluency` — your AI-fluency across four dimensions
//! (Delegation / Description / Discernment / Diligence), each scored with its
//! movement vs. the previous window, plus a reflective question. Deterministic
//! (no model). Same engine as the daemon `fluency` op and the
//! `crossthreads_fluency` MCP tool.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::fluency;
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut db: Option<PathBuf> = None;
    let mut window_days = fluency::WINDOW_DAYS;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--days" => {
                window_days = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--days needs a value"))?
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--days must be an integer"))?
            }
            "--db" => {
                db = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?,
                ))
            }
            other => bail!("unknown option for `fluency`: {other}"),
        }
    }

    let store = Store::open(crate::resolve_db(db)?)?;
    let report = fluency::report_now(&store, window_days)?;
    println!("{}", report.markdown);
    Ok(ExitCode::SUCCESS)
}
