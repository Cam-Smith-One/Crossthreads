//! `crossthreads glance` — the at-a-glance status the menu-bar popover shows:
//! per-tool usage (today vs. your baseline, this week), any live quota window a
//! local sidecar provides, and the insight read (fluency, unresolved threads,
//! optimize focus). `--json` emits the raw bundle. Same engine as the daemon
//! `Glance` op / tray popover.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::glance;
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut db: Option<PathBuf> = None;
    let mut json = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--db" => {
                db = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?,
                ))
            }
            other => bail!("unknown option for `glance`: {other}"),
        }
    }

    let store = Store::open(crate::resolve_db(db)?)?;
    let g = glance::build_now(&store)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&g)?);
        return Ok(ExitCode::SUCCESS);
    }

    println!(
        "Today: {} sessions across {} projects · this week: {}",
        g.today_sessions, g.today_projects, g.week_sessions
    );
    if !g.providers.is_empty() {
        println!("\nBy tool (today · week · ~daily):");
        for p in &g.providers {
            let mut line = format!(
                "  {:14} {:>3} · {:>3} · {:.1}/day",
                p.tool, p.today_sessions, p.week_sessions, p.avg_daily_sessions
            );
            if let Some(q) = &p.quota {
                line.push_str(&format!(
                    "  [quota {:.0}/{:.0}, resets {}]",
                    q.used, q.limit, q.resets_at
                ));
            }
            println!("{line}");
        }
    }
    println!("\nFluency: {}/100", g.fluency_overall);
    if let Some((label, score)) = &g.fluency_weakest {
        println!("  weakest: {label} ({score}/100)");
    }
    println!("Unresolved threads (recent): {}", g.unresolved);
    if let Some(focus) = &g.focus {
        println!("Optimize focus: {focus}");
    }
    Ok(ExitCode::SUCCESS)
}
