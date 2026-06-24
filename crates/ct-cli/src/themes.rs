//! `crossthreads themes` — cluster the indexed conversations and surface the
//! themes you spend time on. The clustering lives in `ct-store::themes` (shared
//! with the daemon, web UI, and MCP); this renders it and, with `--name`, labels
//! each cluster via your local Claude/Codex login (offline keyword label
//! otherwise).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_store::{Store, Theme};

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut db: Option<PathBuf> = None;
    let mut k: usize = 8;
    let mut samples: usize = 3;
    let mut name = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--name" => name = true,
            "--db" => {
                db = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?,
                ))
            }
            "--k" => {
                k = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--k needs a value"))?
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--k must be a number"))?
            }
            "--samples" => {
                samples = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--samples needs a value"))?
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--samples must be a number"))?
            }
            other => bail!("unknown option for `themes`: {other}"),
        }
    }
    if k == 0 {
        bail!("--k must be at least 1");
    }

    let db_path = crate::resolve_db(db)?;
    let store = Store::open(&db_path)?;
    // Keep enough samples per theme for both display and LLM naming.
    let themes = store.themes(k, samples.max(8))?;
    if themes.is_empty() {
        println!(
            "No embeddings yet — run `crossthreads index` first (semantic themes need vectors)."
        );
        return Ok(ExitCode::SUCCESS);
    }

    if name && !ct_llm::available() {
        eprintln!(
            "note: --name needs model auth — set ANTHROPIC_API_KEY or OPENAI_API_KEY, sign into \
             Claude Code or Codex, or install a CLI (`crossthreads llm-auth`). Using keyword \
             labels.\n"
        );
    }

    let total: usize = themes.iter().map(|t| t.size).sum();
    println!(
        "Themes across {total} conversations ({} clusters):\n",
        themes.len()
    );
    for theme in &themes {
        let label = if name {
            llm_name(theme).unwrap_or_else(|| theme.label.clone())
        } else {
            theme.label.clone()
        };
        let tools = theme
            .tools
            .iter()
            .take(3)
            .map(|(t, n)| format!("{t} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("▸ {label}  ({} conversations · {tools})", theme.size);
        for s in theme.samples.iter().take(samples) {
            println!(
                "    · {}",
                s.title.as_deref().unwrap_or("(untitled)").trim()
            );
        }
        println!();
    }
    Ok(ExitCode::SUCCESS)
}

/// Ask the local model login for a concise theme name; `None` on any error so
/// the caller falls back to the keyword label.
fn llm_name(theme: &Theme) -> Option<String> {
    let titles: Vec<&str> = theme
        .samples
        .iter()
        .take(8)
        .map(|s| s.title.as_deref().unwrap_or("").trim())
        .filter(|t| !t.is_empty())
        .collect();
    let user = format!(
        "These AI coding-session titles form one cluster (keywords: {}).\n\
         Give a 2-4 word theme name. Reply with only the name.\n\n- {}",
        theme.label,
        titles.join("\n- ")
    );
    let system =
        "You label clusters of software-engineering chat sessions with a short, specific theme.";
    let raw = ct_llm::complete(system, &user).ok()?;
    let name = raw
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .trim();
    if name.is_empty() || name.len() > 60 {
        None
    } else {
        Some(name.to_string())
    }
}
