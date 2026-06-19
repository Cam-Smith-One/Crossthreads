//! `crossthreads` — CLI surface.
//!
//! Phase 0: `index` runs the built-in connectors and persists normalized
//! conversations into the single SQLite index (dedup by content hash);
//! `search` runs FTS5 keyword search over it. The daemon and semantic search
//! layer onto this same store next.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use ct_core::model::Conversation;

mod index;
mod search;

const USAGE: &str = "\
crossthreads — search your AI coding sessions across tools

USAGE:
    crossthreads <COMMAND> [OPTIONS]

COMMANDS:
    index             Discover, parse, and store local sessions
    search <QUERY>    Keyword-search the index
    help              Show this help

COMMON OPTIONS:
    --db <PATH>       Index database path (default: platform data dir;
                      override with CROSSTHREADS_DB)

`index` OPTIONS:
    --limit <N>       Parse at most N sessions
    --dry-run         Parse only; do not write to the index

`search` OPTIONS:
    --mode <M>        lexical | semantic | hybrid (default: hybrid)
    --limit <N>       Max results (default: 10)
    --json            Emit results as JSON

Build with `--features onnx` for real semantic embeddings (all-MiniLM);
otherwise a deterministic offline embedder is used.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode> {
    match args.first().map(String::as_str) {
        Some("index") => index::run(&args[1..]),
        Some("search") => search::run(&args[1..]),
        Some("help") | Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Some(other) => {
            eprintln!("unknown command: {other}\n");
            print!("{USAGE}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Embed all messages that don't yet have a vector, in batches. Returns the
/// number embedded. Shared by `index` (and, later, the daemon).
pub(crate) fn embed_pending(store: &mut ct_store::Store, embedder: &dyn ct_embed::Embedder) -> Result<usize> {
    let mut total = 0;
    loop {
        let batch = store.pending_embeddings(256)?;
        if batch.is_empty() {
            break;
        }
        let texts: Vec<String> = batch.iter().map(|(_, c)| c.clone()).collect();
        let vecs = embedder.embed(&texts)?;
        let rows: Vec<(i64, Vec<f32>)> = batch
            .iter()
            .map(|(rowid, _)| *rowid)
            .zip(vecs)
            .collect();
        store.store_embeddings(embedder.id(), &rows)?;
        total += rows.len();
    }
    Ok(total)
}

/// Resolve the index DB path: explicit `--db`, else `CROSSTHREADS_DB`, else the
/// platform data dir. Ensures the parent directory exists.
pub(crate) fn resolve_db(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let path = explicit
        .or_else(|| std::env::var_os("CROSSTHREADS_DB").map(PathBuf::from))
        .unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("crossthreads")
                .join("index.db")
        });
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating index dir {}", parent.display()))?;
    }
    Ok(path)
}

/// Detect-and-parse across the built-in connectors, isolating per-session
/// failures so one bad file never aborts the run (FR-ING-07).
pub(crate) fn collect(limit: Option<usize>) -> (Vec<Conversation>, usize) {
    let mut conversations = Vec::new();
    let mut skipped = 0usize;

    for connector in ct_connectors::builtin() {
        if !connector.detect() {
            continue;
        }
        let sessions = match connector.discover() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("warn: discovery failed for {}: {e}", connector.tool().slug());
                continue;
            }
        };
        for session in sessions {
            if let Some(limit) = limit {
                if conversations.len() >= limit {
                    return (conversations, skipped);
                }
            }
            match connector.parse(&session) {
                Ok(convo) => conversations.push(convo),
                Err(e) => {
                    skipped += 1;
                    eprintln!("warn: skipped {}: {e}", session.path.display());
                }
            }
        }
    }

    (conversations, skipped)
}
