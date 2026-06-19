//! `crossthreads index` — discover, parse, persist, and embed sessions.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut dry_run = false;
    let mut limit: Option<usize> = None;
    let mut db: Option<PathBuf> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--limit" => {
                let v = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--limit needs a value"))?;
                limit = Some(v.parse()?);
            }
            "--db" => {
                let v = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?;
                db = Some(PathBuf::from(v));
            }
            other => bail!("unknown option for `index`: {other}"),
        }
    }

    if dry_run {
        let (conversations, unparseable) = ct_index::collect(limit);
        if conversations.is_empty() {
            println!("No sessions found on this machine.");
            return Ok(ExitCode::SUCCESS);
        }
        println!(
            "Parsed {} conversation(s) (dry-run, not stored):\n",
            conversations.len()
        );
        for c in &conversations {
            print_line(c);
        }
        if unparseable > 0 {
            println!("\n{unparseable} session(s) failed to parse — see warnings above.");
        }
        return Ok(ExitCode::SUCCESS);
    }

    let db_path = crate::resolve_db(db)?;
    let mut store = Store::open(&db_path)?;
    let embedder = ct_embed::default_embedder();

    let report = ct_index::index_once(&mut store, &*embedder, limit)?;

    if report.parsed == 0 {
        println!("No sessions found on this machine.");
        return Ok(ExitCode::SUCCESS);
    }

    println!(
        "Indexed into {}:\n  {} new, {} already present{}.",
        db_path.display(),
        report.inserted,
        report.duplicate,
        if report.unparseable > 0 {
            format!(", {} unparseable", report.unparseable)
        } else {
            String::new()
        }
    );
    if report.embedded > 0 {
        println!(
            "Embedded {} message(s) with {}.",
            report.embedded,
            embedder.id()
        );
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
