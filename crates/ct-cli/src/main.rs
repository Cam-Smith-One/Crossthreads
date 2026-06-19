//! `crossthreads` — CLI surface.
//!
//! Phase 0 spike: a single `index` command that runs the built-in connectors,
//! parses whatever is on the machine, and reports the normalized result. This
//! proves the `detect → discover → parse → normalize` pipeline end-to-end
//! before the daemon and storage layers exist.

use std::process::ExitCode;

use anyhow::Result;
use ct_core::model::Conversation;

mod index;

const USAGE: &str = "\
crossthreads — search your AI coding sessions across tools

USAGE:
    crossthreads <COMMAND> [OPTIONS]

COMMANDS:
    index     Discover and parse local sessions (Phase 0: dry-run, no storage)
    help      Show this help

`index` OPTIONS:
    --json        Emit parsed conversations as JSON instead of a summary
    --limit <N>   Parse at most N sessions
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

/// Shared helper: detect-and-parse across the built-in connectors, isolating
/// per-session failures so one bad file never aborts the run (FR-ING-07).
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
