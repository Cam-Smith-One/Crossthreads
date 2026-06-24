//! `crossthreads graph` — a knowledge graph of your work. Projects, tools, and
//! tags as nodes (sized by session count) with co-occurrence edges. Same data as
//! the daemon `graph` op and the `crossthreads_graph` MCP tool. Offline.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut limit: usize = 40;
    let mut db: Option<PathBuf> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--limit" => {
                limit = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--limit needs a value"))?
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--limit must be a number"))?
            }
            "--db" => {
                db = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?,
                ))
            }
            other => bail!("unknown option for `graph`: {other}"),
        }
    }

    let store = Store::open(crate::resolve_db(db)?)?;
    let graph = store.graph(limit)?;
    if graph.nodes.is_empty() {
        println!("No graph yet — run `crossthreads index` first.");
        return Ok(ExitCode::SUCCESS);
    }

    println!(
        "Knowledge graph — {} nodes, {} edges.\n",
        graph.nodes.len(),
        graph.edges.len()
    );

    for kind in ["project", "tool", "tag"] {
        let mut nodes: Vec<_> = graph.nodes.iter().filter(|n| n.kind == kind).collect();
        if nodes.is_empty() {
            continue;
        }
        nodes.sort_by_key(|n| std::cmp::Reverse(n.weight));
        println!("{}s:", title_case(kind));
        for n in nodes.iter().take(12) {
            println!("  {:<28} {:>4}", n.label, n.weight);
        }
        println!();
    }

    println!("Strongest links:");
    for e in graph.edges.iter().take(15) {
        // Strip the `kind:` prefix for display.
        let s = e.source.split_once(':').map_or(&*e.source, |x| x.1);
        let t = e.target.split_once(':').map_or(&*e.target, |x| x.1);
        println!("  {s} ↔ {t}  ({})", e.weight);
    }
    Ok(ExitCode::SUCCESS)
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
