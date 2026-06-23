//! `crossthreads codex-auth` — show which Codex/OpenAI credentials Crossthreads
//! can use for optional LLM features. Prints only *which* method was found, never
//! the secret itself.

use std::process::ExitCode;

use anyhow::Result;

pub fn run(_args: &[String]) -> Result<ExitCode> {
    if let Some(home) = ct_llm::auth::codex_home() {
        println!("codex home: {}", home.display());
    }
    let methods = ct_llm::resolve();
    if methods.is_empty() {
        println!("no Codex/OpenAI auth found.");
        println!(
            "  → set OPENAI_API_KEY, run `codex login`, or install the codex CLI, then re-run."
        );
        return Ok(ExitCode::SUCCESS);
    }
    println!("usable auth (in order of preference):");
    for (i, m) in methods.iter().enumerate() {
        println!("  {}. {}", i + 1, m.describe());
    }
    println!("\nLLM features (e.g. `crossthreads themes --name`) will use the first that works.");
    Ok(ExitCode::SUCCESS)
}
