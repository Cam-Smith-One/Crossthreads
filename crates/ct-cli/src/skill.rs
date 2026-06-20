//! `crossthreads skill install` — drop the Crossthreads agent skill into your
//! tools so they proactively recall prior work via the MCP server.
//!
//! - Claude Code: `<~/.claude>/skills/crossthreads/SKILL.md`
//!   (`CLAUDE_CONFIG_DIR` overrides `~/.claude`).
//! - Codex: `<~/.codex>/prompts/crossthreads.md`, invoked as `/crossthreads`
//!   (`CODEX_HOME` overrides `~/.codex`).
//!
//! The skill text is bundled into the binary, so this works from an installed
//! release with no repo checkout.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};

const CLAUDE_SKILL: &str = include_str!("../../../assets/skills/claude/SKILL.md");
const CODEX_PROMPT: &str = include_str!("../../../assets/skills/codex/crossthreads.md");

const USAGE: &str = "\
crossthreads skill — install the Crossthreads agent skill

USAGE:
    crossthreads skill install [--claude] [--codex] [--force]

By default installs for both tools that are present. Pass --claude or --codex to
limit to one. --force overwrites an existing file.

After installing, configure the Crossthreads MCP server in the tool and keep the
daemon running (`crossthreads-up`). In Claude Code the skill loads automatically;
in Codex, invoke it with `/crossthreads`.
";

pub fn run(args: &[String]) -> Result<ExitCode> {
    match args.first().map(String::as_str) {
        Some("install") => install(&args[1..]),
        Some("help") | Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Some(other) => {
            eprintln!("unknown skill command: {other}\n");
            print!("{USAGE}");
            Ok(ExitCode::FAILURE)
        }
    }
}

fn install(args: &[String]) -> Result<ExitCode> {
    let mut only_claude = false;
    let mut only_codex = false;
    let mut force = false;
    for a in args {
        match a.as_str() {
            "--claude" => only_claude = true,
            "--codex" => only_codex = true,
            "--force" | "-f" => force = true,
            other => anyhow::bail!("unknown option: {other}"),
        }
    }
    // With no tool flag, install for both.
    let (do_claude, do_codex) = if only_claude || only_codex {
        (only_claude, only_codex)
    } else {
        (true, true)
    };

    let mut wrote = 0;
    if do_claude {
        wrote += write_one(
            claude_skill_path()?,
            CLAUDE_SKILL,
            force,
            "Claude Code skill",
        )?;
    }
    if do_codex {
        wrote += write_one(codex_prompt_path()?, CODEX_PROMPT, force, "Codex prompt")?;
    }

    if wrote > 0 {
        println!(
            "\nNext: configure the Crossthreads MCP server in your tool and keep the\n\
             daemon running (`crossthreads-up`). Claude Code loads the skill\n\
             automatically; in Codex, invoke it with `/crossthreads`."
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// Write `content` to `path` (creating parents). Skips an existing file unless
/// `force`. Returns 1 if written, 0 if skipped.
fn write_one(path: PathBuf, content: &str, force: bool, label: &str) -> Result<u32> {
    if path.exists() && !force {
        println!(
            "• {label}: already present at {} (use --force to overwrite)",
            path.display()
        );
        return Ok(0);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    println!("✓ {label}: installed to {}", path.display());
    Ok(1)
}

fn claude_dir() -> Result<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
        .context("could not resolve the Claude config dir (set CLAUDE_CONFIG_DIR)")
}

fn codex_dir() -> Result<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")))
        .context("could not resolve the Codex home dir (set CODEX_HOME)")
}

fn claude_skill_path() -> Result<PathBuf> {
    Ok(claude_dir()?
        .join("skills")
        .join("crossthreads")
        .join("SKILL.md"))
}

fn codex_prompt_path() -> Result<PathBuf> {
    Ok(codex_dir()?.join("prompts").join("crossthreads.md"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_writes_both_files_under_overridden_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let claude = tmp.path().join("claude");
        let codex = tmp.path().join("codex");
        std::env::set_var("CLAUDE_CONFIG_DIR", &claude);
        std::env::set_var("CODEX_HOME", &codex);

        run(&["install".into()]).unwrap();

        let skill = claude.join("skills/crossthreads/SKILL.md");
        let prompt = codex.join("prompts/crossthreads.md");
        assert!(skill.exists(), "claude skill written");
        assert!(prompt.exists(), "codex prompt written");
        // The skill has valid frontmatter our own indexer can read.
        let text = std::fs::read_to_string(&skill).unwrap();
        assert!(text.contains("name: crossthreads"));
        assert!(text.contains("crossthreads_recall"));

        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::env::remove_var("CODEX_HOME");
    }
}
