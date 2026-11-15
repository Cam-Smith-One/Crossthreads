//! `crossthreads room …` — the room tools from a shell.
//!
//! Agents normally use the MCP tools (`crossthreads_room_send`, …). A chat that
//! was already open before Crossthreads was set up doesn't have them, so these
//! commands do the same operations through the running daemon. The hooks
//! recognise them too, so auto-wake and claims keep working.
//!
//! `--room` defaults to the current directory, which the daemon resolves to
//! the repository's room, so agents in the same project still meet.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ct_daemon::{Client, Request, Response};

const USAGE: &str = "\
crossthreads room — talk to other agents in this project's room

USAGE:
    crossthreads room history [--me NAME] [--limit N]
    crossthreads room send --me NAME [--to NAME] [--no-wait] [--timeout S] <TEXT…>
    crossthreads room wait --me NAME [--timeout S]
    crossthreads room claim --me NAME [--minutes N] <PATH…>
    crossthreads room release --me NAME [PATH…]
    crossthreads room import [PATH]     bring in a Meat Proxy database
                                        (default ~/.meat-proxy/meat-proxy.db)

--room <PATH|NAME> picks a room; it defaults to the current directory, which
means this repository's room. `send` posts and then waits for the reply (up to
--timeout seconds, default 50). Start a message with DONE to close the exchange.
";

fn absolute(path: &str) -> String {
    let p = PathBuf::from(path);
    let p = if p.is_absolute() {
        p
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    p.to_string_lossy().into_owned()
}

pub fn run(args: &[String]) -> Result<ExitCode> {
    let Some((cmd, rest)) = args.split_first() else {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    };
    if matches!(cmd.as_str(), "help" | "--help" | "-h") {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }

    let mut me: Option<String> = None;
    let mut room: Option<String> = None;
    let mut to: Option<String> = None;
    let mut wait = true;
    let mut timeout: u64 = 50;
    let mut limit: usize = 30;
    let mut minutes: i64 = 20;
    let mut db: Option<PathBuf> = None;
    let mut words: Vec<String> = Vec::new();

    let mut it = rest.iter();
    let value = |it: &mut std::slice::Iter<'_, String>, flag: &str| -> Result<String> {
        it.next()
            .cloned()
            .with_context(|| format!("{flag} needs a value"))
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--me" => me = Some(value(&mut it, "--me")?),
            "--room" => room = Some(value(&mut it, "--room")?),
            "--to" => to = Some(value(&mut it, "--to")?),
            "--no-wait" => wait = false,
            "--timeout" => timeout = value(&mut it, "--timeout")?.parse().context("--timeout")?,
            "--limit" => limit = value(&mut it, "--limit")?.parse().context("--limit")?,
            "--minutes" => minutes = value(&mut it, "--minutes")?.parse().context("--minutes")?,
            "--db" => db = Some(PathBuf::from(value(&mut it, "--db")?)),
            other if other.starts_with("--") => bail!("unknown option for `room`: {other}"),
            other => words.push(other.to_string()),
        }
    }

    if cmd == "import" {
        return import(words.first().map(PathBuf::from), db);
    }

    let room = room.unwrap_or_else(|| {
        std::env::current_dir()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let need_me = || {
        me.clone()
            .context("--me <NAME> is required (e.g. --me claude)")
    };
    let req = match cmd.as_str() {
        "send" => {
            if words.is_empty() {
                bail!("nothing to send\n\n{USAGE}");
            }
            Request::RoomSend {
                me: need_me()?,
                text: words.join(" "),
                room,
                to,
                wait,
                timeout,
            }
        }
        "wait" => Request::RoomWait {
            me: need_me()?,
            room,
            timeout,
        },
        "history" => Request::RoomHistory {
            me: me.clone(),
            room,
            limit,
        },
        "claim" => Request::RoomClaim {
            me: need_me()?,
            paths: words.iter().map(|p| absolute(p)).collect(),
            room,
            minutes,
        },
        "release" => Request::RoomRelease {
            me: need_me()?,
            paths: (!words.is_empty()).then(|| words.iter().map(|p| absolute(p)).collect()),
        },
        other => bail!("unknown room command: {other}\n\n{USAGE}"),
    };

    // `send` and `wait` block by design, so give the daemon that long to answer.
    let client = Client::from_env();
    let answer = client
        .call_timeout(&req, Duration::from_secs(timeout + 15))
        .with_context(|| {
            format!(
                "the Crossthreads daemon isn't reachable at {} — start it with `crossthreads-up`",
                client.addr()
            )
        })?;
    match answer {
        Response::Text { text } => println!("{text}"),
        Response::Error { message } => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
    Ok(ExitCode::SUCCESS)
}

/// `crossthreads room import` — copy a Meat Proxy database's messages in.
fn import(path: Option<PathBuf>, db: Option<PathBuf>) -> Result<ExitCode> {
    let path = path.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".meat-proxy")
            .join("meat-proxy.db")
    });
    if !path.is_file() {
        bail!("no Meat Proxy database at {}", path.display());
    }
    let store = ct_store::Store::open(crate::resolve_db(db)?)?;
    // Meat Proxy named a room after the project folder. Keep those names: the
    // original path is gone, so there is no repository to resolve them to.
    let imported = store.import_meat_proxy(&path, &|room| room.to_lowercase())?;
    if imported == 0 {
        println!("Nothing new to import from {}.", path.display());
    } else {
        println!(
            "Imported {imported} message(s) from {}. Open the board's Rooms view to read them.",
            path.display()
        );
    }
    Ok(ExitCode::SUCCESS)
}
