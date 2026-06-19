//! Integration test for the Codex connector against a rollout fixture.

use std::path::Path;

use ct_connectors::connectors::codex::parse_rollout;
use ct_core::model::Role;

fn fixture() -> (std::path::PathBuf, String) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-rollout.jsonl");
    let text = std::fs::read_to_string(&path).unwrap();
    (path, text)
}

#[test]
fn parses_rollout_into_conversation() {
    let (path, text) = fixture();
    let convo = parse_rollout(&path, &text).unwrap();

    assert_eq!(convo.tool.slug(), "codex");
    assert_eq!(convo.source.fingerprint, "codex/v1");
    assert_eq!(convo.project.as_deref(), Some("/home/dev/acme-api"));
    assert_eq!(convo.model.as_deref(), Some("gpt-5-codex"));
    assert!(convo.started_at.unwrap() < convo.ended_at.unwrap());

    // user, assistant, tool(call), tool(output); reasoning + session_meta skipped.
    let roles: Vec<Role> = convo.messages.iter().map(|m| m.role).collect();
    assert_eq!(roles, vec![Role::User, Role::Assistant, Role::Tool, Role::Tool]);

    // Code fence extracted from the assistant turn.
    let snip = &convo.messages[1].code_snippets;
    assert_eq!(snip.len(), 1);
    assert_eq!(snip[0].language.as_deref(), Some("python"));

    // function_call captured as a tool call.
    assert_eq!(convo.messages[2].tool_calls.len(), 1);
    assert_eq!(convo.messages[2].tool_calls[0].name, "shell");
    assert!(convo.messages[3].content.contains("5 passed"));
}

#[test]
fn reparse_is_deterministic() {
    let (path, text) = fixture();
    assert_eq!(
        parse_rollout(&path, &text).unwrap().id,
        parse_rollout(&path, &text).unwrap().id
    );
}

#[test]
fn discovery_via_codex_home() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions/2026/05/18");
    std::fs::create_dir_all(&sessions).unwrap();
    let (_p, text) = fixture();
    std::fs::write(sessions.join("rollout-01J9.jsonl"), text).unwrap();
    std::env::set_var("CODEX_HOME", dir.path());

    use ct_connectors::connectors::CodexConnector;
    use ct_core::connector::Connector;
    let connector = CodexConnector::new();
    assert!(connector.detect());
    let sessions = connector.discover().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(connector.parse(&sessions[0]).unwrap().tool.slug(), "codex");

    std::env::remove_var("CODEX_HOME");
}
