//! `crossthreads context <QUERY>` — build a paste-ready context block from the
//! sessions most relevant to a query (FR-ACT-03 / AGENT_API §5).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::Mode;
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut limit: usize = 3;
    let mut max_chars: usize = 6000;
    let mut db: Option<PathBuf> = None;
    let mut mode = Mode::Hybrid;
    let mut remote = false;
    let mut addr: Option<String> = None;
    let mut terms: Vec<String> = Vec::new();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--remote" => remote = true,
            "--limit" => limit = it.next().ok_or_else(|| anyhow::anyhow!("--limit needs a value"))?.parse()?,
            "--max-chars" => max_chars = it.next().ok_or_else(|| anyhow::anyhow!("--max-chars needs a value"))?.parse()?,
            "--db" => db = Some(PathBuf::from(it.next().ok_or_else(|| anyhow::anyhow!("--db needs a value"))?)),
            "--addr" => {
                addr = Some(it.next().ok_or_else(|| anyhow::anyhow!("--addr needs a value"))?.clone());
                remote = true;
            }
            "--mode" => {
                mode = match it.next().map(String::as_str) {
                    Some("lexical") => Mode::Lexical,
                    Some("semantic") => Mode::Semantic,
                    Some("hybrid") => Mode::Hybrid,
                    other => bail!("unknown --mode: {other:?}"),
                };
            }
            other if other.starts_with("--") => bail!("unknown option for `context`: {other}"),
            term => terms.push(term.to_string()),
        }
    }
    if terms.is_empty() {
        bail!("usage: crossthreads context <QUERY> [--limit N] [--max-chars N] [--remote]");
    }
    let query = terms.join(" ");

    let markdown = if remote {
        let client = match addr {
            Some(a) => ct_daemon::Client::new(a),
            None => ct_daemon::Client::from_env(),
        };
        client.build_context(&query, mode, limit, max_chars, Default::default())?.0
    } else {
        let store = Store::open(crate::resolve_db(db)?)?;
        let hits = match mode {
            Mode::Lexical => store.search(&query, limit)?,
            Mode::Semantic => store.search_semantic(&embed(&query)?, limit)?,
            Mode::Hybrid => store.search_hybrid(&query, &embed(&query)?, limit)?,
        };
        let ids: Vec<String> = hits.into_iter().map(|h| h.conversation_id).collect();
        store.render_context(&ids, max_chars)?.markdown
    };

    print!("{markdown}");
    Ok(ExitCode::SUCCESS)
}

fn embed(query: &str) -> Result<Vec<f32>> {
    ct_embed::default_embedder().embed_one(query)
}
