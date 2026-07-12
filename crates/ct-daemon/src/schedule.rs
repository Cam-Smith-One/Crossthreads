//! Turn the weekly review from *pull* into *push*: generate a platform scheduler
//! entry that runs `crossthreads weekly --deliver` on a weekly cadence, and
//! install / remove / report it. macOS uses a launchd user agent; elsewhere a
//! line in the user crontab. The manifest generators and the crontab-block
//! transforms here are **pure** (and unit-tested); the CLI wires them to the
//! filesystem and `launchctl`/`crontab`.

use anyhow::{bail, Result};

/// The launchd label / crontab marker identifying our managed entry.
pub const LABEL: &str = "co.crossthreads.weekly";

/// Which scheduler backs this platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// macOS launchd user agent (`~/Library/LaunchAgents/<LABEL>.plist`).
    Launchd,
    /// A line in the user crontab (Linux/BSD).
    Cron,
}

impl Kind {
    /// The scheduler native to the host OS.
    pub fn host() -> Kind {
        if cfg!(target_os = "macos") {
            Kind::Launchd
        } else {
            Kind::Cron
        }
    }
}

/// A weekly delivery cadence: which day, which local hour, and the binary to run.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Day of week, 0 = Sunday … 6 = Saturday.
    pub weekday: u32,
    /// Local hour, 0–23. Delivery fires at minute 0.
    pub hour: u32,
    /// Absolute path to the `crossthreads` binary.
    pub bin: String,
}

impl Plan {
    /// Validate and construct a plan. `weekday` is 0 (Sun)–6 (Sat); `hour` 0–23.
    pub fn new(weekday: u32, hour: u32, bin: impl Into<String>) -> Result<Self> {
        if weekday > 6 {
            bail!("weekday must be 0 (Sun)–6 (Sat), got {weekday}");
        }
        if hour > 23 {
            bail!("hour must be 0–23, got {hour}");
        }
        Ok(Self {
            weekday,
            hour,
            bin: bin.into(),
        })
    }

    /// Human name of the scheduled day, e.g. `"Monday"`.
    pub fn weekday_name(&self) -> &'static str {
        // 0 = Sunday, matching cron and launchd `Weekday`.
        [
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ][self.weekday as usize % 7]
    }

    /// A one-line human summary of the cadence.
    pub fn summary(&self) -> String {
        format!(
            "every {} at {:02}:00 (local)",
            self.weekday_name(),
            self.hour
        )
    }

    /// The crontab command line (no markers): `min hour * * dow  <bin> weekly --deliver`.
    /// cron's day-of-week is 0–6 with 0 = Sunday, matching our `weekday`.
    pub fn cron_line(&self) -> String {
        format!(
            "0 {} * * {} {} weekly --deliver",
            self.hour, self.weekday, self.bin
        )
    }

    /// A launchd user-agent plist (macOS) that runs the delivery weekly.
    /// launchd's `Weekday` is 0–7 with 0/7 = Sunday, matching our `weekday`.
    pub fn launchd_plist(&self) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>weekly</string>
        <string>--deliver</string>
    </array>
    <key>StartCalendarInterval</key>
    <dict>
        <key>Weekday</key>
        <integer>{weekday}</integer>
        <key>Hour</key>
        <integer>{hour}</integer>
        <key>Minute</key>
        <integer>0</integer>
    </dict>
    <key>RunAtLoad</key>
    <false/>
    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
"#,
            label = LABEL,
            bin = self.bin,
            weekday = self.weekday,
            hour = self.hour,
        )
    }
}

const CRON_BEGIN: &str = "# >>> crossthreads weekly (managed) >>>";
const CRON_END: &str = "# <<< crossthreads weekly (managed) <<<";

/// The managed crontab block for `plan` (begin marker, command, end marker).
pub fn cron_block(plan: &Plan) -> String {
    format!("{CRON_BEGIN}\n{}\n{CRON_END}", plan.cron_line())
}

/// True if `crontab` already contains our managed block.
pub fn has_cron_block(crontab: &str) -> bool {
    crontab.lines().any(|l| l.trim_end() == CRON_BEGIN)
}

/// Return `crontab` with any existing managed block removed. Preserves all other
/// lines and drops a trailing blank left behind by the removal.
pub fn remove_cron_block(crontab: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut in_block = false;
    for line in crontab.lines() {
        let t = line.trim_end();
        if t == CRON_BEGIN {
            in_block = true;
            continue;
        }
        if t == CRON_END {
            in_block = false;
            continue;
        }
        if !in_block {
            out.push(line);
        }
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// Return `crontab` with our managed block set to `plan` — replacing any prior
/// managed block, and leaving every other line untouched. Idempotent.
pub fn apply_cron_block(crontab: &str, plan: &Plan) -> String {
    let base = remove_cron_block(crontab);
    let mut s = base;
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&cron_block(plan));
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan::new(1, 9, "/usr/local/bin/crossthreads").unwrap()
    }

    #[test]
    fn rejects_out_of_range() {
        assert!(Plan::new(7, 9, "x").is_err());
        assert!(Plan::new(1, 24, "x").is_err());
        assert!(Plan::new(0, 0, "x").is_ok());
        assert!(Plan::new(6, 23, "x").is_ok());
    }

    #[test]
    fn cron_line_shape() {
        assert_eq!(
            plan().cron_line(),
            "0 9 * * 1 /usr/local/bin/crossthreads weekly --deliver"
        );
    }

    #[test]
    fn weekday_names_and_summary() {
        assert_eq!(Plan::new(0, 8, "x").unwrap().weekday_name(), "Sunday");
        assert_eq!(plan().weekday_name(), "Monday");
        assert_eq!(plan().summary(), "every Monday at 09:00 (local)");
    }

    #[test]
    fn plist_carries_label_bin_and_schedule() {
        let p = plan().launchd_plist();
        assert!(p.contains("<string>co.crossthreads.weekly</string>"));
        assert!(p.contains("<string>/usr/local/bin/crossthreads</string>"));
        assert!(p.contains("<string>--deliver</string>"));
        assert!(p.contains("<key>Weekday</key>\n        <integer>1</integer>"));
        assert!(p.contains("<key>Hour</key>\n        <integer>9</integer>"));
    }

    #[test]
    fn apply_is_idempotent_and_preserves_others() {
        let existing = "MAILTO=me@example.com\n0 0 * * * /bin/backup\n";
        let once = apply_cron_block(existing, &plan());
        assert!(once.contains("MAILTO=me@example.com"));
        assert!(once.contains("/bin/backup"));
        assert!(has_cron_block(&once));
        // Applying again (e.g. changed cadence) yields exactly one block.
        let twice = apply_cron_block(
            &once,
            &Plan::new(5, 17, "/usr/local/bin/crossthreads").unwrap(),
        );
        assert_eq!(twice.matches(CRON_BEGIN).count(), 1);
        assert!(twice.contains("0 17 * * 5 "));
        assert!(!twice.contains("0 9 * * 1 "));
        // Unmanaged lines survive every rewrite.
        assert!(twice.contains("/bin/backup"));
    }

    #[test]
    fn remove_restores_original() {
        let existing = "0 0 * * * /bin/backup\n";
        let installed = apply_cron_block(existing, &plan());
        let removed = remove_cron_block(&installed);
        assert_eq!(removed, existing);
        assert!(!has_cron_block(&removed));
    }

    #[test]
    fn remove_from_empty_is_empty() {
        assert_eq!(remove_cron_block(""), "");
        assert!(!has_cron_block(""));
    }
}
