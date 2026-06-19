//! `crossthreads search <QUERY>` — FTS5 keyword search over the index.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut as_json = false;
    let mut limit: usize = 10;
    let mut db: Option<PathBuf> = None;
    let mut terms: Vec<String> = Vec::new();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => as_json = true,
            "--limit" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--limit needs a value"))?;
                limit = v.parse()?;
            }
            "--db" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--db needs a value"))?;
                db = Some(PathBuf::from(v));
            }
            other if other.starts_with("--") => bail!("unknown option for `search`: {other}"),
            term => terms.push(term.to_string()),
        }
    }

    if terms.is_empty() {
        bail!("usage: crossthreads search <QUERY>");
    }
    let query = terms.join(" ");

    let db_path = crate::resolve_db(db)?;
    let store = Store::open(&db_path)?;
    let hits = store.search(&query, limit)?;

    if as_json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
        return Ok(ExitCode::SUCCESS);
    }

    if hits.is_empty() {
        println!("No matches for \"{query}\".");
        return Ok(ExitCode::SUCCESS);
    }

    println!("{} result(s) for \"{query}\":\n", hits.len());
    for h in &hits {
        let when = h.started_at.as_deref().unwrap_or("").get(..10).unwrap_or("");
        println!(
            "  {} · {}{}",
            h.tool,
            h.title.as_deref().unwrap_or("(untitled)"),
            if when.is_empty() {
                String::new()
            } else {
                format!("  [{when}]")
            },
        );
        if let Some(project) = &h.project {
            println!("    {project}");
        }
        println!("    {}", h.snippet.replace('\n', " "));
        println!();
    }
    Ok(ExitCode::SUCCESS)
}
