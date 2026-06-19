//! `crossthreads index` — Phase 0 dry-run indexing.

use std::process::ExitCode;

use anyhow::{bail, Result};

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut as_json = false;
    let mut limit: Option<usize> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => as_json = true,
            "--limit" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--limit needs a value"))?;
                limit = Some(v.parse()?);
            }
            other => bail!("unknown option for `index`: {other}"),
        }
    }

    let (conversations, skipped) = crate::collect(limit);

    if as_json {
        println!("{}", serde_json::to_string_pretty(&conversations)?);
        return Ok(ExitCode::SUCCESS);
    }

    if conversations.is_empty() {
        println!("No sessions found. (Looked for built-in connectors' data on this machine.)");
        if skipped > 0 {
            println!("{skipped} session(s) failed to parse — see warnings above.");
        }
        return Ok(ExitCode::SUCCESS);
    }

    println!(
        "Indexed {} conversation(s){}:\n",
        conversations.len(),
        if skipped > 0 {
            format!(" ({skipped} skipped)")
        } else {
            String::new()
        }
    );
    for c in &conversations {
        let when = c
            .started_at
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "????-??-??".into());
        println!(
            "  [{}] {}  {}  ({} msgs)  {}",
            when,
            c.tool.slug(),
            c.project.as_deref().unwrap_or("-"),
            c.messages.len(),
            c.derived_title(),
        );
    }

    Ok(ExitCode::SUCCESS)
}
