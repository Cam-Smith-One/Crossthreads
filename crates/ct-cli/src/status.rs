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
            } => {
                println!("daemon: running");
                println!("  conversations: {conversations}");
                println!("  embeddings:    {embeddings} ({embedder})");
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
    Ok(ExitCode::SUCCESS)
}
