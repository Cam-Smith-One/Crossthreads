//! `crossthreads threads` and `crossthreads thread …` — pieces of work, each
//! one or more sessions linked across tools (SPEC §6.1). Same data as the
//! daemon `threads` / `thread` ops and the `crossthreads_threads` MCP tool.
//! Offline; no model needed.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::{Client, Request, Response};
use ct_store::{Live, Store, ThreadDetail, ThreadFilters, ThreadSummary};

/// A short marker for a live state, or nothing when there is none worth showing.
fn live_tag(live: &Option<Live>) -> String {
    let Some(l) = live else {
        return String::new();
    };
    let word = match l.state.as_str() {
        "needs_you" => "NEEDS YOU",
        "running" => "running",
        _ => return String::new(),
    };
    format!(" ● {word}{}", if l.inferred { " (inferred)" } else { "" })
}

fn date(ts: &Option<String>) -> &str {
    ts.as_deref().map(|t| &t[..t.len().min(10)]).unwrap_or("-")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn take_db(it: &mut std::slice::Iter<'_, String>) -> Result<PathBuf> {
    Ok(PathBuf::from(
        it.next()
            .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?,
    ))
}

/// `crossthreads threads` — list threads, most recently active first.
pub fn run_list(args: &[String]) -> Result<ExitCode> {
    let mut db: Option<PathBuf> = None;
    let mut json = false;
    let mut remote = false;
    let mut filters = ThreadFilters {
        limit: Some(30),
        ..Default::default()
    };

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--status" => filters.status = it.next().cloned(),
            "--tool" => filters.tool = it.next().cloned(),
            "--repo" => filters.repo = it.next().cloned(),
            "--limit" => {
                filters.limit = Some(
                    it.next()
                        .and_then(|n| n.parse().ok())
                        .ok_or_else(|| anyhow::anyhow!("--limit needs a number"))?,
                )
            }
            "--all" => filters.limit = None,
            "--remote" => remote = true,
            "--db" => db = Some(take_db(&mut it)?),
            other => bail!("unknown option for `threads`: {other}"),
        }
    }

    // `--remote` asks the running daemon, which knows what is live right now
    // and, with federation set up, what is on your other devices.
    let threads = if remote {
        match Client::from_env().call(&Request::Threads {
            filters,
            devices: None,
        })? {
            Response::Threads {
                threads,
                unreachable,
            } => {
                if !unreachable.is_empty() {
                    eprintln!("unreachable: {}", unreachable.join(", "));
                }
                threads
            }
            Response::Error { message } => bail!(message),
            other => bail!("unexpected response: {other:?}"),
        }
    } else {
        Store::open(crate::resolve_db(db)?)?.threads(&filters)?
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&threads)?);
        return Ok(ExitCode::SUCCESS);
    }
    if threads.is_empty() {
        println!("No threads yet — run `crossthreads index` first.");
        return Ok(ExitCode::SUCCESS);
    }
    print_list(&threads);
    Ok(ExitCode::SUCCESS)
}

fn print_list(threads: &[ThreadSummary]) {
    // Group by repository, keeping the most recently active repository first.
    let mut repos: Vec<&str> = Vec::new();
    for t in threads {
        let label = t.repo_label.as_deref().unwrap_or("(no repository)");
        if !repos.contains(&label) {
            repos.push(label);
        }
    }
    for repo in repos {
        println!("{repo}");
        for t in threads
            .iter()
            .filter(|t| t.repo_label.as_deref().unwrap_or("(no repository)") == repo)
        {
            let status = match t.status.as_str() {
                "active" => String::new(),
                other => format!(" [{other}]"),
            };
            let sessions = if t.session_count > 1 {
                format!(" · {} sessions", t.session_count)
            } else {
                String::new()
            };
            let pending = if t.pending_suggestions > 0 {
                format!(" · {} suggested", t.pending_suggestions)
            } else {
                String::new()
            };
            let device = t
                .device
                .as_deref()
                .map(|d| format!(" · on {d}"))
                .unwrap_or_default();
            println!(
                "  {}  {}  {}{status}{}\n      {}{sessions}{pending}{device}  {}",
                date(&t.last_activity),
                t.id,
                truncate(&t.title, 70),
                live_tag(&t.live),
                t.tools.join(" + "),
                t.branches.join(", "),
            );
        }
        println!();
    }
}

/// `crossthreads thread <ID>` and its subcommands.
pub fn run_thread(args: &[String]) -> Result<ExitCode> {
    const USAGE: &str = "usage:
    crossthreads thread <ID> [--json]            show a thread (thread or session id)
    crossthreads thread <ID> --handoff           a paste-ready brief to continue it
                                                 in another tool or agent
    crossthreads thread link <A> <B> [--kind K]  put two sessions in one thread
                                                 (also accepts a suggestion)
    crossthreads thread unlink <SESSION>         take a session out of its thread
    crossthreads thread reject <A> <B>           dismiss a suggested link for good
    crossthreads thread rename <ID> <TITLE>      rename (empty title = derived)
    crossthreads thread status <ID> <active|parked|done>";

    let mut db: Option<PathBuf> = None;
    let mut json = false;
    let mut handoff = false;
    let mut kind = "manual".to_string();
    let mut words: Vec<String> = Vec::new();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--handoff" => handoff = true,
            "--kind" => {
                kind = it
                    .next()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("--kind needs a value"))?
            }
            "--db" => db = Some(take_db(&mut it)?),
            other if other.starts_with("--") => bail!("unknown option for `thread`: {other}"),
            other => words.push(other.to_string()),
        }
    }

    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let store = Store::open(crate::resolve_db(db)?)?;
    match words.as_slice() {
        ["link", a, b] => match store.link_sessions(a, b, &kind, "manual")? {
            Some(id) => println!("linked into {id}"),
            None => return not_found("one of those sessions (or both are the same session)"),
        },
        ["unlink", session] => {
            if !store.unlink_session(session)? {
                return not_found("a linked session with that id");
            }
            println!("unlinked {session}");
        }
        ["reject", a, b] => {
            if !store.reject_link(a, b)? {
                return not_found("one of those sessions");
            }
            println!("rejected; it won't be suggested again");
        }
        ["rename", id, title @ ..] => match store.set_thread(id, Some(&title.join(" ")), None)? {
            Some(id) => println!("renamed {id}"),
            None => return not_found("a thread with that id"),
        },
        ["status", id, status] => match store.set_thread(id, None, Some(status))? {
            Some(id) => println!("{id} is now {status}"),
            None => return not_found("a thread with that id"),
        },
        [id] if handoff => match store.thread_handoff(id, 8000)? {
            Some(block) => print!("{}", block.markdown),
            None => return not_found("a thread or session with that id"),
        },
        [id] if !matches!(*id, "link" | "unlink" | "reject" | "rename" | "status") => {
            let Some(thread) = store.thread(id)? else {
                return not_found("a thread or session with that id");
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&thread)?);
            } else {
                print_thread(&thread);
            }
        }
        _ => bail!("{USAGE}"),
    }
    Ok(ExitCode::SUCCESS)
}

fn not_found(what: &str) -> Result<ExitCode> {
    eprintln!("couldn't find {what}");
    Ok(ExitCode::FAILURE)
}

fn print_thread(t: &ThreadDetail) {
    let s = &t.summary;
    println!("{}  [{}]", s.title, s.status);
    println!(
        "{} · {} · {} session(s), {} messages",
        s.id,
        s.repo_label.as_deref().unwrap_or("(no repository)"),
        s.session_count,
        s.message_count
    );
    println!("\nSessions (oldest first):");
    for sess in &t.sessions {
        println!(
            "  {}  {:<12} {}  {}",
            date(&sess.started_at),
            sess.tool,
            sess.conversation_id,
            truncate(sess.title.as_deref().unwrap_or("(untitled)"), 60),
        );
    }
    if !t.links.is_empty() {
        println!("\nLinks:");
        for l in &t.links {
            println!("  {} → {}  {} ({})", l.from, l.to, l.kind, l.origin);
        }
    }
    if !t.suggestions.is_empty() {
        println!("\nSuggested (not applied):");
        for sug in &t.suggestions {
            println!(
                "  {} {} — {}\n      accept: crossthreads thread link {} {}\n      reject: crossthreads thread reject {} {}",
                sug.session.tool,
                sug.session.conversation_id,
                truncate(sug.session.title.as_deref().unwrap_or("(untitled)"), 50),
                sug.with,
                sug.session.conversation_id,
                sug.with,
                sug.session.conversation_id,
            );
        }
    }
    println!("\nOpen a session: crossthreads resume <SESSION ID>");
}
