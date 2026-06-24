//! `crossthreads llm-auth` — show which model credentials Crossthreads can use
//! for optional LLM features, across providers. Prints only *which* method was
//! found per provider, never the secret itself.

use std::process::ExitCode;

use anyhow::Result;
use ct_llm::Provider;

pub fn run(_args: &[String]) -> Result<ExitCode> {
    let mut any = false;
    for provider in [Provider::Anthropic, Provider::OpenAi] {
        let creds = ct_llm::resolve(provider);
        if creds.is_empty() {
            println!("{}: no credentials found", provider.label());
            continue;
        }
        any = true;
        println!("{} — usable (in order of preference):", provider.label());
        for (i, c) in creds.iter().enumerate() {
            println!("  {}. {}", i + 1, c.describe());
        }
    }
    println!();
    if any {
        let order = ct_llm::available_providers()
            .iter()
            .map(|p| p.label())
            .collect::<Vec<_>>()
            .join(", then ");
        println!("LLM features (e.g. `crossthreads themes --name`) will try: {order}.");
        println!("Force one with CROSSTHREADS_LLM_PROVIDER=anthropic|openai.");
    } else {
        println!(
            "No LLM auth found. Set ANTHROPIC_API_KEY or OPENAI_API_KEY, sign into Claude Code or \
             Codex, or install one of their CLIs — then re-run."
        );
    }
    Ok(ExitCode::SUCCESS)
}
