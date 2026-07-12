//! `crossthreads schedule <install|uninstall|status>` — turn the weekly review
//! from pull into push. Installs a platform scheduler entry (launchd user agent
//! on macOS, a managed crontab block elsewhere) that runs
//! `crossthreads weekly --deliver` once a week. The manifest/crontab logic lives
//! in `ct_daemon::schedule` (pure, unit-tested); this file wires it to the
//! filesystem, `launchctl`, and `crontab`.

use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use anyhow::{bail, Context, Result};
use ct_daemon::schedule::{self, Kind, Plan, LABEL};

const USAGE: &str = "\
crossthreads schedule — deliver your weekly review automatically

USAGE:
    crossthreads schedule <install|uninstall|status> [OPTIONS]

OPTIONS:
    --day <NAME|0-6>   Day to deliver (default: Monday; 0 = Sunday)
    --hour <0-23>      Local hour to deliver (default: 9)

Installs a launchd agent (macOS) or a managed crontab line (Linux) that runs
`crossthreads weekly --deliver` on the chosen cadence.";

pub fn run(args: &[String]) -> Result<ExitCode> {
    let Some(sub) = args.first().map(String::as_str) else {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    };

    let mut weekday: u32 = 1; // Monday
    let mut hour: u32 = 9;
    let mut it = args[1..].iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--day" => {
                let v = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--day needs a value"))?;
                weekday = parse_weekday(v)?;
            }
            "--hour" => {
                let v = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--hour needs a value"))?;
                hour = v.parse().with_context(|| format!("bad --hour: {v}"))?;
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            other => bail!("unknown option for `schedule`: {other}"),
        }
    }

    match sub {
        "status" => status(),
        "install" => {
            let bin = current_bin()?;
            let plan = Plan::new(weekday, hour, bin)?;
            install(&plan)
        }
        "uninstall" => uninstall(),
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        other => {
            eprintln!("unknown subcommand: {other}\n");
            print!("{USAGE}");
            Ok(ExitCode::FAILURE)
        }
    }
}

fn parse_weekday(s: &str) -> Result<u32> {
    if let Ok(n) = s.parse::<u32>() {
        if n <= 6 {
            return Ok(n);
        }
        bail!("--day number must be 0 (Sun)–6 (Sat), got {n}");
    }
    let names = [
        "sunday",
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
    ];
    let lc = s.to_lowercase();
    names
        .iter()
        .position(|n| n.starts_with(&lc))
        .map(|i| i as u32)
        .ok_or_else(|| anyhow::anyhow!("unrecognized day: {s}"))
}

/// Absolute path to this binary, so the scheduler can find it after we exit.
fn current_bin() -> Result<String> {
    let exe = std::env::current_exe().context("locating the crossthreads binary")?;
    Ok(exe.to_string_lossy().into_owned())
}

// ---- launchd (macOS) -------------------------------------------------------

fn launchd_plist_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

fn install_launchd(plan: &Plan) -> Result<ExitCode> {
    let path = launchd_plist_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, plan.launchd_plist())
        .with_context(|| format!("writing {}", path.display()))?;
    // Reload so the new cadence takes effect (ignore unload errors — it may not
    // be loaded yet). Best-effort: the plist on disk is the source of truth.
    let _ = Command::new("launchctl")
        .args(["unload", &path.to_string_lossy()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let loaded = Command::new("launchctl")
        .args(["load", &path.to_string_lossy()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    println!("Installed launchd agent → {}", path.display());
    println!("Delivering {}.", plan.summary());
    if !matches!(loaded, Ok(s) if s.success()) {
        eprintln!(
            "note: could not `launchctl load` automatically; run:\n  launchctl load {}",
            path.display()
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn uninstall_launchd() -> Result<ExitCode> {
    let path = launchd_plist_path()?;
    if path.exists() {
        let _ = Command::new("launchctl")
            .args(["unload", &path.to_string_lossy()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        println!("Removed launchd agent {}", path.display());
    } else {
        println!("No launchd agent installed.");
    }
    Ok(ExitCode::SUCCESS)
}

fn status_launchd() -> Result<ExitCode> {
    let path = launchd_plist_path()?;
    if path.exists() {
        println!("Scheduled: launchd agent at {}", path.display());
    } else {
        println!("Not scheduled. Run `crossthreads schedule install`.");
    }
    Ok(ExitCode::SUCCESS)
}

// ---- cron (Linux/BSD) ------------------------------------------------------

/// Read the current user crontab (empty string if none is set yet).
fn read_crontab() -> Result<String> {
    let out = Command::new("crontab").arg("-l").output();
    match out {
        // A missing crontab exits non-zero with "no crontab for …" — treat as empty.
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(_) => Ok(String::new()),
        Err(e) => Err(e).context("running `crontab -l` (is cron installed?)"),
    }
}

/// Replace the user crontab with `content` via `crontab -`.
fn write_crontab(content: &str) -> Result<()> {
    use std::io::Write;
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(Stdio::piped())
        .spawn()
        .context("running `crontab -` (is cron installed?)")?;
    child
        .stdin
        .take()
        .context("opening crontab stdin")?
        .write_all(content.as_bytes())
        .context("writing crontab")?;
    let status = child.wait().context("waiting for crontab")?;
    if !status.success() {
        bail!("`crontab -` failed");
    }
    Ok(())
}

fn install_cron(plan: &Plan) -> Result<ExitCode> {
    let current = read_crontab()?;
    let next = schedule::apply_cron_block(&current, plan);
    write_crontab(&next)?;
    println!("Installed crontab entry ({}).", LABEL);
    println!("Delivering {}.", plan.summary());
    Ok(ExitCode::SUCCESS)
}

fn uninstall_cron() -> Result<ExitCode> {
    let current = read_crontab()?;
    if schedule::has_cron_block(&current) {
        let next = schedule::remove_cron_block(&current);
        write_crontab(&next)?;
        println!("Removed crontab entry ({}).", LABEL);
    } else {
        println!("No crontab entry installed.");
    }
    Ok(ExitCode::SUCCESS)
}

fn status_cron() -> Result<ExitCode> {
    let current = read_crontab()?;
    if schedule::has_cron_block(&current) {
        println!("Scheduled: crontab entry ({LABEL}) present.");
    } else {
        println!("Not scheduled. Run `crossthreads schedule install`.");
    }
    Ok(ExitCode::SUCCESS)
}

// ---- dispatch by host scheduler --------------------------------------------

fn install(plan: &Plan) -> Result<ExitCode> {
    match Kind::host() {
        Kind::Launchd => install_launchd(plan),
        Kind::Cron => install_cron(plan),
    }
}

fn uninstall() -> Result<ExitCode> {
    match Kind::host() {
        Kind::Launchd => uninstall_launchd(),
        Kind::Cron => uninstall_cron(),
    }
}

fn status() -> Result<ExitCode> {
    match Kind::host() {
        Kind::Launchd => status_launchd(),
        Kind::Cron => status_cron(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_weekday;

    #[test]
    fn parses_names_prefixes_and_numbers() {
        assert_eq!(parse_weekday("Monday").unwrap(), 1);
        assert_eq!(parse_weekday("mon").unwrap(), 1);
        assert_eq!(parse_weekday("SUN").unwrap(), 0);
        assert_eq!(parse_weekday("5").unwrap(), 5);
        assert!(parse_weekday("9").is_err());
        assert!(parse_weekday("funday").is_err());
    }
}
