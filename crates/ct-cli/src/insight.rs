//! `crossthreads insight <kind>` — synthesize a high-level view over your recent
//! sessions using your local model login (Settings → Models / `llm-auth`):
//! open loops, knowledge cards, a decision log, a "how I work" profile, or a
//! digest. The synthesis core is shared with the daemon and MCP server
//! (`ct_daemon::insights`); this renders it for the terminal.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::insights::{self, Kind};
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut db: Option<PathBuf> = None;
    let mut kind_arg: Option<String> = None;
    let mut limit: Option<usize> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--db" => {
                db = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?,
                ))
            }
            "--limit" => {
                limit = Some(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--limit needs a value"))?
                        .parse()
                        .map_err(|_| anyhow::anyhow!("--limit must be a number"))?,
                )
            }
            other if other.starts_with("--") => bail!("unknown option for `insight`: {other}"),
            other if kind_arg.is_none() => kind_arg = Some(other.to_string()),
            other => bail!("unexpected argument: {other}"),
        }
    }

    let Some(kind_arg) = kind_arg else {
        eprintln!(
            "usage: crossthreads insight <KIND> [--limit N]\n\n\
             KIND is one of:\n  \
             open_loops      Unresolved work, dangling TODOs, unconfirmed fixes\n  \
             knowledge_cards Durable Q→A cards worth remembering\n  \
             decision_log    Notable decisions and their rationale\n  \
             how_i_work      A profile of your conventions for a CLAUDE.md/AGENTS.md\n  \
             digest          A short reflective digest of recent work"
        );
        return Ok(ExitCode::FAILURE);
    };

    let Some(kind) = Kind::parse(&kind_arg) else {
        bail!(
            "unknown insight kind `{kind_arg}` — expected one of: open_loops, knowledge_cards, \
             decision_log, how_i_work, digest"
        );
    };

    if !ct_llm::available() {
        bail!(
            "insights need a model login — set ANTHROPIC_API_KEY / OPENAI_API_KEY / \
             GEMINI_API_KEY, sign into Claude Code / Codex / Gemini, or run `crossthreads llm-auth`"
        );
    }

    let db_path = crate::resolve_db(db)?;
    let store = Store::open(&db_path)?;
    let n = limit.unwrap_or_else(|| kind.default_limit());
    let convos = store.recent_conversations(n, 1200)?;
    drop(store);

    let (markdown, sources) = insights::synthesize(&convos, kind)?;
    println!("{markdown}");
    if !sources.is_empty() {
        eprintln!("\n(drawn from {} recent session(s))", sources.len());
    }
    Ok(ExitCode::SUCCESS)
}
