//! Skills/prompts connectors: Claude Code SKILL.md + Codex prompts.

use ct_connectors::connectors::skills::{parse_skill_md, CLAUDE_SKILL_FINGERPRINT};
use ct_connectors::connectors::{ClaudeSkillsConnector, CodexPromptsConnector};
use ct_core::connector::Connector;
use ct_core::model::{Kind, Role};
use std::path::Path;

#[test]
fn parses_skill_md_frontmatter_and_body() {
    let text = "---\nname: pdf-extractor\ndescription: Extract text and tables from PDF files\n---\n\n# Steps\n\nUse pdfplumber, then clean the output.\n";
    let convo = parse_skill_md(Path::new("/x/pdf-extractor/SKILL.md"), text);

    assert_eq!(convo.kind, Kind::Skill);
    assert_eq!(convo.title.as_deref(), Some("pdf-extractor"));
    assert_eq!(convo.tool.slug(), "claude-code");
    assert_eq!(convo.source.fingerprint, CLAUDE_SKILL_FINGERPRINT);
    assert_eq!(convo.messages.len(), 1);
    assert_eq!(convo.messages[0].role, Role::System);
    // description + body are both searchable.
    assert!(convo.messages[0]
        .content
        .contains("Extract text and tables"));
    assert!(convo.messages[0].content.contains("pdfplumber"));
    // derived_title uses the explicit name.
    assert_eq!(convo.derived_title(), "pdf-extractor");
}

#[test]
fn skill_without_frontmatter_falls_back_to_dir_name() {
    let convo = parse_skill_md(
        Path::new("/x/my-skill/SKILL.md"),
        "just instructions, no frontmatter",
    );
    assert_eq!(convo.title.as_deref(), Some("my-skill"));
    assert!(convo.messages[0].content.contains("just instructions"));
}

#[test]
fn discovers_claude_skills_under_config_dir() {
    let dir = tempfile::tempdir().unwrap();
    let skill = dir.path().join("skills/pdf-extractor");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: pdf-extractor\ndescription: PDFs\n---\nbody",
    )
    .unwrap();
    std::env::set_var("CLAUDE_CONFIG_DIR", dir.path());

    let c = ClaudeSkillsConnector::new();
    assert!(c.detect());
    let sessions = c.discover().unwrap();
    assert_eq!(sessions.len(), 1);
    let convo = c.parse(&sessions[0]).unwrap();
    assert_eq!(convo.kind, Kind::Skill);
    assert_eq!(convo.title.as_deref(), Some("pdf-extractor"));

    std::env::remove_var("CLAUDE_CONFIG_DIR");
}

#[test]
fn discovers_codex_prompts_under_codex_home() {
    let dir = tempfile::tempdir().unwrap();
    let prompts = dir.path().join("prompts");
    std::fs::create_dir_all(&prompts).unwrap();
    std::fs::write(
        prompts.join("refactor.md"),
        "Refactor the selected code for clarity.",
    )
    .unwrap();
    std::env::set_var("CODEX_HOME", dir.path());

    let c = CodexPromptsConnector::new();
    assert!(c.detect());
    let sessions = c.discover().unwrap();
    assert_eq!(sessions.len(), 1);
    let convo = c.parse(&sessions[0]).unwrap();
    assert_eq!(convo.kind, Kind::Skill);
    assert_eq!(convo.tool.slug(), "codex");
    assert_eq!(convo.title.as_deref(), Some("refactor"));
    assert!(convo.messages[0]
        .content
        .contains("Refactor the selected code"));

    std::env::remove_var("CODEX_HOME");
}
