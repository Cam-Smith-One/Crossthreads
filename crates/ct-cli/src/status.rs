//! `crossthreads status` — index health, locally or from a running daemon.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::{Client, Request, Response};
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut remote = false;
    let mut addr: Option<String> = None;
    let mut db: Option<PathBuf> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--remote" => remote = true,
            "--addr" => {
                addr = Some(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--addr needs a value"))?
                        .clone(),
                );
                remote = true;
            }
            "--db" => {
                db = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?,
                ))
            }
            other => bail!("unknown option for `status`: {other}"),
        }
    }

    if remote {
        let client = match addr {
            Some(a) => Client::new(a),
            None => Client::from_env(),
        };
        match client.call(&Request::Status)? {
            Response::Status {
                conversations,
                embeddings,
                embedder,
                tools,
            } => {
                println!("daemon: running");
                println!("  conversations: {conversations}");
                println!("  embeddings:    {embeddings} ({embedder})");
                for (tool, threads, skills) in tools {
                    let skills = if skills > 0 {
                        format!(" (+{skills} skills)")
                    } else {
                        String::new()
                    };
                    println!("  {tool:<14} {threads}{skills}");
                }
            }
            Response::Error { message } => bail!("daemon error: {message}"),
            other => bail!("unexpected response: {other:?}"),
        }
        return Ok(ExitCode::SUCCESS);
    }

    let db_path = crate::resolve_db(db)?;
    let store = Store::open(&db_path)?;
    println!("index: {}", db_path.display());
    println!("  conversations: {}", store.conversation_count()?);
    println!("  embeddings:    {}", store.embedding_count()?);
    for (tool, kind, n) in store.counts_by_tool()? {
        println!("  {tool:<14} {n} {kind}s");
    }
    Ok(ExitCode::SUCCESS)
}
