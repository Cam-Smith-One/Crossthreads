//! `crossthreads ask <QUESTION>` — answer a question from your own history.
//! Retrieves the most relevant past sessions (hybrid search) and, when a model
//! login is configured, synthesizes a cited answer; otherwise prints the
//! retrieved context for you (or an agent) to read. Same engine as the daemon's
//! `ask` op and the `crossthreads_ask` MCP tool.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut limit: usize = 6;
    let mut db: Option<PathBuf> = None;
    let mut terms: Vec<String> = Vec::new();

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
            other if other.starts_with("--") => bail!("unknown option for `ask`: {other}"),
            term => terms.push(term.to_string()),
        }
    }
    if terms.is_empty() {
        bail!("usage: crossthreads ask <QUESTION> [--limit N]");
    }
    let question = terms.join(" ");

    let store = Store::open(crate::resolve_db(db)?)?;
    let embedding = ct_embed::default_embedder().embed_one(&question)?;
    let hits = store.search_hybrid(&question, &embedding, limit)?;
    if hits.is_empty() {
        println!("No past sessions found relevant to \u{201c}{question}\u{201d}.");
        return Ok(ExitCode::SUCCESS);
    }
    let ids: Vec<String> = hits.iter().map(|h| h.conversation_id.clone()).collect();
    let block = store.render_context(&ids, 9000)?;
    drop(store);

    if !ct_llm::available() {
        // No model — show the retrieved context so the user can read it.
        eprintln!(
            "note: no model login — showing retrieved context (add one with `crossthreads \
             llm-auth` for a synthesized answer).\n"
        );
        print!("{}", block.markdown);
        return Ok(ExitCode::SUCCESS);
    }

    let system = "You answer a developer's question using ONLY the provided excerpts from their \
                  own past AI coding sessions. Cite the session title in parentheses after each \
                  claim. If the excerpts don't contain the answer, say so plainly rather than \
                  guessing. Be concise; use Markdown.";
    let user = format!(
        "Question: {question}\n\n--- RELEVANT PAST SESSIONS ---\n{}",
        block.markdown
    );
    let answer = ct_llm::complete_with(system, &user, 1024)?;
    println!("{answer}");
    eprintln!("\n(answered from {} past session(s))", block.sources.len());
    Ok(ExitCode::SUCCESS)
}
