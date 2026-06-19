//! Integration test: parse a realistic Claude Code JSONL fixture end-to-end.

use std::path::Path;

use ct_connectors::connectors::claude_code::parse_jsonl;
use ct_core::model::Role;

fn fixture() -> (std::path::PathBuf, String) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude_code_session.jsonl");
    let text = std::fs::read_to_string(&path).expect("read fixture");
    (path, text)
}

#[test]
fn parses_session_into_normalized_conversation() {
    let (path, text) = fixture();
    let convo = parse_jsonl(&path, &text).expect("parse");

    // Summary line carries no message and is skipped; 4 real turns remain.
    assert_eq!(convo.messages.len(), 4, "summary line should be skipped");

    // Conversation-level metadata is lifted from the events.
    assert_eq!(convo.project.as_deref(), Some("/home/dev/acme-api"));
    assert_eq!(convo.model.as_deref(), Some("claude-opus-4-8"));
    assert_eq!(convo.git_context.branch.as_deref(), Some("fix/oauth"));
    assert_eq!(convo.tool.slug(), "claude-code");
    assert_eq!(convo.source.fingerprint, "claude-code/v1");

    // Timestamps span the session.
    assert!(convo.started_at.unwrap() < convo.ended_at.unwrap());

    // Roles: user, assistant, tool (reclassified from a tool_result), assistant.
    let roles: Vec<Role> = convo.messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![Role::User, Role::Assistant, Role::Tool, Role::Assistant]
    );

    // Tool call captured from the assistant turn.
    assert_eq!(convo.messages[1].tool_calls.len(), 1);
    assert_eq!(convo.messages[1].tool_calls[0].name, "Read");

    // Code fence extracted from the final assistant turn.
    let snippets = &convo.messages[3].code_snippets;
    assert_eq!(snippets.len(), 1);
    assert_eq!(snippets[0].language.as_deref(), Some("rust"));
    assert!(snippets[0].text.contains("backoff::retry"));

    // Stable id derives from content.
    assert!(convo.id.starts_with("cv_"));
}

#[test]
fn reparsing_is_deterministic() {
    let (path, text) = fixture();
    let a = parse_jsonl(&path, &text).unwrap();
    let b = parse_jsonl(&path, &text).unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(a.content_hash, b.content_hash);
}
