//! `crossthreads search <QUERY>` — lexical / semantic / hybrid search.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::Mode;
use ct_store::{Filters, SearchHit, Store};

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut as_json = false;
    let mut limit: usize = 10;
    let mut db: Option<PathBuf> = None;
    let mut mode = Mode::Hybrid;
    let mut remote = false;
    let mut addr: Option<String> = None;
    let mut filters = Filters::default();
    let mut terms: Vec<String> = Vec::new();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => as_json = true,
            "--remote" => remote = true,
            "--limit" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--limit needs a value"))?;
                limit = v.parse()?;
            }
            "--db" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--db needs a value"))?;
                db = Some(PathBuf::from(v));
            }
            "--addr" => {
                addr = Some(it.next().ok_or_else(|| anyhow::anyhow!("--addr needs a value"))?.clone());
                remote = true;
            }
            "--tool" => filters.tool = Some(it.next().ok_or_else(|| anyhow::anyhow!("--tool needs a value"))?.clone()),
            "--project" => filters.project = Some(it.next().ok_or_else(|| anyhow::anyhow!("--project needs a value"))?.clone()),
            "--since" => filters.since = Some(it.next().ok_or_else(|| anyhow::anyhow!("--since needs a value"))?.clone()),
            "--until" => filters.until = Some(it.next().ok_or_else(|| anyhow::anyhow!("--until needs a value"))?.clone()),
            "--mode" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--mode needs a value"))?;
                mode = match v.as_str() {
                    "lexical" => Mode::Lexical,
                    "semantic" => Mode::Semantic,
                    "hybrid" => Mode::Hybrid,
                    other => bail!("unknown --mode: {other} (lexical|semantic|hybrid)"),
                };
            }
            other if other.starts_with("--") => bail!("unknown option for `search`: {other}"),
            term => terms.push(term.to_string()),
        }
    }

    if terms.is_empty() {
        bail!("usage: crossthreads search <QUERY> [--mode …] [--tool …] [--project …] [--since …] [--until …] [--remote]");
    }
    let query = terms.join(" ");

    // --remote: route through a running daemon (it owns the embedder + store).
    let hits = if remote {
        let client = match addr {
            Some(a) => ct_daemon::Client::new(a),
            None => ct_daemon::Client::from_env(),
        };
        client.search(&query, mode, limit, filters)?
    } else {
        let db_path = crate::resolve_db(db)?;
        let store = Store::open(&db_path)?;
        match mode {
            Mode::Lexical => store.search_filtered(&query, limit, &filters)?,
            Mode::Semantic => {
                let q = embed_query(&query)?;
                store.search_semantic_filtered(&q, limit, &filters)?
            }
            Mode::Hybrid => {
                let q = embed_query(&query)?;
                store.search_hybrid_filtered(&query, &q, limit, &filters)?
            }
        }
    };

    if as_json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
        return Ok(ExitCode::SUCCESS);
    }
    print_hits(&query, mode_label(mode), &hits);
    Ok(ExitCode::SUCCESS)
}

fn embed_query(query: &str) -> Result<Vec<f32>> {
    let embedder = ct_embed::default_embedder();
    embedder.embed_one(query)
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Lexical => "lexical",
        Mode::Semantic => "semantic",
        Mode::Hybrid => "hybrid",
    }
}

fn print_hits(query: &str, mode: &str, hits: &[SearchHit]) {
    if hits.is_empty() {
        println!("No matches for \"{query}\" ({mode}).");
        return;
    }
    println!("{} result(s) for \"{query}\" ({mode}):\n", hits.len());
    for h in hits {
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
}
