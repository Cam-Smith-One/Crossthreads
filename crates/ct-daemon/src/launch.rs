//! "Continue in Claude Code / Codex": open a new terminal window running an AI
//! coding tool in a folder, optionally with an initial prompt or resuming a
//! native session.
//!
//! The prompt is arbitrary text (it comes from transcripts and notes), so it is
//! **never** put on a command line a shell parses. It is written to a private
//! file (0600) under `<data_local_dir>/crossthreads/launch/`, and a small
//! generated launcher script (0700) reads it into a variable and passes it to
//! the tool as one argument. The script only ever contains quoted paths, the
//! validated resume id and fixed text. The launcher deletes the prompt file and
//! itself once it has read them; anything left over (a terminal that never
//! started) is swept an hour later by the next launch.
//!
//! Terminals:
//! - **macOS**: Terminal.app via `open -a Terminal <script>.command` (no
//!   Automation permission prompt). `CROSSTHREADS_TERMINAL` picks another app:
//!   `iTerm` (`open -a iTerm <script>`), `Ghostty` (`open -na Ghostty --args -e
//!   <script>`), `WezTerm` (`wezterm start --cwd <dir> -- <script>` from the app
//!   bundle), and any other name is tried as `open -a <name> <script>`, which
//!   works for apps that run a `.command` file they are asked to open.
//! - **Linux**: `CROSSTHREADS_TERMINAL`, then `$TERMINAL`, then the first of
//!   `x-terminal-emulator`, `gnome-terminal`, `konsole`, `kitty`, `alacritty`,
//!   `wezterm`, `xfce4-terminal`, `foot`, `xterm` found on `PATH`, each with the
//!   flag it uses to run a program.
//! - **Windows**: Windows Terminal (`wt.exe -w new -d <dir> powershell -File
//!   <script>.ps1`) when present, else a new PowerShell console window. The
//!   prompt is read with `Get-Content -Raw` into a variable and passed with
//!   `&`, never interpolated into a command string.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Leftover launcher files older than this are removed by the next launch.
const SWEEP_AFTER: Duration = Duration::from_secs(60 * 60);

/// An AI coding tool we can open in a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tool {
    ClaudeCode,
    Codex,
    /// xAI's Grok Build (`grok`).
    Grok,
}

impl Tool {
    /// The CLI's program name.
    pub fn program(self) -> &'static str {
        match self {
            Tool::ClaudeCode => "claude",
            Tool::Codex => "codex",
            Tool::Grok => "grok",
        }
    }

    /// The name people know it by.
    pub fn display_name(self) -> &'static str {
        match self {
            Tool::ClaudeCode => "Claude Code",
            Tool::Codex => "Codex",
            Tool::Grok => "Grok Build",
        }
    }

    /// How the initial prompt is given: after `--` (Claude Code, Codex), or
    /// as `--prompt=<text>` (Grok Build). Either way it is one argument.
    fn prompt_flag(self) -> &'static str {
        match self {
            Tool::Grok => "--prompt=",
            _ => "--",
        }
    }

    /// From an indexed conversation's tool id (`claude-code`, `codex`).
    pub fn from_id(id: &str) -> Option<Tool> {
        match id {
            "claude-code" | "claude" => Some(Tool::ClaudeCode),
            "codex" => Some(Tool::Codex),
            "grok-build" | "grok" => Some(Tool::Grok),
            _ => None,
        }
    }
}

/// What to open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchPlan {
    pub tool: Tool,
    /// The folder to run the tool in. Must exist.
    pub cwd: PathBuf,
    /// An initial prompt (arbitrary text; never seen by a shell).
    pub prompt: Option<String>,
    /// A native session id to resume.
    pub resume: Option<String>,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// A resume id must match `^[A-Za-z0-9_.:-]{1,128}$` and must not start with
/// `-` (it would read as a flag).
fn validate_resume_id(id: &str) -> Result<()> {
    let ok = (1..=128).contains(&id.len())
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'));
    if !ok {
        bail!("not a valid session id to resume: {id:?}");
    }
    Ok(())
}

/// The folder, canonicalized; it must be an existing directory.
fn validate_cwd(cwd: &Path) -> Result<PathBuf> {
    let canon = fs::canonicalize(cwd)
        .with_context(|| format!("the folder {} doesn't exist", cwd.display()))?;
    if !canon.is_dir() {
        bail!("{} is not a folder", cwd.display());
    }
    Ok(strip_verbatim(canon))
}

/// Windows' `canonicalize` returns `\\?\C:\…` paths, which some programs
/// (PowerShell's `Set-Location`, Windows Terminal) handle badly. Turn
/// `\\?\C:\x` into `C:\x` and `\\?\UNC\srv\x` into `\\srv\x`.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let Some(s) = p.to_str() else { return p };
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    p
}

/// A prompt worth sending: not empty or only whitespace.
fn effective_prompt(p: Option<&str>) -> Option<&str> {
    p.filter(|s| !s.trim().is_empty())
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str()
        .with_context(|| format!("path is not valid UTF-8: {}", p.display()))
}

// ---------------------------------------------------------------------------
// The tool's argv
// ---------------------------------------------------------------------------

/// The tool's arguments (after the program) and whether the prompt is passed.
/// The prompt itself is never in here: the launcher appends `-- <prompt>` from
/// the prompt file when `prompt` is true, so a prompt starting with `-` can't be
/// read as a flag.
fn tool_args(tool: Tool, resume: Option<&str>, has_prompt: bool) -> (Vec<String>, bool) {
    match resume {
        // `claude --resume <id> [prompt]` is documented. `codex resume <id>`
        // takes no initial prompt we can rely on, so resume without one.
        Some(id) => {
            let args = crate::resume::resume_args(tool.program(), id)
                .expect("every tool here has a resume command");
            (args, has_prompt && tool == Tool::ClaudeCode)
        }
        None => (Vec::new(), has_prompt),
    }
}

// ---------------------------------------------------------------------------
// Finding the CLI
// ---------------------------------------------------------------------------

/// Where the tool's CLI is installed, or `None`.
///
/// Looks on `PATH`, then in common install locations, since a macOS GUI app's
/// daemon gets a minimal `PATH`.
pub fn tool_available(tool: Tool) -> Option<PathBuf> {
    let program = tool.program();
    if let Some(p) = find_on_path(program) {
        return Some(p);
    }
    candidate_paths(tool).into_iter().find(|p| is_executable(p))
}

fn exe_names(program: &str) -> Vec<String> {
    if cfg!(windows) {
        // Not all of PATHEXT: a `claude.js` or `.vbs` would run under WSH.
        [".exe", ".cmd", ".bat"]
            .iter()
            .map(|e| format!("{program}{e}"))
            .collect()
    } else {
        vec![program.to_string()]
    }
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names = exe_names(program);
    std::env::split_paths(&path)
        .filter(|d| d.is_absolute())
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| is_executable(p))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Common install locations, most specific first.
fn candidate_paths(tool: Tool) -> Vec<PathBuf> {
    let program = tool.program();
    let home = dirs::home_dir();
    let mut dirs_: Vec<PathBuf> = Vec::new();
    let mut out: Vec<PathBuf> = Vec::new();

    if cfg!(windows) {
        if let Some(h) = &home {
            if tool == Tool::ClaudeCode {
                out.push(h.join(".local").join("bin").join("claude.exe"));
            }
        }
        if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
            out.push(appdata.join("npm").join(format!("{program}.cmd")));
        }
        if let Some(prefix) = npm_prefix() {
            out.push(prefix.join(format!("{program}.cmd")));
        }
        return out;
    }

    if let Some(h) = &home {
        if tool == Tool::ClaudeCode {
            // The "migrate to local install" location.
            out.push(h.join(".claude").join("local").join("claude"));
        }
        dirs_.push(h.join(".local").join("bin"));
        dirs_.push(h.join(".npm-global").join("bin"));
    }
    if let Some(prefix) = npm_prefix() {
        dirs_.push(prefix.join("bin"));
    }
    dirs_.push(PathBuf::from("/opt/homebrew/bin"));
    dirs_.push(PathBuf::from("/usr/local/bin"));
    if let Some(h) = &home {
        dirs_.push(h.join(".bun").join("bin"));
        dirs_.push(h.join(".volta").join("bin"));
        // nvm: newest installed Node first (by name, good enough for vNN.x.y).
        if let Ok(rd) = fs::read_dir(h.join(".nvm").join("versions").join("node")) {
            let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path().join("bin")).collect();
            v.sort();
            v.reverse();
            dirs_.extend(v);
        }
    }
    dirs_.push(PathBuf::from("/usr/bin"));
    out.extend(dirs_.into_iter().map(|d| d.join(program)));
    out
}

/// A custom npm global prefix: `NPM_CONFIG_PREFIX`, else `prefix=` in
/// `~/.npmrc`. (Running `npm` would be slow and needs `node` on `PATH`.)
fn npm_prefix() -> Option<PathBuf> {
    if let Some(p) =
        std::env::var_os("NPM_CONFIG_PREFIX").or_else(|| std::env::var_os("npm_config_prefix"))
    {
        return Some(PathBuf::from(p));
    }
    let home = dirs::home_dir()?;
    let text = fs::read_to_string(home.join(".npmrc")).ok()?;
    parse_npmrc_prefix(&text, &home)
}

fn parse_npmrc_prefix(text: &str, home: &Path) -> Option<PathBuf> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        if k.trim() != "prefix" {
            return None;
        }
        let v = v.trim().trim_matches('"');
        if v.is_empty() {
            return None;
        }
        Some(match v.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(v),
        })
    })
}

// ---------------------------------------------------------------------------
// POSIX launcher script
// ---------------------------------------------------------------------------

/// Single-quote for a POSIX shell, always: `'` → `'\''`.
#[cfg_attr(windows, allow(dead_code))]
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The `/bin/sh` launcher. Every interpolated value is single-quoted; the
/// prompt is only ever the *contents* of `prompt_file`, read at run time.
#[cfg(test)]
fn unix_script(
    tool_path: &Path,
    args: &[String],
    cwd: &Path,
    prompt_file: Option<&Path>,
    script_path: &Path,
) -> Result<String> {
    unix_script_with(tool_path, args, cwd, prompt_file, script_path, "--")
}

/// [`unix_script`] with the tool's way of taking a prompt (see
/// [`Tool::prompt_flag`]).
#[cfg_attr(windows, allow(dead_code))]
fn unix_script_with(
    tool_path: &Path,
    args: &[String],
    cwd: &Path,
    prompt_file: Option<&Path>,
    script_path: &Path,
    prompt_flag: &str,
) -> Result<String> {
    let mut s = String::from(
        "#!/bin/sh\n# Generated by Crossthreads to open an AI coding tool. Safe to delete.\n",
    );
    let mut cleanup = vec![sh_quote(path_str(script_path)?)];
    if let Some(pf) = prompt_file {
        let q = sh_quote(path_str(pf)?);
        s.push_str(&format!("p=\"$(cat -- {q})\" || exit 1\n"));
        cleanup.insert(0, q);
    }
    s.push_str(&format!("rm -f -- {}\n", cleanup.join(" ")));
    // npm-installed CLIs are `#!/usr/bin/env node` scripts: put the tool's own
    // folder (where npm/nvm keep `node` too) on PATH for a minimal environment.
    if let Some(dir) = tool_path.parent() {
        s.push_str(&format!(
            "PATH={}:\"$PATH\"; export PATH\n",
            sh_quote(path_str(dir)?)
        ));
    }
    let mut cmd = vec![sh_quote(path_str(tool_path)?)];
    cmd.extend(args.iter().map(|a| sh_quote(a)));
    if prompt_file.is_some() {
        if prompt_flag == "--" {
            cmd.push("--".into());
            cmd.push("\"$p\"".into());
        } else {
            // `--prompt="$p"`: one argument, the flag and the text.
            cmd.push(format!("{}\"$p\"", sh_quote(prompt_flag)));
        }
    }
    s.push_str(&format!(
        "cd -- {} && exec {}\n",
        sh_quote(path_str(cwd)?),
        cmd.join(" ")
    ));
    Ok(s)
}

// ---------------------------------------------------------------------------
// Windows launcher script (PowerShell)
// ---------------------------------------------------------------------------

/// How PowerShell should start the tool on Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
enum WinInvoke {
    /// A real executable (`claude.exe`).
    Exe(PathBuf),
    /// An npm `.cmd` shim, bypassed: run its JS entry with `node` directly, so
    /// `cmd.exe` never parses the prompt.
    Node { node: PathBuf, script: PathBuf },
    /// A `.cmd`/`.bat` we couldn't see through. `cmd.exe` will parse the
    /// arguments, so the prompt is flattened to text cmd treats as literal.
    Batch(PathBuf),
}

/// PowerShell single-quoted literal. PowerShell also treats the typographic
/// single quotes as quote characters, so those are doubled too.
#[cfg_attr(not(windows), allow(dead_code))]
fn ps_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// The JS entry an npm `.cmd` shim runs: the `"%dp0%\…\x.js"` in its last line.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_npm_shim(text: &str, shim_dir: &Path) -> Option<PathBuf> {
    let start = text.find("\"%dp0%\\")? + "\"%dp0%\\".len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    let rel = &rest[..end];
    let lower = rel.to_ascii_lowercase();
    if !(lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs")) {
        return None;
    }
    if rel.contains('%') || rel.split(['\\', '/']).any(|c| c == "..") {
        return None;
    }
    let mut p = shim_dir.to_path_buf();
    for part in rel.split(['\\', '/']).filter(|c| !c.is_empty()) {
        p.push(part);
    }
    Some(p)
}

/// The PowerShell launcher. The prompt is read with `Get-Content -Raw` into
/// `$prompt` and passed as a single argument with `&`; it is never part of a
/// string PowerShell evaluates.
#[cfg(test)]
fn windows_script(
    invoke: &WinInvoke,
    args: &[String],
    cwd: &Path,
    prompt_file: Option<&Path>,
    script_path: &Path,
) -> Result<String> {
    windows_script_with(invoke, args, cwd, prompt_file, script_path, "--")
}

/// [`windows_script`] with the tool's way of taking a prompt.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_script_with(
    invoke: &WinInvoke,
    args: &[String],
    cwd: &Path,
    prompt_file: Option<&Path>,
    script_path: &Path,
    prompt_flag: &str,
) -> Result<String> {
    let mut s = String::from(
        "# Generated by Crossthreads to open an AI coding tool. Safe to delete.\r\n\
         $ErrorActionPreference = 'Stop'\r\n",
    );
    if let Some(pf) = prompt_file {
        s.push_str(&format!(
            "$promptFile = {}\r\n\
             $prompt = Get-Content -LiteralPath $promptFile -Raw -Encoding UTF8\r\n\
             Remove-Item -LiteralPath $promptFile -Force -ErrorAction SilentlyContinue\r\n",
            ps_quote(path_str(pf)?)
        ));
    }
    s.push_str(&format!(
        "Remove-Item -LiteralPath {} -Force -ErrorAction SilentlyContinue\r\n",
        ps_quote(path_str(script_path)?)
    ));
    let (program, mut pre): (&Path, Vec<String>) = match invoke {
        WinInvoke::Exe(p) | WinInvoke::Batch(p) => (p, Vec::new()),
        WinInvoke::Node { node, script } => (node, vec![path_str(script)?.to_string()]),
    };
    // The program's folder on PATH (node's, for a shim we see through), split
    // as a string so the script text is the same whatever OS builds it.
    let program_s = path_str(program)?;
    if let Some((dir, _)) = program_s
        .rsplit_once(['\\', '/'])
        .filter(|(d, _)| !d.is_empty())
    {
        s.push_str(&format!(
            "$env:Path = {} + ';' + $env:Path\r\n",
            ps_quote(dir)
        ));
    }
    s.push_str(&format!(
        "Set-Location -LiteralPath {}\r\n",
        ps_quote(path_str(cwd)?)
    ));
    if prompt_file.is_some() {
        match invoke {
            WinInvoke::Batch(_) => {
                // cmd.exe re-parses a batch file's arguments: newlines end the
                // command, `"` toggles quoting and `%x%` expands. With those
                // gone, everything else inside the quotes PowerShell adds is
                // literal to cmd.
                s.push_str(
                    "$prompt = ($prompt -replace '[\\r\\n]+', ' ') -replace '\"', \"'\" -replace '%', [string][char]0xFF05\r\n",
                );
            }
            _ => {
                // Windows PowerShell 5.1 (and 7.0-7.2) don't escape embedded
                // quotes when passing an argument to a native program; do what
                // 7.3+ does so the prompt arrives as one intact argument.
                s.push_str(
                    "if ($PSVersionTable.PSVersion -lt [version]'7.3') {\r\n\
                     \x20 $prompt = $prompt -replace '(\\\\*)\"', '$1$1\\\"'\r\n\
                     \x20 if ($prompt -match '\\s') { $prompt = $prompt -replace '(\\\\+)$', '$1$1' }\r\n\
                     }\r\n",
                );
            }
        }
    }
    pre.extend(args.iter().cloned());
    let mut cmd = format!("& {}", ps_quote(path_str(program)?));
    for a in &pre {
        cmd.push(' ');
        cmd.push_str(&ps_quote(a));
    }
    if prompt_file.is_some() {
        if prompt_flag == "--" {
            cmd.push_str(" '--' $prompt");
        } else {
            cmd.push_str(&format!(" ({} + $prompt)", ps_quote(prompt_flag)));
        }
    }
    s.push_str(&cmd);
    s.push_str("\r\n");
    Ok(s)
}

// ---------------------------------------------------------------------------
// Terminals
// ---------------------------------------------------------------------------

/// macOS: (program, args) that open `script` in a new window of `app`
/// (`None` → Terminal.app).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_terminal_command(app: Option<&str>, script: &str, cwd: &str) -> (String, Vec<String>) {
    let app = mac_app(app);
    let s = script.to_string();
    match app.to_ascii_lowercase().trim_end_matches(".app") {
        "terminal" => ("open".into(), vec!["-a".into(), "Terminal".into(), s]),
        "iterm" | "iterm2" => ("open".into(), vec!["-a".into(), "iTerm".into(), s]),
        "ghostty" => (
            "open".into(),
            vec![
                "-na".into(),
                "Ghostty".into(),
                "--args".into(),
                "-e".into(),
                s,
            ],
        ),
        "wezterm" => (
            "/Applications/WezTerm.app/Contents/MacOS/wezterm".into(),
            vec!["start".into(), "--cwd".into(), cwd.into(), "--".into(), s],
        ),
        _ => ("open".into(), vec!["-a".into(), app.to_string(), s]),
    }
}

/// The macOS terminal app to use (`CROSSTHREADS_TERMINAL`, else Terminal).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_app(app: Option<&str>) -> &str {
    app.map(str::trim)
        .filter(|a| !a.is_empty())
        .unwrap_or("Terminal")
}

/// Linux terminals we know, in fallback order.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
const LINUX_TERMINALS: &[&str] = &[
    "x-terminal-emulator",
    "gnome-terminal",
    "konsole",
    "kitty",
    "alacritty",
    "wezterm",
    "xfce4-terminal",
    "foot",
    "xterm",
];

/// The arguments that make terminal `program` run `script`.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn linux_terminal_args(program: &Path, script: &str) -> Vec<String> {
    let name = program
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let s = script.to_string();
    match name {
        "gnome-terminal" | "ptyxis" | "kgx" => vec!["--".into(), s],
        "kitty" | "foot" => vec![s],
        "wezterm" => vec!["start".into(), "--".into(), s],
        "xfce4-terminal" => vec!["-x".into(), s],
        // konsole, alacritty, xterm, x-terminal-emulator, tilix, urxvt, …
        _ => vec!["-e".into(), s],
    }
}

/// A user-chosen terminal (`CROSSTHREADS_TERMINAL`, then `$TERMINAL`), or the
/// first known one installed.
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_terminal() -> Option<PathBuf> {
    let chosen = ["CROSSTHREADS_TERMINAL", "TERMINAL"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    for t in chosen {
        let p = PathBuf::from(&t);
        if p.is_absolute() && is_executable(&p) {
            return Some(p);
        }
        if let Some(p) = find_on_path(&t) {
            return Some(p);
        }
    }
    LINUX_TERMINALS.iter().find_map(|t| find_on_path(t))
}

/// Start a helper process and don't wait for the terminal; reap it in the
/// background so it doesn't linger as a zombie.
fn spawn_detached(mut cmd: Command) -> Result<()> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

fn launch_dir() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .context("couldn't find this account's local data folder")?
        .join("crossthreads")
        .join("launch");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// Remove launcher files left over from launches more than an hour ago.
fn sweep(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > SWEEP_AFTER);
        if old {
            let _ = fs::remove_file(e.path());
        }
    }
}

fn random_name() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("the OS random number generator failed");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Create a new file only this account can read (0600, or 0700 for a script),
/// failing if the name is taken.
fn write_private(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(if executable { 0o700 } else { 0o600 });
    }
    #[cfg(not(unix))]
    let _ = executable;
    let mut f = o
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Launch
// ---------------------------------------------------------------------------

/// Open a new terminal window running the plan's tool. Returns a short
/// description of what was opened. Doesn't wait for the terminal.
pub fn launch(plan: &LaunchPlan) -> Result<String> {
    if let Some(id) = &plan.resume {
        validate_resume_id(id)?;
    }
    let cwd = validate_cwd(&plan.cwd)?;
    let tool_path = tool_available(plan.tool).with_context(|| {
        format!(
            "{} isn't installed (couldn't find `{}` on PATH or in the usual install folders)",
            plan.tool.display_name(),
            plan.tool.program()
        )
    })?;
    let prompt = effective_prompt(plan.prompt.as_deref());
    let (args, send_prompt) = tool_args(plan.tool, plan.resume.as_deref(), prompt.is_some());

    let dir = launch_dir()?;
    sweep(&dir);
    let name = random_name();
    let prompt_file = match prompt.filter(|_| send_prompt) {
        Some(p) => {
            let pf = dir.join(format!("{name}.prompt"));
            write_private(&pf, p.as_bytes(), false)?;
            Some(pf)
        }
        None => None,
    };

    let opened_in = open_terminal(
        &dir,
        &name,
        &tool_path,
        &args,
        &cwd,
        prompt_file.as_deref(),
        plan.tool.prompt_flag(),
    );
    if opened_in.is_err() {
        if let Some(pf) = &prompt_file {
            let _ = fs::remove_file(pf);
        }
    }
    let opened_in = opened_in?;

    let mut what = format!(
        "Opened {} in {} at {}",
        plan.tool.display_name(),
        opened_in,
        cwd.display()
    );
    if let Some(id) = &plan.resume {
        what.push_str(&format!(", resuming session {id}"));
    }
    if prompt.is_some() && !send_prompt {
        what.push_str(&format!(
            " (the prompt wasn't passed: {} resumes without one)",
            plan.tool.display_name()
        ));
    }
    Ok(what)
}

#[cfg(target_os = "macos")]
fn open_terminal(
    dir: &Path,
    name: &str,
    tool_path: &Path,
    args: &[String],
    cwd: &Path,
    prompt_file: Option<&Path>,
    prompt_flag: &str,
) -> Result<String> {
    // A `.command` file opens in Terminal.app without an Automation prompt.
    let script = dir.join(format!("{name}.command"));
    let text = unix_script_with(tool_path, args, cwd, prompt_file, &script, prompt_flag)?;
    write_private(&script, text.as_bytes(), true)?;
    let app = std::env::var("CROSSTHREADS_TERMINAL").ok();
    let (program, argv) = mac_terminal_command(app.as_deref(), path_str(&script)?, path_str(cwd)?);
    let label = mac_app(app.as_deref()).to_string();
    let mut cmd = Command::new(&program);
    cmd.args(&argv);
    let result = if program == "open" {
        // `open` hands the file to the app and returns at once, so waiting on
        // it is cheap and reports a missing app ("Unable to find application").
        cmd.stdin(Stdio::null())
            .output()
            .map_err(|e| e.to_string())
            .and_then(|o| {
                if o.status.success() {
                    Ok(())
                } else {
                    Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
                }
            })
    } else {
        spawn_detached(cmd).map_err(|e| e.to_string())
    };
    result.map_err(|e| {
        let _ = fs::remove_file(&script);
        anyhow::anyhow!("couldn't open {label}: {e}")
    })?;
    Ok(label)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_terminal(
    dir: &Path,
    name: &str,
    tool_path: &Path,
    args: &[String],
    cwd: &Path,
    prompt_file: Option<&Path>,
    prompt_flag: &str,
) -> Result<String> {
    let terminal = linux_terminal().context(
        "couldn't find a terminal to open: set $TERMINAL or CROSSTHREADS_TERMINAL, or install one (e.g. xterm)",
    )?;
    let script = dir.join(format!("{name}.sh"));
    let text = unix_script_with(tool_path, args, cwd, prompt_file, &script, prompt_flag)?;
    write_private(&script, text.as_bytes(), true)?;
    let mut cmd = Command::new(&terminal);
    cmd.args(linux_terminal_args(&terminal, path_str(&script)?))
        .current_dir(cwd);
    let label = terminal
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "a terminal".into());
    spawn_detached(cmd).map_err(|e| {
        let _ = fs::remove_file(&script);
        anyhow::anyhow!("couldn't start {label}: {e}")
    })?;
    Ok(label)
}

#[cfg(windows)]
fn open_terminal(
    dir: &Path,
    name: &str,
    tool_path: &Path,
    args: &[String],
    cwd: &Path,
    prompt_file: Option<&Path>,
    prompt_flag: &str,
) -> Result<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    let invoke = windows_invoke(tool_path);
    let script = dir.join(format!("{name}.ps1"));
    let text = windows_script_with(&invoke, args, cwd, prompt_file, &script, prompt_flag)?;
    // A BOM so Windows PowerShell 5.1 reads non-ASCII paths as UTF-8.
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(text.as_bytes());
    write_private(&script, &bytes, true)?;
    let script_s = path_str(&script)?;
    let ps_args = [
        "-NoLogo",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-NoExit",
        "-File",
    ];

    let result = if let Some(wt) = find_on_path("wt") {
        // Windows Terminal splits its command line on `;` unless escaped.
        let esc = |s: &str| s.replace(';', "\\;");
        let mut cmd = Command::new(wt);
        cmd.args(["-w", "new", "-d"])
            .arg(esc(path_str(cwd)?))
            .arg("powershell.exe")
            .args(ps_args)
            .arg(esc(script_s))
            .creation_flags(CREATE_NO_WINDOW);
        spawn_detached(cmd).map(|_| "Windows Terminal".to_string())
    } else {
        // No cmd.exe in between: PowerShell gets its own new console window.
        let mut cmd = Command::new("powershell.exe");
        cmd.args(ps_args)
            .arg(script_s)
            .current_dir(cwd)
            .creation_flags(CREATE_NEW_CONSOLE);
        spawn_detached(cmd).map(|_| "PowerShell".to_string())
    };
    result.map_err(|e| {
        let _ = fs::remove_file(&script);
        anyhow::anyhow!("couldn't open a terminal: {e}")
    })
}

/// See through an npm `.cmd` shim to `node <entry.js>` when we can.
#[cfg(windows)]
fn windows_invoke(tool_path: &Path) -> WinInvoke {
    let ext = tool_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if !matches!(ext.as_deref(), Some("cmd") | Some("bat")) {
        return WinInvoke::Exe(tool_path.to_path_buf());
    }
    let dir = tool_path.parent().unwrap_or(Path::new("."));
    let script = fs::read_to_string(tool_path)
        .ok()
        .and_then(|t| parse_npm_shim(&t, dir))
        .filter(|s| s.is_file());
    let node = Some(dir.join("node.exe"))
        .filter(|p| p.is_file())
        .or_else(|| find_on_path("node"))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")));
    match (script, node) {
        (Some(script), Some(node)) => WinInvoke::Node { node, script },
        _ => WinInvoke::Batch(tool_path.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVIL: &str = "\"; rm -rf ~; \" $(touch /tmp/pwned) `id` '; exit; '";

    #[test]
    fn resume_ids() {
        for ok in [
            "abc-123",
            "1e2d3c4b-5a6f-7089-9abc-def012345678",
            "a.b:c_d",
            &"x".repeat(128),
        ] {
            assert!(validate_resume_id(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            &"x".repeat(129),
            "a b",
            "a;b",
            "$(id)",
            "a'b",
            "a/b",
            "-rf",
            "--help",
            "a\nb",
            "é",
        ] {
            assert!(validate_resume_id(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn cwd_must_be_an_existing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a b 'c' $(x)");
        fs::create_dir(&sub).unwrap();
        let canon = validate_cwd(&sub).unwrap();
        assert!(canon.is_absolute() && canon.is_dir());
        assert_eq!(canon, strip_verbatim(fs::canonicalize(&sub).unwrap()));

        let file = dir.path().join("f.txt");
        fs::write(&file, "x").unwrap();
        assert!(validate_cwd(&file).is_err());
        assert!(validate_cwd(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn verbatim_paths_are_stripped() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\Users\me")),
            PathBuf::from(r"C:\Users\me")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\srv\share")),
            PathBuf::from(r"\\srv\share")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from("/home/me")),
            PathBuf::from("/home/me")
        );
    }

    #[test]
    fn tool_args_reuse_resume_knowledge() {
        assert_eq!(tool_args(Tool::ClaudeCode, None, true), (vec![], true));
        assert_eq!(
            tool_args(Tool::ClaudeCode, Some("abc"), true),
            (vec!["--resume".to_string(), "abc".to_string()], true)
        );
        assert_eq!(
            tool_args(Tool::Codex, Some("abc"), true),
            (vec!["resume".to_string(), "abc".to_string()], false)
        );
        assert_eq!(tool_args(Tool::Codex, None, true), (vec![], true));
        assert_eq!(tool_args(Tool::Codex, None, false), (vec![], false));
        assert_eq!(Tool::from_id("claude-code"), Some(Tool::ClaudeCode));
        assert_eq!(Tool::from_id("codex"), Some(Tool::Codex));
        assert_eq!(Tool::from_id("cursor"), None);
    }

    #[test]
    fn empty_prompts_are_dropped() {
        assert_eq!(effective_prompt(Some("  \n")), None);
        assert_eq!(effective_prompt(None), None);
        assert_eq!(effective_prompt(Some("hi")), Some("hi"));
    }

    #[test]
    fn sh_quoting() {
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote("$(x)"), "'$(x)'");
        assert_eq!(sh_quote(""), "''");
    }

    /// Run `text` (which must be a POSIX script) with `sh -n` to check syntax.
    #[cfg(unix)]
    fn sh_syntax_ok(text: &str) -> bool {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.sh");
        fs::write(&p, text).unwrap();
        Command::new("sh")
            .arg("-n")
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success())
    }

    #[test]
    fn unix_script_quotes_everything_and_never_contains_the_prompt() {
        let tool = Path::new("/opt/it's here/bin/claude");
        let cwd = Path::new("/home/me/My \"Project\" $(rm -rf ~) 'x'");
        let pf = Path::new("/data/cross threads/launch/ab'cd.prompt");
        let sp = Path::new("/data/cross threads/launch/ab'cd.sh");
        let args = vec!["--resume".to_string(), "abc-123".to_string()];
        let s = unix_script(tool, &args, cwd, Some(pf), sp).unwrap();

        assert!(s.starts_with("#!/bin/sh\n"));
        assert!(!s.contains("rm -rf ~;"));
        assert!(!s.contains(EVIL));
        assert!(
            s.contains(r#"p="$(cat -- '/data/cross threads/launch/ab'\''cd.prompt')" || exit 1"#)
        );
        assert!(s.contains(
            r#"rm -f -- '/data/cross threads/launch/ab'\''cd.prompt' '/data/cross threads/launch/ab'\''cd.sh'"#
        ));
        assert!(s.contains(r#"PATH='/opt/it'\''s here/bin':"$PATH"; export PATH"#));
        assert!(s.contains(
            r#"cd -- '/home/me/My "Project" $(rm -rf ~) '\''x'\''' && exec '/opt/it'\''s here/bin/claude' '--resume' 'abc-123' -- "$p""#
        ));
        #[cfg(unix)]
        assert!(sh_syntax_ok(&s));
    }

    #[test]
    fn unix_script_without_prompt() {
        let s = unix_script(
            Path::new("/usr/local/bin/codex"),
            &["resume".to_string(), "u-1".to_string()],
            Path::new("/w"),
            None,
            Path::new("/d/x.sh"),
        )
        .unwrap();
        assert!(!s.contains("$p"));
        assert!(!s.contains("cat"));
        assert!(s.contains("cd -- '/w' && exec '/usr/local/bin/codex' 'resume' 'u-1'\n"));
    }

    /// End to end on the POSIX side: run the generated script with a fake tool
    /// and a hostile prompt; the tool must receive it as one exact argument and
    /// nothing in it may run.
    #[cfg(unix)]
    #[test]
    fn unix_script_passes_a_hostile_prompt_as_one_argument() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("it's a \"dir\" $(x)");
        fs::create_dir(&root).unwrap();
        let canary = root.join("pwned");
        let prompt = format!(
            "-x first line\n\"; touch '{c}'; \"\n$(touch '{c}')\n`touch '{c}'`\n'; touch '{c}'; '",
            c = canary.display()
        );
        let out = root.join("out");
        let tool = root.join("tool");
        fs::write(
            &tool,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$#\" > {o}\npwd >> {o}\nfor a in \"$@\"; do printf '[%s]' \"$a\" >> {o}; done\n",
                o = sh_quote(out.to_str().unwrap())
            ),
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        let pf = root.join("p.prompt");
        fs::write(&pf, &prompt).unwrap();
        let sp = root.join("l.sh");
        let s = unix_script(
            &tool,
            &["--resume".into(), "id-1".into()],
            &root,
            Some(&pf),
            &sp,
        )
        .unwrap();
        assert!(!s.contains("touch"));
        fs::write(&sp, &s).unwrap();
        let st = Command::new("sh").arg(&sp).status().unwrap();
        assert!(st.success());

        let got = fs::read_to_string(&out).unwrap();
        let mut lines = got.splitn(3, '\n');
        assert_eq!(lines.next(), Some("4"));
        assert_eq!(
            fs::canonicalize(lines.next().unwrap()).unwrap(),
            fs::canonicalize(&root).unwrap()
        );
        assert_eq!(
            lines.next().unwrap(),
            format!("[--resume][id-1][--][{prompt}]")
        );
        assert!(!canary.exists(), "the prompt ran as code");
        assert!(
            !pf.exists() && !sp.exists(),
            "launcher files are cleaned up"
        );
    }

    #[test]
    fn ps_quoting() {
        assert_eq!(ps_quote("a b"), "'a b'");
        assert_eq!(ps_quote("it's"), "'it''s'");
        assert_eq!(ps_quote("it\u{2019}s"), "'it\u{2019}\u{2019}s'");
        assert_eq!(ps_quote("$(x) `n \"y\""), "'$(x) `n \"y\"'");
    }

    #[test]
    fn windows_script_reads_the_prompt_from_the_file() {
        let s = windows_script(
            &WinInvoke::Exe(PathBuf::from(r"C:\Users\O'Brien\.local\bin\claude.exe")),
            &["--resume".into(), "abc".into()],
            Path::new(r"C:\Users\O'Brien\My $(Project)"),
            Some(Path::new(
                r"C:\Users\O'Brien\AppData\Local\crossthreads\launch\ab.prompt",
            )),
            Path::new(r"C:\Users\O'Brien\AppData\Local\crossthreads\launch\ab.ps1"),
        )
        .unwrap();
        assert!(s.contains(
            r"$promptFile = 'C:\Users\O''Brien\AppData\Local\crossthreads\launch\ab.prompt'"
        ));
        assert!(s.contains("$prompt = Get-Content -LiteralPath $promptFile -Raw -Encoding UTF8"));
        assert!(s.contains(r"Set-Location -LiteralPath 'C:\Users\O''Brien\My $(Project)'"));
        assert!(s.contains(r"$env:Path = 'C:\Users\O''Brien\.local\bin' + ';' + $env:Path"));
        assert!(s.contains("PSVersion -lt [version]'7.3'"));
        assert!(s.ends_with(
            "& 'C:\\Users\\O''Brien\\.local\\bin\\claude.exe' '--resume' 'abc' '--' $prompt\r\n"
        ));
        assert!(!s.contains("Invoke-Expression") && !s.contains("iex "));
        assert!(!s.contains(EVIL));
    }

    #[test]
    fn windows_script_through_node_and_batch() {
        let node = windows_script(
            &WinInvoke::Node {
                node: PathBuf::from(r"C:\nodejs\node.exe"),
                script: PathBuf::from(r"C:\npm\node_modules\@openai\codex\bin\codex.js"),
            },
            &[],
            Path::new(r"C:\w"),
            None,
            Path::new(r"C:\l\x.ps1"),
        )
        .unwrap();
        assert!(node.ends_with(
            "& 'C:\\nodejs\\node.exe' 'C:\\npm\\node_modules\\@openai\\codex\\bin\\codex.js'\r\n"
        ));
        assert!(!node.contains("$prompt"));

        let batch = windows_script(
            &WinInvoke::Batch(PathBuf::from(r"C:\npm\claude.cmd")),
            &[],
            Path::new(r"C:\w"),
            Some(Path::new(r"C:\l\x.prompt")),
            Path::new(r"C:\l\x.ps1"),
        )
        .unwrap();
        assert!(batch.contains("-replace '[\\r\\n]+', ' '"));
        assert!(batch.contains("-replace '%', [string][char]0xFF05"));
        assert!(batch.ends_with("& 'C:\\npm\\claude.cmd' '--' $prompt\r\n"));
    }

    #[test]
    fn npm_shims_are_seen_through() {
        let shim = "@ECHO off\r\nGOTO start\r\n:start\r\nSETLOCAL\r\n\
            endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\r\n";
        let dir = Path::new("/npm");
        assert_eq!(
            parse_npm_shim(shim, dir),
            Some(
                dir.join("node_modules")
                    .join("@anthropic-ai")
                    .join("claude-code")
                    .join("cli.js")
            )
        );
        assert_eq!(parse_npm_shim("@echo off\r\nfoo.exe %*", dir), None);
        assert_eq!(parse_npm_shim("\"%dp0%\\..\\evil.js\"", dir), None);
        assert_eq!(parse_npm_shim("\"%dp0%\\x.exe\"", dir), None);
    }

    #[test]
    fn npmrc_prefix() {
        let home = Path::new("/home/me");
        assert_eq!(
            parse_npmrc_prefix("foo=bar\nprefix = ~/.npm-global\n", home),
            Some(PathBuf::from("/home/me/.npm-global"))
        );
        assert_eq!(
            parse_npmrc_prefix("prefix=\"/opt/npm\"", home),
            Some(PathBuf::from("/opt/npm"))
        );
        assert_eq!(parse_npmrc_prefix("registry=x", home), None);
    }

    #[test]
    fn linux_terminal_flags() {
        let s = "/d/x.sh";
        let a = |t: &str| linux_terminal_args(Path::new(t), s);
        assert_eq!(a("/usr/bin/gnome-terminal"), ["--", s]);
        assert_eq!(a("/usr/bin/konsole"), ["-e", s]);
        assert_eq!(a("/usr/bin/kitty"), [s]);
        assert_eq!(a("/usr/bin/alacritty"), ["-e", s]);
        assert_eq!(a("/usr/bin/wezterm"), ["start", "--", s]);
        assert_eq!(a("/usr/bin/xterm"), ["-e", s]);
        assert_eq!(a("/usr/bin/x-terminal-emulator"), ["-e", s]);
        assert_eq!(a("/usr/bin/xfce4-terminal"), ["-x", s]);
    }

    #[test]
    fn mac_terminal_choices() {
        let s = "/Users/me/Library/Application Support/crossthreads/launch/a.command";
        let (p, a) = mac_terminal_command(None, s, "/w");
        assert_eq!(
            (p.as_str(), a),
            ("open", vec!["-a".to_string(), "Terminal".into(), s.into()])
        );
        let (p, a) = mac_terminal_command(Some("iTerm"), s, "/w");
        assert_eq!(
            (p.as_str(), a),
            ("open", vec!["-a".to_string(), "iTerm".into(), s.into()])
        );
        let (p, a) = mac_terminal_command(Some("Ghostty"), s, "/w");
        assert_eq!(p, "open");
        assert_eq!(a, ["-na", "Ghostty", "--args", "-e", s]);
        let (p, a) = mac_terminal_command(Some("WezTerm"), s, "/w");
        assert!(p.ends_with("/wezterm"));
        assert_eq!(a, ["start", "--cwd", "/w", "--", s]);
        let (p, a) = mac_terminal_command(Some("Warp"), s, "/w");
        assert_eq!(
            (p.as_str(), a),
            ("open", vec!["-a".to_string(), "Warp".into(), s.into()])
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.prompt");
        write_private(&p, EVIL.as_bytes(), false).unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_to_string(&p).unwrap(), EVIL);
        // Never overwrites.
        assert!(write_private(&p, b"x", false).is_err());
        let sp = dir.path().join("x.sh");
        write_private(&sp, b"#!/bin/sh\n", true).unwrap();
        assert_eq!(
            fs::metadata(&sp).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn sweep_keeps_fresh_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("fresh.prompt");
        fs::write(&p, "x").unwrap();
        sweep(dir.path());
        assert!(p.exists());
    }

    #[test]
    fn random_names_differ() {
        let a = random_name();
        assert_eq!(a.len(), 32);
        assert_ne!(a, random_name());
    }

    #[test]
    fn grok_takes_its_prompt_as_one_flag_argument() {
        assert_eq!(Tool::from_id("grok-build"), Some(Tool::Grok));
        assert_eq!(tool_args(Tool::Grok, None, true), (vec![], true));
        assert_eq!(
            tool_args(Tool::Grok, Some("s-1"), true),
            (vec!["--resume".to_string(), "s-1".to_string()], false)
        );
        let s = unix_script_with(
            Path::new("/usr/local/bin/grok"),
            &[],
            Path::new("/w"),
            Some(Path::new("/l/x.prompt")),
            Path::new("/l/x.sh"),
            Tool::Grok.prompt_flag(),
        )
        .unwrap();
        assert!(
            s.contains(r#"exec '/usr/local/bin/grok' '--prompt='"$p""#),
            "{s}"
        );
        #[cfg(unix)]
        assert!(sh_syntax_ok(&s));
        let w = windows_script_with(
            &WinInvoke::Exe(PathBuf::from(r"C:\bin\grok.exe")),
            &[],
            Path::new(r"C:\w"),
            Some(Path::new(r"C:\l\x.prompt")),
            Path::new(r"C:\l\x.ps1"),
            Tool::Grok.prompt_flag(),
        )
        .unwrap();
        assert!(w.contains("('--prompt=' + $prompt)"), "{w}");
    }
}
