//! `crossthreads trends` — how your working metrics are trending (last N days vs
//! the previous N). Deterministic; no model. Same data as the daemon `trends` op
//! and the `crossthreads_trends` MCP tool.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::optimize;
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut db: Option<PathBuf> = None;
    let mut days: i64 = optimize::WINDOW_DAYS;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--days" => {
                days = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--days needs a value"))?
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--days must be a number"))?
            }
            "--db" => {
                db = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?,
                ))
            }
            other => bail!("unknown option for `trends`: {other}"),
        }
    }

    let store = Store::open(crate::resolve_db(db)?)?;
    let rows = optimize::trends_now(&store, days.clamp(1, 365))?;
    println!("Trends — last {days} days vs previous {days}:\n");
    for r in rows {
        let fmt = |v: f64| {
            if r.is_pct {
                format!("{:.0}%", v * 100.0)
            } else {
                format!("{v:.1}")
            }
        };
        let dir = if r.delta.abs() < 1e-9 {
            "  —"
        } else if r.improved {
            "  ↑ better"
        } else {
            "  ↓ worse"
        };
        println!(
            "  {:<22} {:>6}  (was {:>6}){dir}",
            r.label,
            fmt(r.current),
            fmt(r.previous)
        );
    }
    Ok(ExitCode::SUCCESS)
}
