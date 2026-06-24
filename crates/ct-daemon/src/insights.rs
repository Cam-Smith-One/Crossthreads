//! LLM-synthesis insights over the indexed history — open loops, knowledge
//! cards, a decision log, a "how I work" profile, and a digest. Each gathers
//! recent conversation text and asks the configured model ([`ct_llm`]) to
//! synthesize it into Markdown, returning the source conversation ids it drew
//! from. Opt-in: needs a model login (Settings → Models). The model call runs
//! *without* the store lock held.

use anyhow::{bail, Result};
use ct_store::{behavior, Filters, RecentConversation, Store, StoredConversation, Theme};

/// A kind of insight to synthesize.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    OpenLoops,
    KnowledgeCards,
    DecisionLog,
    HowIWork,
    Digest,
    Recurrence,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s.trim().to_lowercase().replace('-', "_").as_str() {
            "open_loops" | "loops" => Some(Kind::OpenLoops),
            "knowledge_cards" | "cards" => Some(Kind::KnowledgeCards),
            "decision_log" | "decisions" | "adr" => Some(Kind::DecisionLog),
            "how_i_work" | "profile" => Some(Kind::HowIWork),
            "digest" | "weekly" => Some(Kind::Digest),
            "recurrence" | "recurring" | "patterns" | "alerts" => Some(Kind::Recurrence),
            _ => None,
        }
    }

    /// Default number of recent records (threads + skills, all tools) to read.
    /// Generous by default so insights span the whole history; the corpus is
    /// still byte-bounded in [`synthesize`], and callers can override the limit.
    pub fn default_limit(self) -> usize {
        match self {
            // Open loops / digest lean recent, but still cast a wide net.
            Kind::OpenLoops | Kind::Digest => 200,
            // Cards / decisions / how-I-work / recurrence benefit from breadth.
            Kind::KnowledgeCards | Kind::DecisionLog | Kind::HowIWork | Kind::Recurrence => 400,
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
            Kind::Recurrence => {
                "You spot recurring problems and patterns across a developer's coding sessions."
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
            Kind::Recurrence => {
                "Identify RECURRING problems, questions, and patterns — things this developer \
                 keeps running into across multiple sessions (the same bug class, the same setup \
                 friction, a question re-asked, a workaround re-derived). For each, a bold title, \
                 how many times / where it recurs, and a one-line suggestion to break the loop \
                 (e.g. a note for CLAUDE.md, a script, a fix). Order by how often it recurs. Only \
                 include things that genuinely appear more than once. If nothing clearly recurs, \
                 say so. Markdown."
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
        // Tag skills explicitly so the model can weigh them appropriately.
        let kind = if c.kind == "skill" { " · skill" } else { "" };
        corpus.push_str(&format!(
            "\n### [{}{kind}] {title} ({}, {date})\n{}\n",
            c.tool,
            c.project.as_deref().unwrap_or("-"),
            c.text.trim()
        ));
        // Keep the prompt bounded regardless of how many records were asked for.
        // Generous budget so a large, multi-tool history still fits the model's
        // context (~75k tokens of input on the default models).
        if corpus.len() > 300_000 {
            break;
        }
    }
    let user = format!("{}\n\n--- SESSIONS ---\n{corpus}", kind.instruction());
    // Roomier output budget than a label call — insights synthesize a lot of
    // input into a structured Markdown answer.
    let markdown = ct_llm::complete_with(kind.system(), &user, 2048)?;
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

// --- Behavioral insights ("how you work") --------------------------------
//
// Unlike the text kinds above, these are grounded in the deterministic metrics
// layer ([`behavior`]): the daemon `gather`s data under the store lock into an
// owned [`BehavioralInput`], drops the lock, then `synthesize`s with the model.

/// A behavioral insight kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behavioral {
    /// A quantified profile of how the user works.
    WorkDna,
    /// Gaps, blind spots, and a draft CLAUDE.md.
    Gaps,
    /// Prompt coaching: smoothest vs roughest sessions, with rewrites.
    PromptCoach,
    /// Repeatable procedures mined into draft skills.
    ProcessMiner,
}

impl Behavioral {
    pub fn parse(s: &str) -> Option<Behavioral> {
        match s.trim().to_lowercase().replace('-', "_").as_str() {
            "work_dna" | "workdna" | "dna" | "profile_plus" => Some(Behavioral::WorkDna),
            "gaps" | "blind_spots" | "blindspots" => Some(Behavioral::Gaps),
            "prompt_coach" | "coach" | "prompts" => Some(Behavioral::PromptCoach),
            "process_miner" | "processes" | "process" | "repeatable" => {
                Some(Behavioral::ProcessMiner)
            }
            _ => None,
        }
    }
}

/// Owned data gathered for a behavioral insight (no store handle held).
pub struct BehavioralInput {
    kind: Behavioral,
    metrics_summary: String,
    corpus: String,
    sources: Vec<String>,
}

/// Build text from a fetched conversation, bounded to `max_chars`.
fn convo_text(c: &StoredConversation, max_chars: usize) -> String {
    let mut out = String::new();
    for m in &c.messages {
        if out.len() >= max_chars {
            break;
        }
        let remaining = max_chars - out.len();
        let snippet: String = m.content.chars().take(remaining).collect();
        out.push_str(&m.role);
        out.push_str(": ");
        out.push_str(snippet.trim());
        out.push('\n');
    }
    out
}

/// Concatenate recent-conversation samples into a bounded corpus + source ids.
fn samples_corpus(samples: &[RecentConversation], cap: usize) -> (String, Vec<String>) {
    let mut corpus = String::new();
    let mut sources = Vec::new();
    for c in samples {
        sources.push(c.id.clone());
        corpus.push_str(&format!(
            "\n### [{}] {}\n{}\n",
            c.tool,
            c.title.as_deref().unwrap_or("(untitled)"),
            c.text.trim()
        ));
        if corpus.len() > cap {
            break;
        }
    }
    (corpus, sources)
}

/// Gather the inputs for a behavioral insight. Holds the store only for queries
/// (the slow model call happens later in [`synthesize_behavioral`]).
pub fn gather_behavioral(store: &Store, kind: Behavioral) -> Result<BehavioralInput> {
    let (metrics, per) = store.work_metrics(&Filters::default())?;
    if metrics.sessions == 0 {
        bail!("no sessions indexed yet — run `crossthreads index` first");
    }
    let metrics_summary = behavior::summarize(&metrics);

    let (corpus, sources) = match kind {
        Behavioral::WorkDna | Behavioral::Gaps => {
            let samples = store.recent_conversations(50, 1200)?;
            samples_corpus(&samples, 32_000)
        }
        Behavioral::PromptCoach => {
            // Pick the roughest and the smoothest *resolved* sessions by friction.
            let mut rough = per.clone();
            rough.sort_by(|a, b| {
                b.friction
                    .partial_cmp(&a.friction)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut smooth: Vec<_> = per
                .iter()
                .filter(|p| p.resolved && p.user_turns >= 1)
                .cloned()
                .collect();
            smooth.sort_by(|a, b| {
                a.friction
                    .partial_cmp(&b.friction)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut corpus = String::from("## ROUGH SESSIONS (high friction)\n");
            let mut sources = Vec::new();
            for p in rough.iter().take(3) {
                if let Some(c) = store.get_conversation(&p.id)? {
                    sources.push(p.id.clone());
                    corpus.push_str(&format!(
                        "\n### {} ({} user turns, {} corrections)\n{}\n",
                        c.title.as_deref().unwrap_or("(untitled)"),
                        p.user_turns,
                        p.corrections,
                        convo_text(&c, 3500)
                    ));
                }
            }
            corpus.push_str("\n## SMOOTH SESSIONS (low friction)\n");
            for p in smooth.iter().take(3) {
                if let Some(c) = store.get_conversation(&p.id)? {
                    sources.push(p.id.clone());
                    corpus.push_str(&format!(
                        "\n### {} ({} user turns)\n{}\n",
                        c.title.as_deref().unwrap_or("(untitled)"),
                        p.user_turns,
                        convo_text(&c, 3500)
                    ));
                }
            }
            (corpus, sources)
        }
        Behavioral::ProcessMiner => {
            // Clusters of similar work = candidate repeatable procedures.
            let themes = store.themes(12, 4)?;
            let mut corpus = String::new();
            let mut sources = Vec::new();
            for t in themes.iter().filter(|t| t.size >= 3).take(8) {
                corpus.push_str(&format!(
                    "\n## Recurring area: {} ({} sessions)\n",
                    t.label, t.size
                ));
                for s in t.samples.iter().take(2) {
                    if let Some(c) = store.get_conversation(&s.id)? {
                        sources.push(s.id.clone());
                        corpus.push_str(&format!(
                            "### example: {}\n{}\n",
                            c.title.as_deref().unwrap_or("(untitled)"),
                            convo_text(&c, 3000)
                        ));
                    }
                }
                if corpus.len() > 60_000 {
                    break;
                }
            }
            (corpus, sources)
        }
    };

    Ok(BehavioralInput {
        kind,
        metrics_summary,
        corpus,
        sources,
    })
}

/// Synthesize a behavioral insight from already-gathered data (no store lock).
pub fn synthesize_behavioral(input: BehavioralInput) -> Result<(String, Vec<String>)> {
    let BehavioralInput {
        kind,
        metrics_summary,
        corpus,
        sources,
    } = input;

    let (system, user) = match kind {
        Behavioral::WorkDna => (
            "You profile how a developer works from hard metrics plus session samples. \
             Specific, evidenced, never generic.",
            format!(
                "Quantitative metrics on how this developer works, across their whole history:\n\n\
                 {metrics_summary}\n\n\
                 A sample of recent sessions for color:\n{corpus}\n\n\
                 Write their WORK DNA — a sharp, second-person profile grounded in the numbers \
                 above (quote the figures). Cover: their default rhythm (turns, tempo), where \
                 friction concentrates and the likely cause, how they open tasks (specificity), \
                 and 2–3 signature habits. Markdown, concise."
            ),
        ),
        Behavioral::Gaps => (
            "You find a developer's blind spots and gaps from their metrics and sessions, \
             then give concrete, kind, actionable fixes.",
            format!(
                "Metrics on how this developer works:\n\n{metrics_summary}\n\n\
                 Recent sessions:\n{corpus}\n\n\
                 Identify their GAPS & BLIND SPOTS: practices largely missing (e.g. low test or \
                 commit mention rates), work they start but rarely finish (high abandonment), and \
                 friction they keep absorbing. Cite the relevant figures. Then end with a short \
                 fenced ```markdown CLAUDE.md``` block of 4–8 personalized rules that would close \
                 the biggest gaps. Markdown."
            ),
        ),
        Behavioral::PromptCoach => (
            "You are a prompt-craft coach. You compare a developer's smoothest and roughest \
             sessions and teach them, kindly and concretely, using their own words.",
            format!(
                "Metrics:\n{metrics_summary}\n\n{corpus}\n\n\
                 Coach this developer on their PROMPTING. Contrast what made the smooth sessions \
                 flow vs what made the rough ones thrash — focus on *their opening message* and \
                 how they course-correct. Give 3–5 concrete, generalizable tips, each with a \
                 short before→after rewrite drawn from their actual prompts above. Markdown, \
                 encouraging."
            ),
        ),
        Behavioral::ProcessMiner => (
            "You turn a developer's repeated workflows into reusable skills. You output concrete, \
             installable artifacts, not vague advice.",
            format!(
                "Below are clusters of similar sessions — candidate repeatable procedures.\n{corpus}\n\n\
                 For the 2–4 strongest repeatable procedures, write a REUSABLE SKILL. For each: a \
                 short name, when to use it, and a fenced ```markdown SKILL.md``` block with a \
                 numbered procedure the developer (or their agent) can follow next time — so they \
                 stop re-deriving it. Only include procedures that genuinely recur in the samples."
            ),
        ),
    };

    if corpus.trim().is_empty() {
        bail!("not enough indexed history yet for this insight");
    }
    let markdown = ct_llm::complete_with(system, &user, 2048)?;
    Ok((markdown, sources))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behavioral_parse_aliases() {
        assert_eq!(Behavioral::parse("work_dna"), Some(Behavioral::WorkDna));
        assert_eq!(Behavioral::parse("coach"), Some(Behavioral::PromptCoach));
        assert_eq!(
            Behavioral::parse("processes"),
            Some(Behavioral::ProcessMiner)
        );
        assert_eq!(Behavioral::parse("blind-spots"), Some(Behavioral::Gaps));
        assert!(Behavioral::parse("nope").is_none());
    }

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
