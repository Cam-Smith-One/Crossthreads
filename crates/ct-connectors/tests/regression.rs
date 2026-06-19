//! Connector regression corpus.
//!
//! Parses every committed fixture and asserts the invariants every connector
//! must uphold. This is the guard against silent parser breakage when a tool's
//! on-disk format drifts (PRD top risk; FR-ING-08): if a change makes a fixture
//! stop parsing — or parse differently — this fails in CI. Extend it by adding a
//! fixture and a case below.

use std::path::Path;

use ct_core::model::{Conversation, Role};

fn fixtures() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> (std::path::PathBuf, String) {
    let path = fixtures().join(rel);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"));
    (path, text)
}

/// Invariants every parsed conversation must satisfy.
fn assert_valid(convo: &Conversation, expect_fingerprint: &str) {
    assert!(convo.id.starts_with("cv_"), "id should be content-derived");
    assert!(!convo.content_hash.is_empty(), "content hash required");
    assert_eq!(
        convo.source.fingerprint, expect_fingerprint,
        "fingerprint stamp"
    );
    assert!(!convo.source.path.is_empty(), "provenance path required");
    assert!(!convo.messages.is_empty(), "a conversation needs messages");

    for (i, m) in convo.messages.iter().enumerate() {
        // Every message is either text-bearing or a tool call — never empty noise.
        let has_content = !m.content.trim().is_empty() || !m.tool_calls.is_empty();
        assert!(has_content, "message {i} is empty");
        // Roles come from the closed enum; assert the set is what we expect.
        assert!(matches!(
            m.role,
            Role::User | Role::Assistant | Role::Tool | Role::System
        ));
    }

    // Timestamps, when present, must be ordered.
    if let (Some(s), Some(e)) = (convo.started_at, convo.ended_at) {
        assert!(s <= e, "started_at must precede ended_at");
    }
}

#[test]
fn claude_code_fixture_is_stable() {
    use ct_connectors::connectors::claude_code::parse_jsonl;
    let (path, text) = read("tests/fixtures/claude_code_session.jsonl");
    let convo = parse_jsonl(&path, &text).unwrap();
    assert_valid(&convo, "claude-code/v1");
    // Golden: message count is part of the contract.
    assert_eq!(convo.messages.len(), 4);
    assert_eq!(
        parse_jsonl(&path, &text).unwrap().id,
        convo.id,
        "deterministic"
    );
}

#[test]
fn aider_fixture_is_stable() {
    use ct_connectors::connectors::aider::parse_in_file;
    // Aider needs the canonical filename.
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join(".aider.chat.history.md");
    std::fs::copy(
        fixtures().join("tests/fixtures/aider.chat.history.md"),
        &dst,
    )
    .unwrap();

    let s0 = parse_in_file(&dst, "session/0").unwrap();
    let s1 = parse_in_file(&dst, "session/1").unwrap();
    assert_valid(&s0, "aider/v1");
    assert_valid(&s1, "aider/v1");
    assert_eq!(s0.messages.len(), 2);
    assert_eq!(s1.messages.len(), 2);
    assert_ne!(s0.id, s1.id, "distinct sessions, distinct ids");
}

#[test]
fn codex_fixture_is_stable() {
    use ct_connectors::connectors::codex::parse_rollout;
    let (path, text) = read("tests/fixtures/codex-rollout.jsonl");
    let convo = parse_rollout(&path, &text).unwrap();
    assert_valid(&convo, "codex/v1");
    assert_eq!(convo.messages.len(), 4);
}

#[test]
fn every_builtin_connector_reports_a_versioned_fingerprint() {
    // Fingerprints must be non-empty and namespaced `tool/vN` so drift can be
    // versioned (FR-ING-08).
    for c in ct_connectors::builtin() {
        let fp = c.fingerprint();
        assert!(
            fp.contains('/'),
            "{} fingerprint should be tool/vN: {fp}",
            c.tool().slug()
        );
        assert!(
            fp.ends_with(|ch: char| ch.is_ascii_digit()),
            "{fp} should end in a version"
        );
    }
}
