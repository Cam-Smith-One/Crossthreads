//! Rooms: where agents and the human talk to each other (SPEC §6.4).
//!
//! This is the storage half: messages, each agent's read position, and file
//! claims. It never blocks. Waiting for a reply, deciding who is listening and
//! formatting text for agents live in the daemon.
//!
//! A room carries one **exchange** at a time. An exchange starts after the
//! last message that began with `DONE` or came from the human, and that same
//! boundary is what the optional turn cap counts from.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::Store;

/// One message in a room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomMessage {
    pub id: i64,
    /// Epoch seconds.
    pub ts: i64,
    pub room: String,
    pub author: String,
    /// Recipient for a direct message; `None` is a broadcast.
    pub to: Option<String>,
    pub text: String,
    /// Sent by the user from the board, not by an agent.
    pub human: bool,
}

/// A file or folder an agent has reserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub path: String,
    pub holder: String,
    pub room: String,
    /// Epoch seconds.
    pub expires: i64,
}

/// A room as listed for the human.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSummary {
    pub room: String,
    pub messages: i64,
    /// Agent messages in the current exchange.
    pub turns: i64,
    /// Epoch seconds of the latest message.
    pub last_ts: i64,
    /// Agents that have taken part, with when each was last seen (epoch
    /// seconds).
    pub agents: Vec<(String, i64)>,
    /// True when the latest agent message closed the exchange with `DONE`.
    pub closed: bool,
}

/// Does a message close its exchange?
pub fn is_done(text: &str) -> bool {
    text.trim_start().to_ascii_uppercase().starts_with("DONE")
}

/// True if two absolute paths are the same, or one contains the other.
pub fn overlaps(a: &str, b: &str) -> bool {
    fn within(inner: &str, outer: &str) -> bool {
        let outer = outer.trim_end_matches(['/', '\\']);
        inner
            .strip_prefix(outer)
            .is_some_and(|rest| rest.starts_with(['/', '\\']))
    }
    let (a, b) = (
        a.trim_end_matches(['/', '\\']),
        b.trim_end_matches(['/', '\\']),
    );
    a == b || within(a, b) || within(b, a)
}

fn message(r: &rusqlite::Row<'_>) -> rusqlite::Result<RoomMessage> {
    Ok(RoomMessage {
        id: r.get(0)?,
        ts: r.get(1)?,
        room: r.get(2)?,
        author: r.get(3)?,
        to: r.get(4)?,
        text: r.get(5)?,
        human: r.get::<_, i64>(6)? != 0,
    })
}

const COLS: &str = "id, ts, room, author, recipient, text, human";

impl Store {
    /// Add a message and return its id.
    pub fn room_post(
        &self,
        room: &str,
        author: &str,
        text: &str,
        to: Option<&str>,
        human: bool,
        now: i64,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO room_messages (ts, room, author, recipient, text, human)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![now, room, author, to, text, human as i64],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Note that an agent is here, creating its record on first sight. A
    /// newcomer starts having "read" nothing, so it catches up on what it missed.
    pub fn room_touch(&self, room: &str, name: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO room_agents (room, name, seen) VALUES (?1, ?2, ?3)
             ON CONFLICT(room, name) DO UPDATE SET seen = excluded.seen",
            params![room, name, now],
        )?;
        Ok(())
    }

    /// Record which session an agent is, as learned from a hook.
    pub fn room_bind(&self, room: &str, name: &str, transcript_path: &str, now: i64) -> Result<()> {
        self.room_touch(room, name, now)?;
        self.conn.execute(
            "UPDATE room_agents SET transcript_path = ?3 WHERE room = ?1 AND name = ?2",
            params![room, name, transcript_path],
        )?;
        Ok(())
    }

    /// Messages an agent has not read: everything after its cursor that it
    /// didn't write and that is addressed to it or to everyone.
    pub fn room_unread(&self, room: &str, name: &str) -> Result<Vec<RoomMessage>> {
        let cursor: i64 = self
            .conn
            .query_row(
                "SELECT cursor FROM room_agents WHERE room = ?1 AND name = ?2",
                params![room, name],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM room_messages
             WHERE room = ?1 AND id > ?2 AND author <> ?3
               AND (recipient IS NULL OR recipient = ?3)
             ORDER BY id"
        ))?;
        let rows = stmt
            .query_map(params![room, cursor, name], message)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Mark everything in the room as read by this agent.
    pub fn room_mark_read(&self, room: &str, name: &str, now: i64) -> Result<()> {
        self.room_touch(room, name, now)?;
        self.conn.execute(
            "UPDATE room_agents
             SET cursor = (SELECT IFNULL(MAX(id), 0) FROM room_messages WHERE room = ?1)
             WHERE room = ?1 AND name = ?2",
            params![room, name],
        )?;
        Ok(())
    }

    /// The id of the message that opened the current exchange's boundary: the
    /// latest `DONE` or human message, or 0.
    fn exchange_start(&self, room: &str) -> Result<i64> {
        let mut stmt = self.conn.prepare(
            "SELECT id, text, human FROM room_messages WHERE room = ?1 ORDER BY id DESC LIMIT 500",
        )?;
        let rows = stmt.query_map([room], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)? != 0,
            ))
        })?;
        for row in rows {
            let (id, text, human) = row?;
            if human || is_done(&text) {
                return Ok(id);
            }
        }
        Ok(0)
    }

    /// Agent messages sent in the current exchange.
    pub fn room_turns(&self, room: &str) -> Result<i64> {
        let start = self.exchange_start(room)?;
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM room_messages WHERE room = ?1 AND id > ?2 AND human = 0",
            params![room, start],
            |r| r.get(0),
        )?)
    }

    /// The messages of the exchange that a `DONE` message (by id) closed: from
    /// just after the previous `DONE` up to and including it.
    pub fn room_exchange(&self, room: &str, done_id: i64) -> Result<Vec<RoomMessage>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM room_messages WHERE room = ?1 AND id <= ?2 ORDER BY id DESC LIMIT 500"
        ))?;
        let rows = stmt
            .query_map(params![room, done_id], message)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out = Vec::new();
        for m in rows {
            if m.id != done_id && !m.human && is_done(&m.text) {
                break;
            }
            out.push(m);
        }
        out.reverse();
        Ok(out)
    }

    /// The last `limit` messages of a room, oldest first.
    pub fn room_history(&self, room: &str, limit: usize) -> Result<Vec<RoomMessage>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM room_messages WHERE room = ?1 ORDER BY id DESC LIMIT ?2"
        ))?;
        let mut rows = stmt
            .query_map(params![room, limit as i64], message)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Messages in a room after `after`, oldest first (for the board's stream).
    pub fn room_messages_after(
        &self,
        room: &str,
        after: i64,
        limit: usize,
    ) -> Result<Vec<RoomMessage>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM room_messages WHERE room = ?1 AND id > ?2 ORDER BY id LIMIT ?3"
        ))?;
        let rows = stmt
            .query_map(params![room, after, limit as i64], message)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Agents that have taken part in a room, with when each was last seen and
    /// the session it is, if known.
    pub fn room_agents(&self, room: &str) -> Result<Vec<(String, i64, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, seen, transcript_path FROM room_agents WHERE room = ?1 ORDER BY name",
        )?;
        let rows = stmt
            .query_map([room], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// When an agent was last seen in a room.
    pub fn room_seen(&self, room: &str, name: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT seen FROM room_agents WHERE room = ?1 AND name = ?2",
                params![room, name],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// True when the latest agent message in the room is a `DONE`.
    pub fn room_closed(&self, room: &str) -> Result<bool> {
        let last: Option<String> = self
            .conn
            .query_row(
                "SELECT text FROM room_messages WHERE room = ?1 AND human = 0
                 ORDER BY id DESC LIMIT 1",
                [room],
                |r| r.get(0),
            )
            .optional()?;
        Ok(last.is_some_and(|t| is_done(&t)))
    }

    /// Every room that has messages, most recently active first.
    pub fn rooms(&self) -> Result<Vec<RoomSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT room, COUNT(*), MAX(ts) FROM room_messages GROUP BY room ORDER BY MAX(id) DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for (room, messages, last_ts) in rows {
            out.push(RoomSummary {
                turns: self.room_turns(&room)?,
                closed: self.room_closed(&room)?,
                agents: self
                    .room_agents(&room)?
                    .into_iter()
                    .map(|(name, seen, _)| (name, seen))
                    .collect(),
                room,
                messages,
                last_ts,
            });
        }
        Ok(out)
    }

    // ---- claims ------------------------------------------------------------

    /// Claims that have not expired. Expired ones are removed as a side effect.
    pub fn claims(&self, now: i64) -> Result<Vec<Claim>> {
        self.conn
            .execute("DELETE FROM claims WHERE expires < ?1", [now])?;
        let mut stmt = self
            .conn
            .prepare("SELECT path, holder, room, expires FROM claims ORDER BY path")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Claim {
                    path: r.get(0)?,
                    holder: r.get(1)?,
                    room: r.get(2)?,
                    expires: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Reserve `path` for `holder`, or return the claim that blocks it. An
    /// agent's own claim never blocks it, so claiming again extends.
    pub fn claim(
        &self,
        path: &str,
        holder: &str,
        room: &str,
        expires: i64,
        now: i64,
    ) -> Result<std::result::Result<(), Claim>> {
        if let Some(clash) = self
            .claims(now)?
            .into_iter()
            .find(|c| c.holder != holder && overlaps(path, &c.path))
        {
            return Ok(Err(clash));
        }
        self.conn.execute(
            "INSERT OR REPLACE INTO claims (path, holder, room, expires) VALUES (?1, ?2, ?3, ?4)",
            params![path, holder, room, expires],
        )?;
        Ok(Ok(()))
    }

    /// Release an agent's claims: the given paths, or all of them. Returns how
    /// many were released.
    pub fn release(&self, holder: &str, paths: Option<&[String]>) -> Result<usize> {
        match paths {
            Some(paths) => {
                let mut n = 0;
                for p in paths {
                    n += self.conn.execute(
                        "DELETE FROM claims WHERE holder = ?1 AND path = ?2",
                        params![holder, p],
                    )?;
                }
                Ok(n)
            }
            None => Ok(self
                .conn
                .execute("DELETE FROM claims WHERE holder = ?1", [holder])?),
        }
    }

    /// Release everything an agent holds in one room (when it sends `DONE`).
    pub fn release_in_room(&self, holder: &str, room: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM claims WHERE holder = ?1 AND room = ?2",
            params![holder, room],
        )?)
    }

    /// The id of the latest indexed version of the session read from `path`.
    pub fn conversation_for_source(&self, path: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM conversations WHERE source_path = ?1 AND kind = 'thread'
                 ORDER BY message_count DESC LIMIT 1",
                [path],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Bring in a Meat Proxy database (`~/.meat-proxy/meat-proxy.db`): its
    /// messages and each agent's read position. Claims are not imported; they
    /// are short-lived. `map_room` turns a Meat Proxy room name into the room
    /// id to file it under. Returns how many messages were imported. Safe to
    /// run again: a room that already has messages here is skipped.
    pub fn import_meat_proxy(
        &self,
        path: &std::path::Path,
        map_room: &dyn Fn(&str) -> String,
    ) -> Result<usize> {
        let src = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut stmt = src
            .prepare("SELECT id, ts, room, author, recipient, text, human FROM msg ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let existing: std::collections::HashSet<String> =
            self.rooms()?.into_iter().map(|r| r.room).collect();
        let tx = self.conn.unchecked_transaction()?;
        let mut imported = 0;
        let mut touched = std::collections::BTreeSet::new();
        for (_, ts, room, author, to, text, human) in rows {
            let room = map_room(&room);
            if existing.contains(&room) {
                continue;
            }
            tx.execute(
                "INSERT INTO room_messages (ts, room, author, recipient, text, human)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![ts as i64, room, author, to, text, human],
            )?;
            touched.insert((room, author, human != 0, ts as i64));
            imported += 1;
        }
        // Everyone who spoke has, by definition, been here; imported history is
        // already read, so nobody is handed a backlog.
        for (room, author, human, ts) in touched {
            if human {
                continue;
            }
            tx.execute(
                "INSERT INTO room_agents (room, name, seen, cursor) VALUES (?1, ?2, ?3,
                    (SELECT IFNULL(MAX(id), 0) FROM room_messages WHERE room = ?1))
                 ON CONFLICT(room, name) DO UPDATE SET
                    seen = max(seen, excluded.seen), cursor = excluded.cursor",
                params![room, author, ts],
            )?;
        }
        tx.commit()?;
        Ok(imported)
    }
}
