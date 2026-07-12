//! `crossthreads resume <ID>` — how to pick a conversation back up in its native
//! tool. Prints a runnable resume command where one exists (e.g. Claude Code's
//! `claude --resume <id>`), otherwise the source transcript to open. `--run`
//! executes the resume command directly. IDs come from `search`/`context`.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use anyhow::{bail, Result};
use ct_daemon::resume::{resume_for, Action};
use ct_store::Store;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let mut db: Option<PathBuf> = None;
    let mut id: Option<String> = None;
    let mut do_run = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--run" => do_run = true,
            "--db" => {
                db = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--db needs a value"))?,
                ))
            }
            other if other.starts_with('-') => bail!("unknown option for `resume`: {other}"),
            other => id = Some(other.to_string()),
        }
    }

    let Some(id) = id else {
        bail!("usage: crossthreads resume <ID> [--run]");
    };

    let store = Store::open(crate::resolve_db(db)?)?;
    let Some(conv) = store
        .get_conversation(&id)?
        .filter(|c| !c.source_path.is_empty())
    else {
        eprintln!("no conversation with id {id} (or it has no source path)");
        return Ok(ExitCode::FAILURE);
    };

    let r = resume_for(&conv.tool, &conv.source_path, conv.project.as_deref());
    match r.action {
        Action::Resume {
            program,
            args,
            cwd,
            display,
        } => {
            if do_run {
                let mut cmd = Command::new(&program);
                cmd.args(&args);
                if let Some(dir) = &cwd {
                    cmd.current_dir(dir);
                }
                let status = cmd.status();
                return match status {
                    Ok(s) if s.success() => Ok(ExitCode::SUCCESS),
                    Ok(s) => Ok(ExitCode::from(s.code().unwrap_or(1) as u8)),
                    Err(e) => {
                        eprintln!("could not run `{program}`: {e}");
                        Ok(ExitCode::FAILURE)
                    }
                };
            }
            println!("{display}");
        }
        Action::Reveal { path } => {
            if do_run {
                eprintln!("no resume command for {} — open the transcript:", r.tool);
            }
            println!("{path}");
        }
    }
    Ok(ExitCode::SUCCESS)
}
