//! `crossthreads activity` — session activity over time (a temporal view).
//! Buckets your indexed sessions by day or week with a per-tool breakdown and
//! draws a small bar chart. Same data as the daemon `activity` op and the
//! `crossthreads_activity` MCP tool. Offline; no model needed.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_store::{Filters, Store};

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut bucket = "day".to_string();
    let mut db: Option<PathBuf> = None;
    let mut filters = Filters::default();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--by" => {
                bucket = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--by needs a value (day|week)"))?
                    .to_lowercase();
                if bucket != "day" && bucket != "week" {
                    bail!("--by must be `day` or `week`");
                }
            }
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
            other => bail!("unknown option for `activity`: {other}"),
        }
    }

    let store = Store::open(crate::resolve_db(db)?)?;
    let periods = store.activity(&bucket, &filters)?;
    if periods.is_empty() {
        println!("No dated sessions to chart yet — run `crossthreads index` first.");
        return Ok(ExitCode::SUCCESS);
    }

    let total: i64 = periods.iter().map(|p| p.total).sum();
    let max = periods.iter().map(|p| p.total).max().unwrap_or(1).max(1);
    println!(
        "Activity by {bucket} — {} period(s), {total} sessions:\n",
        periods.len()
    );
    for p in &periods {
        // Scale a bar to ~32 cells.
        let cells = ((p.total as f64 / max as f64) * 32.0).round() as usize;
        let bar = "█".repeat(cells.max(1));
        let tools = p
            .by_tool
            .iter()
            .take(3)
            .map(|(t, n)| format!("{t} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("{:<10} {:>4}  {bar}  {tools}", p.period, p.total);
    }
    Ok(ExitCode::SUCCESS)
}
