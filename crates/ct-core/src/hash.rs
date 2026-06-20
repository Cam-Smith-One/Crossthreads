//! Content hashing for stable ids and deduplication (FR-ING-06, FR-SCH-04).
//!
//! Ids are derived from content, not source paths, so the same conversation
//! discovered twice (e.g. re-indexed, or synced from another machine) collapses
//! to one record.

use sha2::{Digest, Sha256};

use crate::model::{Message, Tool};

/// Hex sha-256 of the given bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

/// Stable content hash for a conversation: tool + the ordered (role, content)
/// of every message. Deliberately excludes timestamps and source paths so the
/// same logical conversation hashes identically across machines.
pub fn conversation_hash(tool: &Tool, messages: &[Message]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tool.slug().as_bytes());
    hasher.update([0]);
    for m in messages {
        hasher.update(role_byte(m).to_le_bytes());
        hasher.update(m.content.as_bytes());
        hasher.update([0]);
    }
    hex(&hasher.finalize())
}

/// A conversation id: a short, stable prefix of the content hash.
pub fn conversation_id(content_hash: &str) -> String {
    format!("cv_{}", &content_hash[..content_hash.len().min(16)])
}

/// A *stable* per-conversation key for user state (bookmarks, pins).
///
/// Unlike [`conversation_hash`] — which changes whenever a message is appended —
/// this keys on the tool, the source location, and a `seed` taken from the first
/// message, so it stays constant as a conversation grows. That lets bookmarks
/// and pins survive both a continuing session and a full index rebuild. (Two
/// sessions sharing one source file *and* an identical first message can
/// collide; that is rare and only loses the distinction between those two.)
pub fn session_key(tool: &Tool, source_path: &str, seed: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tool.slug().as_bytes());
    hasher.update([0]);
    hasher.update(source_path.as_bytes());
    hasher.update([0]);
    hasher.update(seed.as_bytes());
    let h = hex(&hasher.finalize());
    format!("sk_{}", &h[..h.len().min(16)])
}

fn role_byte(m: &Message) -> u8 {
    use crate::model::Role::*;
    match m.role {
        User => 1,
        Assistant => 2,
        Tool => 3,
        System => 4,
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Role;

    fn msg(role: Role, content: &str) -> Message {
        Message {
            id: "irrelevant".into(),
            role,
            content: content.into(),
            timestamp: None,
            code_snippets: vec![],
            tool_calls: vec![],
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn same_content_hashes_equal_regardless_of_message_ids() {
        let a = vec![msg(Role::User, "hello"), msg(Role::Assistant, "hi")];
        let mut b = a.clone();
        b[0].id = "different".into();
        b[0].timestamp = Some(chrono::Utc::now());
        assert_eq!(
            conversation_hash(&Tool::Aider, &a),
            conversation_hash(&Tool::Aider, &b),
        );
    }

    #[test]
    fn different_tool_changes_hash() {
        let m = vec![msg(Role::User, "hello")];
        assert_ne!(
            conversation_hash(&Tool::Aider, &m),
            conversation_hash(&Tool::Cursor, &m),
        );
    }

    #[test]
    fn id_has_prefix_and_is_short() {
        let h = sha256_hex(b"abc");
        let id = conversation_id(&h);
        assert!(id.starts_with("cv_"));
        assert_eq!(id.len(), 3 + 16);
    }
}
