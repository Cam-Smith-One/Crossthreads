//! Where a thread stands (SPEC §6.7): a line on what's done, what's next and
//! what's in the way, kept per thread.
//!
//! Without a model it is read straight from the latest session: the last thing
//! asked, and how the agent answered. With the user's model connected, the
//! daemon writes a better one after a session goes quiet, and keeps it until
//! the thread moves on.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::Store;

/// Where a thread stands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    /// One or two sentences: what's been done and where it is now.
    pub summary: String,
    /// The next step, when known.
    #[serde(default)]
    pub next: Option<String>,
    /// What's in the way, when anything is.
    #[serde(default)]
    pub blocked: Option<String>,
    /// The last thing the user asked in the thread.
    #[serde(default)]
    pub last_ask: Option<String>,
    /// Written by the user's model (else read from the transcript).
    #[serde(default)]
    pub by_model: bool,
    /// The thread's last activity when this was written.
    #[serde(default)]
    pub upto: Option<String>,
}

pub fn clip(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        s
    } else {
        format!("{}…", s.chars().take(max).collect::<String>().trim_end())
    }
}

/// Messages that are the tool talking, not the person: skip them when looking
/// for "the last thing asked".
fn is_noise(text: &str) -> bool {
    let t = text.trim_start();
    t.is_empty()
        || t.starts_with('<')
        || t.starts_with("Caveat:")
        || t.starts_with("[Request interrupted")
        || t.starts_with("This session is being continued")
}

impl Store {
    /// The last few user and assistant messages of a session, oldest first:
    /// `(role, text)`.
    pub fn session_tail(&self, conversation_id: &str, n: usize) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT role, content FROM messages
             WHERE conversation_id = ?1 AND role IN ('user', 'assistant')
             ORDER BY seq DESC LIMIT ?2",
        )?;
        let mut rows = stmt
            .query_map(params![conversation_id, (n * 3) as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.retain(|(role, text)| !(role == "user" && is_noise(text)) && !text.trim().is_empty());
        rows.truncate(n);
        rows.reverse();
        Ok(rows)
    }

    /// Where a thread stands, read from its latest session (no model).
    pub fn plain_standing(&self, latest_conversation: &str) -> Result<Option<Standing>> {
        let tail = self.session_tail(latest_conversation, 12)?;
        let ask = tail
            .iter()
            .rev()
            .find(|(r, _)| r == "user")
            .map(|(_, t)| t.clone());
        let reply = tail
            .iter()
            .rev()
            .find(|(r, _)| r == "assistant")
            .map(|(_, t)| t.clone());
        let summary = match (&ask, &reply) {
            (_, Some(reply)) => clip(reply, 220),
            (Some(ask), None) => format!("Asked: {}", clip(ask, 200)),
            (None, None) => return Ok(None),
        };
        Ok(Some(Standing {
            summary,
            last_ask: ask.map(|a| clip(&a, 200)),
            ..Default::default()
        }))
    }

    /// The model-written standing kept for a thread, if any.
    pub fn saved_standing(&self, thread_id: &str) -> Result<Option<Standing>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta_kv WHERE key = ?1",
                [format!("standing:{thread_id}")],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    /// Keep a model-written standing for a thread.
    pub fn save_standing(&self, thread_id: &str, standing: &Standing) -> Result<()> {
        self.meta_set(
            &format!("standing:{thread_id}"),
            &serde_json::to_string(standing)?,
        )
    }
}

/// Read a model's answer: JSON `{"summary","next","blocked"}`, possibly in a
/// code fence or with text around it.
pub fn parse_model_standing(text: &str) -> Option<Standing> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    let v: serde_json::Value = serde_json::from_str(text.get(start..=end)?).ok()?;
    let field = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(|s| clip(s, 300))
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null") && s != "none")
    };
    Some(Standing {
        summary: field("summary")?,
        next: field("next"),
        blocked: field("blocked"),
        by_model: true,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_model_answer_in_any_wrapping() {
        let s = parse_model_standing(
            "Here you go:\n```json\n{\"summary\": \"Retry added and tested.\", \"next\": \"Add jitter\", \"blocked\": null}\n```",
        )
        .unwrap();
        assert_eq!(s.summary, "Retry added and tested.");
        assert_eq!(s.next.as_deref(), Some("Add jitter"));
        assert!(s.blocked.is_none());
        assert!(s.by_model);
        assert!(parse_model_standing("no json").is_none());
        assert!(parse_model_standing("{\"next\":\"x\"}").is_none());
    }

    #[test]
    fn clips_and_flattens() {
        assert_eq!(clip("a\n  b", 10), "a b");
        assert_eq!(clip("abcdefghij", 4), "abcd…");
    }
}
