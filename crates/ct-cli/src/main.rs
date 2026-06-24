//! `crossthreads` — CLI surface.
//!
//! Phase 0: `index` runs the built-in connectors and persists normalized
//! conversations into the single SQLite index (dedup by content hash);
//! `search` runs FTS5 keyword search over it. The daemon and semantic search
//! layer onto this same store next.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};

mod activity;
mod ask;
mod context;
mod graph;
mod index;
mod insight;
mod llm_auth;
mod metrics;
mod search;
mod skill;
mod status;
mod themes;

const USAGE: &str = "\
crossthreads — search your AI coding sessions across tools

USAGE:
    crossthreads <COMMAND> [OPTIONS]

COMMANDS:
    index             Discover, parse, and store local sessions
    search <QUERY>    Search the index (lexical / semantic / hybrid)
    context <QUERY>   Build a paste-ready context block from top matches
    status            Show index health (local, or --remote for the daemon)
    skill install     Install the Crossthreads agent skill for Claude Code/Codex
    themes            Cluster your sessions into themes (--k N; --name to label
                      them with your local Claude/Codex login)
    insight <KIND>    Synthesize over your history with your model login:
                      open_loops | knowledge_cards | decision_log | how_i_work |
                      digest | recurrence | work_dna | gaps | prompt_coach |
                      process_miner (--limit N)
    ask <QUESTION>    Answer a question from your history (cited; uses your model)
    activity          Session activity over time (--by day|week)
    graph             Knowledge graph of projects, tools, and tags
    metrics           Behavioral metrics: how you work (turns, friction, tempo)
    llm-auth          Show which model credentials are available, per provider
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
    --tool <SLUG>     Filter to one tool (claude-code, cursor, aider, codex)
    --kind <KIND>     Filter by record kind (thread | skill)
    --project <STR>   Filter to projects whose path contains STR
    --since <DATE>    Only sessions on/after DATE (YYYY-MM-DD)
    --until <DATE>    Only sessions on/before DATE (YYYY-MM-DD)
    --remote          Query a running daemon (crossthreadsd) instead of the
                      local DB; --addr <ADDR> sets the address
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
        Some("context") => context::run(&args[1..]),
        Some("status") => status::run(&args[1..]),
        Some("skill") => skill::run(&args[1..]),
        Some("themes") => themes::run(&args[1..]),
        Some("insight") => insight::run(&args[1..]),
        Some("ask") => ask::run(&args[1..]),
        Some("activity") => activity::run(&args[1..]),
        Some("graph") => graph::run(&args[1..]),
        Some("metrics") => metrics::run(&args[1..]),
        Some("llm-auth") => llm_auth::run(&args[1..]),
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
