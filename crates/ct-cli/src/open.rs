//! `crossthreads open` — open the web app, signed in.
//!
//! The daemon only answers clients on this machine that present the local
//! key, and a browser can't read that key. So this command asks the daemon
//! (with the key) for a one-time ticket that works for 60 seconds, and opens
//! `http://127.0.0.1:47101/login#<ticket>`; the page trades it for a cookie.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use ct_daemon::{Client, Request, Response};

const USAGE: &str = "\
crossthreads open — open the Crossthreads web app in your browser, signed in

USAGE:
    crossthreads open [--print] [--wait]

    --print   print the one-time sign-in link instead of opening a browser
    --wait    wait up to 30 seconds for the daemon to start (used by crossthreads-up)
";

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut print = false;
    let mut wait = false;
    for a in args {
        match a.as_str() {
            "--print" => print = true,
            "--wait" => wait = true,
            "-h" | "--help" | "help" => {
                print!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            other => bail!("unknown option for `open`: {other}\n\n{USAGE}"),
        }
    }
    let client = Client::from_env();
    let deadline = Instant::now() + Duration::from_secs(if wait { 30 } else { 0 });
    let url = loop {
        match client.call(&Request::LoginTicket) {
            Ok(Response::LoginTicket { url, .. }) => break url,
            Ok(Response::Error { message }) if Instant::now() >= deadline => bail!(message),
            Err(e) if Instant::now() >= deadline => {
                bail!("{e:#}\nIs Crossthreads running? Start it with `crossthreads-up`.")
            }
            Ok(other) if Instant::now() >= deadline => bail!("unexpected response: {other:?}"),
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    };
    if print || !open_browser(&url) {
        println!("Open this link within 60 seconds (it works once):\n{url}");
    } else {
        println!("Opened Crossthreads in your browser.");
    }
    Ok(ExitCode::SUCCESS)
}

/// Hand the link to the system's browser. The link is passed as one argument
/// and never through a shell.
fn open_browser(url: &str) -> bool {
    let status = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).status()
    } else if cfg!(windows) {
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .status()
    } else {
        std::process::Command::new("xdg-open").arg(url).status()
    };
    status.is_ok_and(|s| s.success())
}
