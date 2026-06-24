//! LLM-synthesis insights over the indexed history — open loops, knowledge
//! cards, a decision log, a "how I work" profile, and a digest. Each gathers
//! recent conversation text and asks the configured model ([`ct_llm`]) to
//! synthesize it into Markdown, returning the source conversation ids it drew
//! from. Opt-in: needs a model login (Settings → Models). The model call runs
//! *without* the store lock held.

use anyhow::{bail, Result};
use ct_store::{RecentConversation, Theme};

/// A kind of insight to synthesize.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    OpenLoops,
    KnowledgeCards,
    DecisionLog,
    HowIWork,
    Digest,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s.trim().to_lowercase().replace('-', "_").as_str() {
            "open_loops" | "loops" => Some(Kind::OpenLoops),
            "knowledge_cards" | "cards" => Some(Kind::KnowledgeCards),
            "decision_log" | "decisions" | "adr" => Some(Kind::DecisionLog),
            "how_i_work" | "profile" => Some(Kind::HowIWork),
            "digest" | "weekly" => Some(Kind::Digest),
            _ => None,
        }
    }

    /// Default number of recent conversations to read.
    pub fn default_limit(self) -> usize {
        match self {
            Kind::OpenLoops | Kind::Digest => 40,
            Kind::KnowledgeCards | Kind::DecisionLog | Kind::HowIWork => 80,
        }
    }

    fn system(self) -> &'static str {
        match self {
            Kind::OpenLoops => {
                "You analyze a developer's recent AI coding sessions and surface unresolved work."
            }
            Kind::KnowledgeCards => {
                "You distill durable, reusable knowledge from a developer's coding sessions."
            }
            Kind::DecisionLog => {
                "You extract decisions and their rationale from a developer's coding sessions."
            }
            Kind::HowIWork => {
                "You infer a developer's working conventions from their coding sessions."
            }
            Kind::Digest => {
                "You write a brief, useful reflective digest of a developer's recent work."
            }
        }
    }

    fn instruction(self) -> &'static str {
        match self {
            Kind::OpenLoops => {
                "List the OPEN LOOPS across these sessions: unanswered questions, half-finished \
                 tasks, \"I'll come back to this\" notes, dangling TODOs, and errors that were \
                 never confirmed fixed. Output a Markdown bullet list; each item is a short bold \
                 title, one line of context, and the session title it came from. Include only \
                 genuinely unresolved items — if a thread reached a clear resolution, skip it. If \
                 nothing is open, say so."
            }
            Kind::KnowledgeCards => {
                "Write up to 8 KNOWLEDGE CARDS for things solved or decided here that are worth \
                 remembering. Each card: a bold one-line title (the question/topic) then 1–3 \
                 sentences of the canonical answer. Prefer things that recur across sessions. \
                 Markdown."
            }
            Kind::DecisionLog => {
                "Extract notable DECISIONS in the form \"Chose X over Y because Z\". One Markdown \
                 bullet each, with the rationale and (if clear) the session it came from. Skip \
                 trivial choices."
            }
            Kind::HowIWork => {
                "Infer this developer's working conventions — tools, languages, code style, commit \
                 habits, recurring gotchas, and preferences. Output a concise Markdown list \
                 suitable to drop into a CLAUDE.md / AGENTS.md so a new agent instantly knows how \
                 they operate. Only include things actually evidenced in the sessions."
            }
            Kind::Digest => {
                "Write a short DIGEST with three sections: (1) what they've been working on, (2) \
                 3 open questions or unfinished threads, (3) 2 things worth revisiting. Markdown, \
                 concise."
            }
        }
    }
}

/// Synthesize an insight from already-gathered conversations (no store lock).
pub fn synthesize(convos: &[RecentConversation], kind: Kind) -> Result<(String, Vec<String>)> {
    if convos.is_empty() {
        bail!("no conversations indexed yet — run `crossthreads index` first");
    }
    let mut corpus = String::new();
    let mut sources = Vec::with_capacity(convos.len());
    for c in convos {
        sources.push(c.id.clone());
        let title = c.title.as_deref().unwrap_or("(untitled)");
        let date = c
            .started_at
            .as_deref()
            .map(|s| s.get(..10).unwrap_or(s))
            .unwrap_or("");
        corpus.push_str(&format!(
            "\n### [{}] {title} ({}, {date})\n{}\n",
            c.tool,
            c.project.as_deref().unwrap_or("-"),
            c.text.trim()
        ));
        // Keep the prompt bounded regardless of how many sessions were asked for.
        if corpus.len() > 60_000 {
            break;
        }
    }
    let user = format!("{}\n\n--- SESSIONS ---\n{corpus}", kind.instruction());
    let markdown = ct_llm::complete_long(kind.system(), &user)?;
    Ok((markdown, sources))
}

/// Generate a short, human label for a theme cluster from its sample titles and
/// tool mix (feature: LLM-named themes). Best-effort: returns `None` if no model
/// is available or the call fails, so the caller keeps the keyword label.
pub fn name_theme(theme: &Theme) -> Option<String> {
    let mut titles = String::new();
    for s in theme.samples.iter().take(8) {
        if let Some(t) = &s.title {
            titles.push_str("- ");
            titles.push_str(t.trim());
            titles.push('\n');
        }
    }
    if titles.is_empty() {
        return None;
    }
    let user = format!(
        "These AI coding-session titles all belong to one cluster. Give it a \
         short, specific name (2–4 words, Title Case, no quotes or trailing \
         punctuation):\n\n{titles}"
    );
    let raw = ct_llm::complete(
        "You name clusters of developer sessions with a short, specific title.",
        &user,
    )
    .ok()?;
    // Models sometimes wrap the answer in quotes or add a trailing period.
    let name = raw
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '.' || c == '*')
        .trim()
        .to_string();
    if name.is_empty() || name.len() > 60 {
        None
    } else {
        Some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_aliases_and_normalizes() {
        assert!(matches!(Kind::parse("open_loops"), Some(Kind::OpenLoops)));
        assert!(matches!(Kind::parse("loops"), Some(Kind::OpenLoops)));
        assert!(matches!(Kind::parse("open-loops"), Some(Kind::OpenLoops)));
        assert!(matches!(Kind::parse("  Loops "), Some(Kind::OpenLoops)));
        assert!(matches!(Kind::parse("cards"), Some(Kind::KnowledgeCards)));
        assert!(matches!(Kind::parse("adr"), Some(Kind::DecisionLog)));
        assert!(matches!(Kind::parse("decisions"), Some(Kind::DecisionLog)));
        assert!(matches!(Kind::parse("profile"), Some(Kind::HowIWork)));
        assert!(matches!(Kind::parse("weekly"), Some(Kind::Digest)));
        assert!(Kind::parse("nonsense").is_none());
    }

    #[test]
    fn synthesize_bails_on_empty_corpus() {
        let err = synthesize(&[], Kind::OpenLoops).unwrap_err();
        assert!(err.to_string().contains("no conversations"));
    }

    #[test]
    fn default_limits_are_sane() {
        assert!(Kind::OpenLoops.default_limit() > 0);
        assert!(Kind::HowIWork.default_limit() >= Kind::Digest.default_limit());
    }
}
