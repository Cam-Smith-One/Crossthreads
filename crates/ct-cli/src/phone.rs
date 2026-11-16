//! `crossthreads phone …` — the board on your phone, over your tailnet
//! (SPEC FR-BRD-04). The same settings as Settings → Phone in the web app.

use std::process::ExitCode;

use anyhow::{bail, Result};
use ct_daemon::{Client, Request, Response};

const USAGE: &str = "\
crossthreads phone — open your board on your phone, over Tailscale

USAGE:
    crossthreads phone                 show whether it's on, its address, paired phones
    crossthreads phone on | off        start or stop listening on this device's tailnet address
    crossthreads phone pair            show a QR code and one-time code to pair a phone
    crossthreads phone post <ID> on|off
                                       let a paired phone post to rooms (off by default)
    crossthreads phone revoke <ID>     forget a paired phone

Your phone and this computer must both be signed in to the same Tailscale
network. Phones can read threads, rooms and search; nothing else.
";

pub fn run(args: &[String]) -> Result<ExitCode> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let req = match words.as_slice() {
        [] | ["status"] => Request::PhoneStatus,
        ["on"] => Request::PhoneSetEnabled { enabled: true },
        ["off"] => Request::PhoneSetEnabled { enabled: false },
        ["pair"] => Request::PhonePair,
        ["revoke", id] => Request::PhoneRevoke { id: id.to_string() },
        ["post", id, state] => Request::PhoneSetPost {
            id: id.to_string(),
            can_post: match *state {
                "on" | "yes" | "true" => true,
                "off" | "no" | "false" => false,
                other => bail!("expected on or off, got `{other}`"),
            },
        },
        ["help" | "--help" | "-h"] => {
            print!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        _ => bail!("{USAGE}"),
    };
    match Client::from_env().call(&req)? {
        Response::Phone {
            enabled,
            listening,
            url,
            error,
            phones,
        } => {
            match (enabled, listening, url) {
                (_, true, Some(url)) => println!("Phone access is on: {url}"),
                (true, _, _) => println!("Phone access is on but not listening."),
                _ => println!("Phone access is off. `crossthreads phone on` to start it."),
            }
            if let Some(e) = error {
                println!("  {e}");
            }
            if phones.is_empty() {
                if listening {
                    println!("No phones paired yet. `crossthreads phone pair` to add one.");
                }
            } else {
                println!("\nPaired phones:");
                for p in phones {
                    let seen = chrono::DateTime::from_timestamp(p.last_seen, 0)
                        .map(|t| {
                            t.with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M")
                                .to_string()
                        })
                        .unwrap_or_else(|| "never".into());
                    println!(
                        "  {}  {:<20} last seen {seen}{}",
                        p.id,
                        p.name,
                        if p.can_post {
                            ", can post to rooms"
                        } else {
                            ""
                        }
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Response::PhonePairing {
            code,
            url,
            expires_secs,
            ..
        } => {
            if let Ok(qr) = qrcode::QrCode::new(url.as_bytes()) {
                let art = qr
                    .render::<qrcode::render::unicode::Dense1x2>()
                    .quiet_zone(true)
                    .build();
                println!("{art}");
            }
            println!("Scan with your phone's camera, or open {url}");
            println!(
                "and enter the code {code}. It works once, for {} minutes.",
                expires_secs / 60
            );
            Ok(ExitCode::SUCCESS)
        }
        Response::Error { message } => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
}
