//! Threads: grouping sessions across tools, linking, and link detection.

use chrono::{DateTime, Duration, Utc};
use ct_core::hash::{conversation_hash, conversation_id};
use ct_core::model::{Conversation, GitContext, Kind, Message, Role, Source, Tool};
use ct_store::{Store, ThreadFilters, Upsert};

const REPO: &str = "github.com/acme/api";

fn at(minutes: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-01T09:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + Duration::minutes(minutes)
}

fn msg(role: Role, content: &str, when: DateTime<Utc>) -> Message {
    Message {
        id: "m".into(),
        role,
        content: content.into(),
        timestamp: Some(when),
        code_snippets: vec![],
        tool_calls: vec![],
        metadata: serde_json::Value::Null,
    }
}

/// A session of `turns` user/assistant exchanges about `topic`, starting at
/// `start` minutes, one exchange per minute.
fn session(
    tool: Tool,
    file: &str,
    project: &str,
    topic: &str,
    start: i64,
    turns: usize,
) -> Conversation {
    let mut messages = Vec::new();
    for i in 0..turns {
        let t = at(start + i as i64);
        messages.push(msg(
            Role::User,
            &format!("Step {i} of the {topic} work: please look at the handler and tell me what you would change next, in detail."),
            t,
        ));
        messages.push(msg(
            Role::Assistant,
            &format!("For step {i} of {topic} I changed the retry loop to back off exponentially, added a jitter term, and covered it with a regression test."),
            t,
        ));
    }
    build(tool, file, project, messages)
}

fn build(tool: Tool, file: &str, project: &str, messages: Vec<Message>) -> Conversation {
    let content_hash = conversation_hash(&tool, &messages);
    Conversation {
        id: conversation_id(&content_hash),
        started_at: messages.first().and_then(|m| m.timestamp),
        ended_at: messages.last().and_then(|m| m.timestamp),
        tool,
        kind: Kind::Thread,
        title: None,
        project: Some(project.into()),
        model: None,
        git_context: GitContext::default(),
        source: Source {
            path: file.into(),
            offset: None,
            fingerprint: "test/v1".into(),
        },
        content_hash,
        messages,
    }
}

/// The same session, on a git branch.
fn on(branch: &str, mut c: Conversation) -> Conversation {
    c.git_context.branch = Some(branch.into());
    c
}

fn add(store: &mut Store, c: &Conversation, repo: &str) {
    assert_eq!(store.upsert_conversation(c).unwrap(), Upsert::Inserted);
    store
        .set_repo_key(c.project.as_deref().unwrap(), repo)
        .unwrap();
}

/// A Codex session that opens with the Claude session's transcript pasted in,
/// the way a transfer or handoff arrives.
fn transferred_from(source: &Conversation, file: &str, start: i64) -> Conversation {
    let transcript: String = source
        .messages
        .iter()
        .map(|m| format!("{:?}:\n  {}\n\n", m.role, m.content))
        .collect();
    build(
        Tool::Codex,
        file,
        "/work/api",
        vec![
            msg(
                Role::User,
                &format!("Continue this work. Here is the conversation so far:\n\n{transcript}"),
                at(start),
            ),
            msg(
                Role::Assistant,
                "Picking up from the regression test.",
                at(start + 1),
            ),
        ],
    )
}

fn all(store: &Store) -> Vec<ct_store::ThreadSummary> {
    store.threads(&ThreadFilters::default()).unwrap()
}

#[test]
fn every_session_is_a_thread_of_one_until_linked() {
    let mut store = Store::open_in_memory().unwrap();
    let a = session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 3);
    let b = session(Tool::Codex, "/s/b.jsonl", "/work/web", "billing", 600, 3);
    add(&mut store, &a, REPO);
    add(&mut store, &b, "github.com/acme/web");

    let threads = all(&store);
    assert_eq!(threads.len(), 2);
    // Most recently active first.
    assert_eq!(threads[0].tools, vec!["codex"]);
    assert_eq!(threads[1].repo_label.as_deref(), Some("acme/api"));
    assert!(threads
        .iter()
        .all(|t| t.session_count == 1 && t.status == "active"));
    // A thread can be opened by its id or by a session's conversation id.
    let by_id = store.thread(&threads[1].id).unwrap().unwrap();
    let by_session = store.thread(&a.id).unwrap().unwrap();
    assert_eq!(by_id, by_session);
    assert_eq!(by_id.sessions[0].conversation_id, a.id);
}

#[test]
fn a_growing_session_stays_one_session() {
    let mut store = Store::open_in_memory().unwrap();
    add(
        &mut store,
        &session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 2),
        REPO,
    );
    // The same file, re-indexed after more turns: a second row, same session.
    let longer = session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 5);
    add(&mut store, &longer, REPO);

    let threads = all(&store);
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].session_count, 1);
    assert_eq!(threads[0].message_count, 10);
    let detail = store.thread(&threads[0].id).unwrap().unwrap();
    assert_eq!(detail.sessions[0].conversation_id, longer.id);
}

#[test]
fn detects_a_transfer_and_links_it_as_continued() {
    let mut store = Store::open_in_memory().unwrap();
    let claude = session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 6);
    // Unrelated work in the same repository, close in time.
    let other = session(
        Tool::ClaudeCode,
        "/s/other.jsonl",
        "/work/api",
        "billing",
        2,
        6,
    );
    let codex = transferred_from(&claude, "/s/codex.jsonl", 10);
    add(&mut store, &claude, REPO);
    add(&mut store, &other, REPO);
    add(&mut store, &codex, REPO);

    let report = store.detect_links().unwrap();
    assert_eq!(report.scanned, 3);
    assert_eq!(report.applied, 1);

    let thread = store.thread(&codex.id).unwrap().unwrap();
    let ids: Vec<&str> = thread
        .sessions
        .iter()
        .map(|s| s.conversation_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![claude.id.as_str(), codex.id.as_str()],
        "no false merge"
    );
    assert_eq!(thread.summary.tools, vec!["claude-code", "codex"]);
    assert_eq!(thread.links.len(), 1);
    let link = &thread.links[0];
    assert_eq!(
        (link.from.as_str(), link.to.as_str()),
        (claude.id.as_str(), codex.id.as_str())
    );
    assert_eq!(
        (link.kind.as_str(), link.origin.as_str()),
        ("continued", "detected")
    );
    // The thread is named after the work, not the pasted transcript.
    assert!(thread.summary.title.starts_with("Step 0 of the oauth work"));

    // A second pass has nothing new to do.
    assert_eq!(store.detect_links().unwrap(), Default::default());
    assert_eq!(all(&store).len(), 2);
}

#[test]
fn a_nearby_session_in_another_tool_is_only_suggested() {
    let mut store = Store::open_in_memory().unwrap();
    // Different branches, so they aren't one piece of work on their own.
    let claude = on(
        "feat/oauth",
        session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 4),
    );
    let codex = on(
        "feat/cache",
        session(Tool::Codex, "/s/b.jsonl", "/work/api", "caching", 20, 4),
    );
    add(&mut store, &claude, REPO);
    add(&mut store, &codex, REPO);

    let report = store.detect_links().unwrap();
    assert_eq!((report.applied, report.suggested), (0, 1));
    assert_eq!(all(&store).len(), 2, "suggestions don't change membership");

    let thread = store.thread(&codex.id).unwrap().unwrap();
    assert_eq!(thread.summary.pending_suggestions, 1);
    let s = &thread.suggestions[0];
    assert_eq!(s.with, codex.id);
    assert_eq!(s.session.conversation_id, claude.id);
    assert!(s.confidence < ct_store::threads::APPLY_THRESHOLD);

    // Accepting it merges the two.
    let id = store
        .link_sessions(&s.session.conversation_id, &s.with, "continued", "manual")
        .unwrap()
        .unwrap();
    let merged = store.thread(&id).unwrap().unwrap();
    assert_eq!(merged.sessions.len(), 2);
    assert!(merged.suggestions.is_empty());
}

#[test]
fn back_to_back_sessions_in_one_tool_are_not_suggested() {
    let mut store = Store::open_in_memory().unwrap();
    add(
        &mut store,
        &session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 4),
        REPO,
    );
    add(
        &mut store,
        &session(Tool::ClaudeCode, "/s/b.jsonl", "/work/api", "logging", 5, 4),
        REPO,
    );
    let report = store.detect_links().unwrap();
    assert_eq!((report.applied, report.suggested), (0, 0));
}

#[test]
fn sessions_in_different_repositories_never_link() {
    let mut store = Store::open_in_memory().unwrap();
    let claude = session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 6);
    let mut codex = transferred_from(&claude, "/s/codex.jsonl", 10);
    codex.project = Some("/work/web".into());
    add(&mut store, &claude, REPO);
    add(&mut store, &codex, "github.com/acme/web");

    let report = store.detect_links().unwrap();
    assert_eq!((report.applied, report.suggested), (0, 0));
}

#[test]
fn a_rejected_suggestion_is_never_proposed_again() {
    let mut store = Store::open_in_memory().unwrap();
    let claude = on(
        "feat/oauth",
        session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 4),
    );
    let codex = on(
        "feat/cache",
        session(Tool::Codex, "/s/b.jsonl", "/work/api", "caching", 20, 4),
    );
    add(&mut store, &claude, REPO);
    add(&mut store, &codex, REPO);
    assert_eq!(store.detect_links().unwrap().suggested, 1);

    assert!(store.reject_link(&claude.id, &codex.id).unwrap());
    assert!(store
        .thread(&codex.id)
        .unwrap()
        .unwrap()
        .suggestions
        .is_empty());

    // The Codex session grows and is seen again as new by a fresh scan.
    let conn_reset = store.unlink_session(&codex.id).unwrap();
    assert!(!conn_reset, "nothing to unlink: it was never applied");
    let later = on(
        "feat/metrics",
        session(Tool::Codex, "/s/d.jsonl", "/work/api", "metrics", 22, 4),
    );
    add(&mut store, &later, REPO);
    let report = store.detect_links().unwrap();
    // The new session gets its own suggestion; the rejected pair does not return.
    assert_eq!(report.suggested, 1);
    assert!(store
        .thread(&codex.id)
        .unwrap()
        .unwrap()
        .suggestions
        .is_empty());
}

#[test]
fn unlinking_a_detected_link_sticks() {
    let mut store = Store::open_in_memory().unwrap();
    let claude = session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 6);
    let codex = transferred_from(&claude, "/s/codex.jsonl", 10);
    add(&mut store, &claude, REPO);
    add(&mut store, &codex, REPO);
    assert_eq!(store.detect_links().unwrap().applied, 1);
    assert_eq!(all(&store).len(), 1);

    assert!(store.unlink_session(&codex.id).unwrap());
    assert_eq!(all(&store).len(), 2);
    assert!(store.thread(&claude.id).unwrap().unwrap().links.is_empty());

    // Even if the scan record is lost, the pair is remembered as rejected.
    let fresh = transferred_from(&claude, "/s/codex.jsonl", 10);
    assert_eq!(
        store.upsert_conversation(&fresh).unwrap(),
        Upsert::Duplicate
    );
    assert_eq!(store.detect_links().unwrap().applied, 0);
    assert_eq!(all(&store).len(), 2);
}

#[test]
fn manual_link_rename_status_and_unlink() {
    let mut store = Store::open_in_memory().unwrap();
    let a = session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 2);
    let b = session(
        Tool::Codex,
        "/s/b.jsonl",
        "/work/api",
        "oauth review",
        500,
        2,
    );
    let c = session(
        Tool::Cursor,
        "/s/c.jsonl",
        "/work/api",
        "oauth polish",
        900,
        2,
    );
    for s in [&a, &b, &c] {
        add(&mut store, s, REPO);
    }

    // A thread of one can be renamed and parked before it is linked to anything.
    let solo = store.thread(&a.id).unwrap().unwrap().summary.id;
    assert_eq!(
        store
            .set_thread(&solo, Some("OAuth refresh"), Some("parked"))
            .unwrap(),
        Some(solo.clone())
    );
    assert!(store.set_thread(&solo, None, Some("nonsense")).is_err());
    assert_eq!(store.set_thread("th_nope", Some("x"), None).unwrap(), None);

    // Order of arguments doesn't matter: the earlier session's thread is kept.
    let id = store
        .link_sessions(&b.id, &a.id, "reviewed", "manual")
        .unwrap()
        .unwrap();
    assert_eq!(id, solo);
    assert_eq!(
        store
            .link_sessions(&b.id, &c.id, "manual", "manual")
            .unwrap(),
        Some(solo.clone())
    );
    assert!(store
        .link_sessions(&a.id, &a.id, "manual", "manual")
        .unwrap()
        .is_none());
    assert!(store
        .link_sessions(&a.id, "cv_missing", "manual", "manual")
        .unwrap()
        .is_none());
    assert!(store
        .link_sessions(&a.id, &b.id, "bogus", "manual")
        .is_err());

    let t = store.thread(&solo).unwrap().unwrap();
    assert_eq!(t.summary.title, "OAuth refresh");
    assert_eq!(t.summary.status, "parked");
    assert_eq!(t.summary.tools, vec!["claude-code", "codex", "cursor"]);
    assert_eq!(t.links.len(), 2);
    assert_eq!(
        store
            .threads(&ThreadFilters {
                status: Some("parked".into()),
                ..Default::default()
            })
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .threads(&ThreadFilters {
                tool: Some("cursor".into()),
                ..Default::default()
            })
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .threads(&ThreadFilters {
            repo: Some("acme/web".into()),
            ..Default::default()
        })
        .unwrap()
        .is_empty());

    // The founding session leaves. The thread keeps its id, title and the
    // other members, and the leaver is a thread of one.
    assert!(store.unlink_session(&a.id).unwrap());
    let left = store.thread(&a.id).unwrap().unwrap();
    assert_ne!(left.summary.id, solo);
    assert_eq!(left.sessions.len(), 1);
    assert_eq!(left.summary.status, "active");
    let rest = store.thread(&b.id).unwrap().unwrap();
    assert_eq!(rest.summary.id, solo);
    assert_eq!(rest.summary.title, "OAuth refresh");
    assert_eq!(rest.sessions.len(), 2);

    // An empty title goes back to the derived one.
    store
        .set_thread(&rest.summary.id, Some("  "), Some("active"))
        .unwrap();
    assert!(store
        .thread(&b.id)
        .unwrap()
        .unwrap()
        .summary
        .title
        .starts_with("Step 0"));
    // With no custom title or status, a thread that shrinks to one dissolves.
    assert!(store.unlink_session(&c.id).unwrap());
    assert_eq!(all(&store).len(), 3);
    assert!(!store.unlink_session(&b.id).unwrap());
}

#[test]
fn threads_survive_reopening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index.db");
    let a = session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 2);
    let b = session(
        Tool::Codex,
        "/s/b.jsonl",
        "/work/api",
        "oauth review",
        500,
        2,
    );
    let id = {
        let mut store = Store::open(&path).unwrap();
        add(&mut store, &a, REPO);
        add(&mut store, &b, REPO);
        let id = store
            .link_sessions(&a.id, &b.id, "manual", "manual")
            .unwrap()
            .unwrap();
        store.set_thread(&id, Some("Kept"), Some("done")).unwrap();
        id
    };
    let store = Store::open(&path).unwrap();
    let t = store.thread(&id).unwrap().unwrap();
    assert_eq!(
        (t.summary.title.as_str(), t.summary.status.as_str()),
        ("Kept", "done")
    );
    assert_eq!(t.sessions.len(), 2);
}

#[test]
fn repo_keys_are_filled_per_project() {
    let mut store = Store::open_in_memory().unwrap();
    let a = session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 2);
    assert_eq!(store.upsert_conversation(&a).unwrap(), Upsert::Inserted);
    assert_eq!(
        store.projects_missing_repo_key().unwrap(),
        vec!["/work/api"]
    );
    assert_eq!(store.set_repo_key("/work/api", REPO).unwrap(), 1);
    assert!(store.projects_missing_repo_key().unwrap().is_empty());
    assert_eq!(all(&store)[0].repo_key.as_deref(), Some(REPO));
}

#[test]
fn a_handoff_carries_the_goal_and_where_the_work_stands() {
    let mut store = Store::open_in_memory().unwrap();
    let claude = session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 30);
    let codex = transferred_from(&claude, "/s/codex.jsonl", 40);
    add(&mut store, &claude, REPO);
    add(&mut store, &codex, REPO);
    store
        .link_sessions(&claude.id, &codex.id, "continued", "manual")
        .unwrap();

    let block = store.thread_handoff(&codex.id, 3000).unwrap().unwrap();
    let md = &block.markdown;
    assert!(
        md.starts_with("## Handoff: Step 0 of the oauth work"),
        "{md}"
    );
    assert!(md.contains("Repository: acme/api"));
    assert!(md.contains("1. claude-code") && md.contains("2. codex"));
    // The goal comes from the first session, the state from the latest one.
    assert!(md.contains("### The original request\n\nStep 0 of the oauth work"));
    assert!(md.contains("### Where it left off (codex"));
    assert!(
        md.trim_end()
            .ends_with("Picking up from the regression test."),
        "{md}"
    );
    assert!(md.len() <= 3400, "stays near the budget: {}", md.len());
    assert_eq!(block.sources, vec![claude.id.clone(), codex.id.clone()]);

    // A long latest session keeps its end, not its start.
    let long = session(
        Tool::ClaudeCode,
        "/s/long.jsonl",
        "/work/web",
        "billing",
        500,
        60,
    );
    add(&mut store, &long, "github.com/acme/web");
    let md = store
        .thread_handoff(&long.id, 2500)
        .unwrap()
        .unwrap()
        .markdown;
    assert!(md.contains("earlier messages omitted"));
    assert!(
        md.contains("For step 59 of billing") && !md.contains("For step 30 of billing"),
        "{md}"
    );
    assert!(store.thread_handoff("th_missing", 2000).unwrap().is_none());
}

// ---- threads that group themselves (work key) ----------------------------

#[test]
fn sessions_on_one_branch_are_one_thread_in_any_tool() {
    let mut store = Store::open_in_memory().unwrap();
    // Days apart, different tools, same repository and branch: one thread.
    let a = on(
        "feat/oauth",
        session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 2),
    );
    let b = on(
        "feat/oauth",
        session(Tool::Codex, "/s/b.jsonl", "/work/api", "review", 3000, 2),
    );
    let c = on(
        "feat/billing",
        session(Tool::Codex, "/s/c.jsonl", "/work/api", "billing", 10, 2),
    );
    for s in [&a, &b, &c] {
        add(&mut store, s, REPO);
    }
    let threads = all(&store);
    assert_eq!(threads.len(), 2);
    let oauth = store.thread(&a.id).unwrap().unwrap();
    assert_eq!(oauth.sessions.len(), 2);
    assert_eq!(oauth.summary.tools, vec!["claude-code", "codex"]);
    assert_eq!(
        store.thread(&b.id).unwrap().unwrap().summary.id,
        oauth.summary.id
    );

    // The id comes from the repository and branch alone, so another computer
    // with its own sessions on that branch arrives at the same id.
    let mut other = Store::open_in_memory().unwrap();
    let d = on(
        "feat/oauth",
        session(Tool::Cursor, "/x/d.jsonl", "/home/me/api", "tests", 50, 2),
    );
    add(&mut other, &d, REPO);
    assert_eq!(
        other.thread(&d.id).unwrap().unwrap().summary.id,
        oauth.summary.id
    );
}

#[test]
fn on_main_a_long_break_starts_a_new_thread() {
    let mut store = Store::open_in_memory().unwrap();
    let a = on(
        "main",
        session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 2),
    );
    let b = on(
        "main",
        session(Tool::Codex, "/s/b.jsonl", "/work/api", "review", 60, 2),
    );
    let c = on(
        "main",
        session(Tool::Codex, "/s/c.jsonl", "/work/api", "later", 60 * 8, 2),
    );
    for s in [&a, &b, &c] {
        add(&mut store, s, REPO);
    }
    let first = store.thread(&a.id).unwrap().unwrap();
    assert_eq!(
        first.sessions.len(),
        2,
        "an hour apart: same stretch of work"
    );
    assert_ne!(
        store.thread(&c.id).unwrap().unwrap().summary.id,
        first.summary.id
    );
    assert_eq!(all(&store).len(), 2);
}

#[test]
fn taking_a_session_out_of_its_work_thread_sticks() {
    let mut store = Store::open_in_memory().unwrap();
    let a = on(
        "feat/x",
        session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "x", 0, 2),
    );
    let b = on(
        "feat/x",
        session(Tool::Codex, "/s/b.jsonl", "/work/api", "y", 30, 2),
    );
    add(&mut store, &a, REPO);
    add(&mut store, &b, REPO);
    let id = store.thread(&a.id).unwrap().unwrap().summary.id;
    // Renaming a thread that formed by itself works.
    assert_eq!(
        store.set_thread(&id, Some("X"), None).unwrap(),
        Some(id.clone())
    );
    assert!(store.unlink_session(&b.id).unwrap());
    assert_eq!(all(&store).len(), 2);
    assert_eq!(store.thread(&a.id).unwrap().unwrap().summary.title, "X");
    // A later session on the branch still joins the work's thread.
    let c = on(
        "feat/x",
        session(Tool::Cursor, "/s/c.jsonl", "/work/api", "z", 90, 2),
    );
    add(&mut store, &c, REPO);
    assert_eq!(store.thread(&c.id).unwrap().unwrap().summary.id, id);
    assert!(!store.unlink_session(&a.id).unwrap() || all(&store).len() == 3);
}

#[test]
fn where_a_thread_stands_comes_from_its_latest_session() {
    let mut store = Store::open_in_memory().unwrap();
    let a = on(
        "feat/x",
        session(Tool::ClaudeCode, "/s/a.jsonl", "/work/api", "oauth", 0, 3),
    );
    add(&mut store, &a, REPO);
    let st = store.plain_standing(&a.id).unwrap().unwrap();
    assert!(
        st.summary.starts_with("For step 2 of oauth"),
        "{}",
        st.summary
    );
    assert!(st.last_ask.unwrap().starts_with("Step 2 of the oauth work"));
    assert!(!st.by_model);

    let id = store.thread(&a.id).unwrap().unwrap().summary.id;
    assert!(store.saved_standing(&id).unwrap().is_none());
    let mine = ct_store::Standing {
        summary: "Retry done".into(),
        by_model: true,
        ..Default::default()
    };
    store.save_standing(&id, &mine).unwrap();
    assert_eq!(store.saved_standing(&id).unwrap(), Some(mine));
}
