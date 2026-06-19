//! `crossthreads index` — discover, parse, and persist sessions.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_store::{Store, Upsert};

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut dry_run = false;
    let mut limit: Option<usize> = None;
    let mut db: Option<PathBuf> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--limit" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--limit needs a value"))?;
                limit = Some(v.parse()?);
            }
            "--db" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--db needs a value"))?;
                db = Some(PathBuf::from(v));
            }
            other => bail!("unknown option for `index`: {other}"),
        }
    }

    let (conversations, parse_skipped) = crate::collect(limit);

    if conversations.is_empty() {
        println!("No sessions found on this machine.");
        return Ok(ExitCode::SUCCESS);
    }

    if dry_run {
        println!("Parsed {} conversation(s) (dry-run, not stored):\n", conversations.len());
        for c in &conversations {
            print_line(c);
        }
        if parse_skipped > 0 {
            println!("\n{parse_skipped} session(s) failed to parse — see warnings above.");
        }
        return Ok(ExitCode::SUCCESS);
    }

    let db_path = crate::resolve_db(db)?;
    let mut store = Store::open(&db_path)?;

    let (mut inserted, mut duplicate) = (0usize, 0usize);
    for c in &conversations {
        match store.upsert_conversation(c)? {
            Upsert::Inserted => inserted += 1,
            Upsert::Duplicate => duplicate += 1,
        }
    }

    println!(
        "Indexed {} conversation(s) into {}:\n  {inserted} new, {duplicate} already present{}.",
        conversations.len(),
        db_path.display(),
        if parse_skipped > 0 {
            format!(", {parse_skipped} unparseable")
        } else {
            String::new()
        }
    );

    // Compute embeddings for new messages so semantic/hybrid search works.
    let embedder = ct_embed::default_embedder();
    let embedded = crate::embed_pending(&mut store, &*embedder)?;
    if embedded > 0 {
        println!("Embedded {embedded} message(s) with {}.", embedder.id());
    }

    println!("Total in index: {}", store.conversation_count()?);
    Ok(ExitCode::SUCCESS)
}

fn print_line(c: &ct_core::model::Conversation) {
    let when = c
        .started_at
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "????-??-??".into());
    println!(
        "  [{}] {}  {}  ({} msgs)  {}",
        when,
        c.tool.slug(),
        c.project.as_deref().unwrap_or("-"),
        c.messages.len(),
        c.derived_title(),
    );
}
