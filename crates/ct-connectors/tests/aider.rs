//! Integration test for the Aider connector against a fixture transcript.

use std::path::Path;

use ct_connectors::connectors::aider::{discover_in_file, parse_in_file, split_sessions};
use ct_core::model::Role;

fn fixture_path(dir: &Path) -> std::path::PathBuf {
    // The connector keys off the exact filename `.aider.chat.history.md`.
    let dst = dir.join(".aider.chat.history.md");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/aider.chat.history.md");
    std::fs::copy(src, &dst).unwrap();
    dst
}

#[test]
fn splits_into_two_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture_path(dir.path());
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(split_sessions(&text).len(), 2);

    let discovered = discover_in_file(&path).unwrap();
    let locators: Vec<&str> = discovered.iter().filter_map(|s| s.locator.as_deref()).collect();
    assert_eq!(locators, vec!["session/0", "session/1"]);
}

#[test]
fn parses_first_session_with_roles_code_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture_path(dir.path());
    let convo = parse_in_file(&path, "session/0").unwrap();

    assert_eq!(convo.tool.slug(), "aider");
    assert_eq!(convo.source.fingerprint, "aider/v1");
    // Project is the directory holding the history file.
    assert_eq!(convo.project.as_deref(), Some(dir.path().to_string_lossy().as_ref()));
    // Timestamp parsed from the session header.
    assert_eq!(
        convo.started_at.unwrap().to_rfc3339(),
        "2026-05-14T09:12:00+00:00"
    );
    // Model sniffed from a `>` note.
    assert_eq!(convo.model.as_deref(), Some("gpt-4o"));

    // user then assistant; `>` notes excluded.
    let roles: Vec<Role> = convo.messages.iter().map(|m| m.role).collect();
    assert_eq!(roles, vec![Role::User, Role::Assistant]);
    assert!(convo.messages[0].content.contains("token refresh keeps looping"));

    // Fenced code extracted from the assistant turn.
    let snip = &convo.messages[1].code_snippets;
    assert_eq!(snip.len(), 1);
    assert_eq!(snip[0].language.as_deref(), Some("rust"));
    assert!(snip[0].text.contains("backoff::retry"));
}

#[test]
fn parses_second_session_independently() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture_path(dir.path());
    let convo = parse_in_file(&path, "session/1").unwrap();
    assert_eq!(convo.messages.len(), 2);
    assert!(convo.messages[0].content.contains("dark mode toggle"));
    assert_eq!(
        convo.started_at.unwrap().to_rfc3339(),
        "2026-05-20T10:00:00+00:00"
    );
}

#[test]
fn discovery_via_env_root_finds_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let _ = fixture_path(dir.path());
    // Point the connector's roots at our temp dir.
    std::env::set_var("CROSSTHREADS_AIDER_ROOTS", dir.path());

    use ct_connectors::connectors::AiderConnector;
    use ct_core::connector::Connector;
    let connector = AiderConnector::new();
    assert!(connector.detect());
    let sessions = connector.discover().unwrap();
    assert_eq!(sessions.len(), 2);
    let convo = connector.parse(&sessions[0]).unwrap();
    assert_eq!(convo.tool.slug(), "aider");

    std::env::remove_var("CROSSTHREADS_AIDER_ROOTS");
}
