//! `crossthreads-relay`: run a Crossthreads relay. See `docs/RELAY.md`.

use std::process::ExitCode;
use std::time::Duration;

use ct_relay::{Config, Relay};

const USAGE: &str = "\
crossthreads-relay: forwards end-to-end encrypted traffic between paired Crossthreads devices.

USAGE:
    crossthreads-relay [--listen ADDR] [OPTIONS]

OPTIONS:
    --listen ADDR                 Address to listen on [default: 0.0.0.0:47200]
    --max-conns N                 Open connections in total [default: 4096]
    --max-conns-per-ip N          Open connections per IP address [default: 64]
    --max-new-conns-per-min N     New connections per IP address per minute [default: 600]
    --max-registrations N         Registered devices [default: 10000]
    --max-slots N                 Pairing codes open at once [default: 4096]
    --max-pair-ttl SECS           Longest a pairing code may stay open [default: 900]
    --idle-timeout SECS           Close a tunnel idle this long [default: 600]
    --rate BYTES                  Bytes per second per tunnel direction, 0 = unlimited [default: 8388608]
    --max-bytes BYTES             Bytes per tunnel direction before it is cut, 0 = unlimited [default: 4294967296]
    --quiet                       Don't log connection events
    -h, --help                    Show this help
    -V, --version                 Show the version

The relay never sees content: devices encrypt end to end (Noise KK) and pair with SPAKE2.
It logs IP addresses and short public-key prefixes, never data.
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("crossthreads-relay: {e}");
            eprintln!("Try --help.");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let mut listen = "0.0.0.0:47200".to_string();
    let mut c = Config::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = || -> Result<String, String> {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        let num = |v: String| -> Result<u64, String> {
            v.parse::<u64>()
                .map_err(|_| format!("{flag}: not a number: {v}"))
        };
        match flag.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("crossthreads-relay {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--listen" => listen = value()?,
            "--max-conns" => c.max_conns = num(value()?)? as usize,
            "--max-conns-per-ip" => c.max_conns_per_ip = num(value()?)? as usize,
            "--max-new-conns-per-min" => {
                c.max_new_conns_per_ip_per_min = num(value()?)?.min(u32::MAX as u64) as u32
            }
            "--max-registrations" => c.max_registrations = num(value()?)? as usize,
            "--max-slots" => c.max_slots = num(value()?)? as usize,
            "--max-pair-ttl" => c.max_pair_ttl = Duration::from_secs(num(value()?)?.max(1)),
            "--idle-timeout" => c.idle_timeout = Duration::from_secs(num(value()?)?.max(1)),
            "--rate" => c.rate_bytes_per_sec = num(value()?)?,
            "--max-bytes" => c.max_bytes_per_conn = num(value()?)?,
            "--quiet" => c.log = false,
            other => return Err(format!("unknown option {other}")),
        }
    }
    let relay = Relay::bind(&listen, c).map_err(|e| format!("can't listen on {listen}: {e}"))?;
    relay.run();
    Ok(())
}
