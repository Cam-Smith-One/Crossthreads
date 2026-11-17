//! `crossthreads devices …` — add your other computers with a short code
//! (SPEC §6.6). The same as Devices → Add a device in the app.

use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::{Client, Request, Response};

const USAGE: &str = "\
crossthreads devices — your other computers

USAGE:
    crossthreads devices               list them, online or not
    crossthreads devices add           show a code to type on the new computer
    crossthreads devices join <CODE> [ADDRESS]
                                       on the new computer: join the one showing CODE.
                                       ADDRESS (its Tailscale address) only if it isn't found.

Both computers need Crossthreads open and Tailscale signed in to the same
account. Paired computers search each other's history, share threads, and
their agents can message each other.
";

pub fn run(args: &[String]) -> Result<ExitCode> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let client = Client::from_env();
    match words.as_slice() {
        [] | ["list"] => match client.call(&Request::Devices)? {
            Response::Devices { devices, .. } => {
                for d in devices {
                    println!(
                        "{} {}{}{}",
                        if d.online { "●" } else { "○" },
                        d.name,
                        if d.local { " (this computer)" } else { "" },
                        d.addr.map(|a| format!("  {a}")).unwrap_or_default()
                    );
                }
            }
            Response::Error { message } => bail!(message),
            other => bail!("unexpected response: {other:?}"),
        },
        ["add"] => {
            match client.call(&Request::DevicePairStart)? {
                Response::DevicePairCode {
                    code,
                    expires_in,
                    device,
                    addr,
                    problem,
                } => {
                    if let Some(p) = problem {
                        println!("{p}\n");
                    }
                    println!("On the new computer, open Crossthreads → Devices → Add a device, and type:\n");
                    println!("    {code}\n");
                    println!(
                    "(or run `crossthreads devices join {code}`). It works for {} minutes, once.",
                    expires_in / 60
                );
                    if let Some(a) = addr {
                        let ip = a.rsplit_once(':').map(|(ip, _)| ip).unwrap_or(&a);
                        println!("If it can't find {device}, also give it this address: {ip}");
                    }
                }
                Response::Error { message } => bail!(message),
                other => bail!("unexpected response: {other:?}"),
            }
        }
        ["join", code, rest @ ..] if rest.len() <= 1 => {
            let req = Request::DevicePairJoin {
                code: code.to_string(),
                addr: rest.first().map(|a| a.to_string()),
            };
            match client.call_timeout(&req, std::time::Duration::from_secs(60))? {
                Response::DevicePaired { name, note } => {
                    println!(
                        "Added {name}. Your history, threads and rooms now span both computers."
                    );
                    if let Some(n) = note {
                        println!("{n}");
                    }
                }
                Response::Error { message } => bail!(message),
                other => bail!("unexpected response: {other:?}"),
            }
        }
        ["help" | "--help" | "-h"] => print!("{USAGE}"),
        _ => bail!("{USAGE}"),
    }
    Ok(ExitCode::SUCCESS)
}

const NOTE_USAGE: &str = "\
crossthreads note [--kind decision|todo|context] <TEXT>
    Keep a note on the thread for this folder; agents and your other devices see it.
crossthreads note done <ID>
    Mark a todo done.
";

/// `crossthreads note …`
pub fn note(args: &[String]) -> Result<ExitCode> {
    let client = Client::from_env();
    let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
    let req = match args {
        [d, id] if d == "done" && id.starts_with("n_") => Request::ThreadNoteEdit {
            id: id.clone(),
            kind: Some("done".into()),
            removed: false,
        },
        [flag, kind, rest @ ..] if flag == "--kind" && !rest.is_empty() => Request::ThreadNoteAdd {
            thread: None,
            device: None,
            cwd: Some(cwd),
            kind: kind.clone(),
            text: rest.join(" "),
            author: None,
        },
        [h] if h == "--help" || h == "-h" || h == "help" => {
            print!("{NOTE_USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        [] => bail!("{NOTE_USAGE}"),
        text => Request::ThreadNoteAdd {
            thread: None,
            device: None,
            cwd: Some(cwd),
            kind: "context".into(),
            text: text.join(" "),
            author: None,
        },
    };
    match client.call(&req)? {
        Response::ThreadNote { note } => {
            println!("Noted ({}) on {} [{}]", note.kind, note.thread_id, note.id)
        }
        Response::Error { message } => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
    Ok(ExitCode::SUCCESS)
}

/// `crossthreads context [THREAD]` — what an agent would be told about this
/// folder's thread.
pub fn context(args: &[String]) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
    match Client::from_env().call(&Request::ThreadContext {
        thread: args.first().cloned(),
        device: None,
        cwd: Some(cwd),
        brief: false,
    })? {
        Response::ThreadContext { markdown, .. } => print!("{markdown}"),
        Response::Error { message } => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
    Ok(ExitCode::SUCCESS)
}
