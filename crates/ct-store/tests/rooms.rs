//! Rooms: messages, read positions, exchanges, file claims and the Meat Proxy
//! import.

use ct_store::rooms::{is_done, overlaps};
use ct_store::Store;

const R: &str = "github.com/acme/api";

#[test]
fn unread_is_everything_new_addressed_to_you_and_not_yours() {
    let s = Store::open_in_memory().unwrap();
    s.room_post(R, "claude", "plan: add backoff", None, false, 10)
        .unwrap();
    s.room_post(
        R,
        "claude",
        "codex, take src/auth",
        Some("codex"),
        false,
        11,
    )
    .unwrap();
    s.room_post(
        R,
        "claude",
        "note for gemini only",
        Some("gemini"),
        false,
        12,
    )
    .unwrap();
    s.room_post("other-room", "claude", "elsewhere", None, false, 13)
        .unwrap();

    // A newcomer catches up on what it missed, minus other people's DMs.
    let got: Vec<String> = s
        .room_unread(R, "codex")
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(got, ["plan: add backoff", "codex, take src/auth"]);
    // You never read your own messages back.
    assert!(s.room_unread(R, "claude").unwrap().is_empty());

    s.room_mark_read(R, "codex", 20).unwrap();
    assert!(s.room_unread(R, "codex").unwrap().is_empty());
    s.room_post(R, "cam", "use the config value", None, true, 21)
        .unwrap();
    let got = s.room_unread(R, "codex").unwrap();
    assert_eq!(got.len(), 1);
    assert!(got[0].human);
    assert_eq!(s.room_seen(R, "codex").unwrap(), Some(20));
    assert_eq!(s.room_seen(R, "nobody").unwrap(), None);
}

#[test]
fn an_exchange_runs_from_the_last_done_or_human_message() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.room_turns(R).unwrap(), 0);
    s.room_post(R, "claude", "brief", None, false, 1).unwrap();
    s.room_post(R, "codex", "did it", None, false, 2).unwrap();
    assert_eq!(s.room_turns(R).unwrap(), 2);
    assert!(!s.room_closed(R).unwrap());

    let done = s
        .room_post(R, "claude", "done: shipped the backoff", None, false, 3)
        .unwrap();
    assert!(s.room_closed(R).unwrap());
    assert_eq!(s.room_turns(R).unwrap(), 0, "DONE starts a fresh exchange");
    let texts: Vec<String> = s
        .room_exchange(R, done)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(texts, ["brief", "did it", "done: shipped the backoff"]);

    // A second exchange holds only its own messages.
    s.room_post(R, "claude", "next task", None, false, 4)
        .unwrap();
    assert!(!s.room_closed(R).unwrap());
    s.room_post(R, "cam", "keep going", None, true, 5).unwrap();
    assert_eq!(
        s.room_turns(R).unwrap(),
        0,
        "a human message resets the count"
    );
    s.room_post(R, "codex", "ok", None, false, 6).unwrap();
    let done2 = s.room_post(R, "codex", "DONE", None, false, 7).unwrap();
    let ids: Vec<i64> = s
        .room_exchange(R, done2)
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, [4, 5, 6, 7]);

    assert!(is_done("  Done — all green") && !is_done("not done yet"));
}

#[test]
fn history_and_room_list() {
    let s = Store::open_in_memory().unwrap();
    for i in 0..5 {
        s.room_post(R, "claude", &format!("m{i}"), None, false, i)
            .unwrap();
    }
    s.room_touch(R, "claude", 4).unwrap();
    s.room_post("design-review", "codex", "hello", None, false, 9)
        .unwrap();

    let last: Vec<String> = s
        .room_history(R, 2)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(last, ["m3", "m4"]);
    assert_eq!(s.room_messages_after(R, 3, 10).unwrap().len(), 2);

    let rooms = s.rooms().unwrap();
    assert_eq!(rooms[0].room, "design-review", "most recently active first");
    assert_eq!((rooms[1].messages, rooms[1].turns), (5, 5));
    assert_eq!(rooms[1].agents, vec![("claude".to_string(), 4)]);
}

#[test]
fn claims_block_others_extend_for_the_holder_and_expire() {
    let s = Store::open_in_memory().unwrap();
    assert!(s
        .claim("/work/api/src/auth", "codex", R, 100, 0)
        .unwrap()
        .is_ok());

    // A file inside a claimed folder, and a folder containing a claimed one.
    let clash = s
        .claim("/work/api/src/auth/session.rs", "claude", R, 100, 1)
        .unwrap()
        .unwrap_err();
    assert_eq!((clash.holder.as_str(), clash.expires), ("codex", 100));
    assert!(s
        .claim("/work/api/src", "claude", R, 100, 1)
        .unwrap()
        .is_err());
    // A sibling with a shared name prefix is not inside it.
    assert!(s
        .claim("/work/api/src/authz", "claude", R, 100, 1)
        .unwrap()
        .is_ok());
    // The holder can claim again to extend, and claim inside its own folder.
    assert!(s
        .claim("/work/api/src/auth", "codex", R, 200, 1)
        .unwrap()
        .is_ok());
    assert!(s
        .claim("/work/api/src/auth/session.rs", "codex", R, 200, 1)
        .unwrap()
        .is_ok());
    assert_eq!(s.claims(1).unwrap().len(), 3);

    assert_eq!(
        s.release(
            "codex",
            Some(&["/work/api/src/auth/session.rs".to_string()])
        )
        .unwrap(),
        1
    );
    assert_eq!(s.release_in_room("claude", R).unwrap(), 1);
    // Past its expiry a claim is gone and no longer blocks.
    assert!(s.claims(201).unwrap().is_empty());
    assert!(s
        .claim("/work/api/src/auth/session.rs", "claude", R, 400, 201)
        .unwrap()
        .is_ok());
    assert_eq!(s.release("claude", None).unwrap(), 1);

    assert!(overlaps("/a/b", "/a/b/") && overlaps("C:\\w\\api", "C:\\w\\api\\src"));
    assert!(!overlaps("/a/b", "/a/bc"));
}

#[test]
fn finds_a_session_by_its_transcript_and_binds_agents_to_them() {
    let s = Store::open_in_memory().unwrap();
    s.room_bind(R, "claude", "/t/a.jsonl", 5).unwrap();
    s.room_touch(R, "codex", 6).unwrap();
    assert_eq!(
        s.room_agents(R).unwrap(),
        vec![
            ("claude".to_string(), 5, Some("/t/a.jsonl".to_string())),
            ("codex".to_string(), 6, None),
        ]
    );
    assert_eq!(s.conversation_for_source("/t/a.jsonl").unwrap(), None);
}

#[test]
fn imports_a_meat_proxy_database_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meat-proxy.db");
    let src = rusqlite::Connection::open(&path).unwrap();
    src.execute_batch(
        "CREATE TABLE msg(id INTEGER PRIMARY KEY, ts REAL, room TEXT, author TEXT, recipient TEXT,
                          text TEXT, human INTEGER DEFAULT 0);
         INSERT INTO msg(ts, room, author, recipient, text, human) VALUES
           (100.5, 'api', 'claude', 'codex', 'brief', 0),
           (101.2, 'api', 'codex', NULL, 'DONE ok', 0),
           (102.0, 'api', 'cam', NULL, 'thanks', 1),
           (103.0, 'web', 'codex', NULL, 'hi', 0);",
    )
    .unwrap();
    drop(src);

    let s = Store::open_in_memory().unwrap();
    let map = |room: &str| format!("github.com/acme/{room}");
    assert_eq!(s.import_meat_proxy(&path, &map).unwrap(), 4);
    let api = s.room_history("github.com/acme/api", 10).unwrap();
    assert_eq!(api.len(), 3);
    assert_eq!((api[0].ts, api[0].to.as_deref()), (100, Some("codex")));
    assert!(api[2].human);
    // Imported history is already read: nobody gets a backlog.
    assert!(s
        .room_unread("github.com/acme/api", "codex")
        .unwrap()
        .is_empty());
    // Running it again adds nothing.
    assert_eq!(s.import_meat_proxy(&path, &map).unwrap(), 0);
    assert_eq!(s.rooms().unwrap().len(), 2);
}
