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
        .room_unread(R, "codex", None)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(got, ["plan: add backoff", "codex, take src/auth"]);
    // You never read your own messages back.
    assert!(s.room_unread(R, "claude", None).unwrap().is_empty());

    s.room_mark_read(R, "codex", 20).unwrap();
    assert!(s.room_unread(R, "codex", None).unwrap().is_empty());
    s.room_post(R, "cam", "use the config value", None, true, 21)
        .unwrap();
    let got = s.room_unread(R, "codex", None).unwrap();
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
        .room_unread("github.com/acme/api", "codex", None)
        .unwrap()
        .is_empty());
    // Running it again adds nothing.
    assert_eq!(s.import_meat_proxy(&path, &map).unwrap(), 0);
    assert_eq!(s.rooms().unwrap().len(), 2);
}

#[test]
fn relayed_messages_are_stored_once_and_reach_the_right_agent() {
    use ct_store::rooms::RoomWire;
    let s = Store::open_in_memory().unwrap();
    // claude here, on "mac", posts first.
    let local = s
        .room_post(R, "claude", "review src/auth?", None, false, 10)
        .unwrap();
    let wire = |seq, author: &str, to: Option<&str>, text: &str| RoomWire {
        seq,
        ts: 20 + seq,
        room: R.into(),
        author: author.into(),
        to: to.map(String::from),
        text: text.into(),
        human: false,
    };
    // A different "claude" on linux, and codex on linux replying to us.
    let a = wire(7, "claude", None, "I'm the linux claude");
    let b = wire(8, "codex", Some("claude@mac"), "looks fine");
    let c = wire(9, "codex", Some("claude@other"), "not for you");
    assert!(s.room_receive("linux", &a).unwrap().is_some());
    assert!(s.room_receive("linux", &b).unwrap().is_some());
    assert!(s.room_receive("linux", &c).unwrap().is_some());
    // Delivered twice (push, then catch-up): stored once.
    assert!(s.room_receive("linux", &b).unwrap().is_none());
    assert_eq!(s.room_received_upto("linux").unwrap(), 9);
    assert_eq!(s.room_received_upto("nowhere").unwrap(), 0);

    // The local claude sees the remote claude and the message addressed to it.
    let got = s.room_unread(R, "claude", Some("mac")).unwrap();
    let senders: Vec<String> = got.iter().map(|m| m.sender()).collect();
    assert_eq!(senders, vec!["claude@linux", "codex@linux"]);

    // Only this device's own messages are offered to peers.
    let mine = s.room_local_after(0, 0, 10).unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].id, local);
    assert!(s.room_local_after(local, 0, 10).unwrap().is_empty());

    // Remote agents show up in the room list.
    let rooms = s.rooms().unwrap();
    let names: Vec<&str> = rooms[0].agents.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"codex@linux") && names.contains(&"claude@linux"));
}

#[test]
fn a_room_appears_as_soon_as_an_agent_is_in_it() {
    let s = Store::open_in_memory().unwrap();
    // codex is listening; nobody has said anything yet.
    s.room_touch(R, "codex", 100).unwrap();
    let rooms = s.rooms().unwrap();
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].room, R);
    assert_eq!(rooms[0].messages, 0);
    assert_eq!(rooms[0].agents.len(), 1);
    assert!(!rooms[0].closed);

    // Once claude speaks, the count follows; the newest activity sorts first.
    s.room_post(R, "claude", "hi", None, false, 200).unwrap();
    s.room_touch("other-room", "gemini", 300).unwrap();
    let rooms = s.rooms().unwrap();
    assert_eq!(rooms[0].room, "other-room");
    let mine = rooms.iter().find(|r| r.room == R).unwrap();
    assert_eq!(mine.messages, 1);
    assert_eq!(mine.last_ts, 200);
}
