//! Indexing orchestration shared by the CLI and the daemon.
//!
//! Ties the connectors, store, and embedder together: discover sessions, parse
//! them, persist (deduped), then embed any new messages. Kept separate from
//! `ct-store` (which stays embedder/connector-agnostic) and from the binaries
//! (which shouldn't duplicate this loop).

use std::collections::BTreeMap;

use anyhow::Result;
use ct_core::connector::ConnectorError;
use ct_core::model::Conversation;
use ct_core::redact::redact;
use ct_embed::Embedder;
use ct_store::{Store, Upsert};

/// Redact secrets from every message of a conversation in place, so credentials
/// never reach the stored content, FTS index, or embeddings.
fn redact_conversation(convo: &mut Conversation) {
    for m in &mut convo.messages {
        if let std::borrow::Cow::Owned(clean) = redact(&m.content) {
            m.content = clean;
        }
        for s in &mut m.code_snippets {
            if let std::borrow::Cow::Owned(clean) = redact(&s.text) {
                s.text = clean;
            }
        }
    }
}

/// Summary of one indexing pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexReport {
    /// Conversations successfully parsed from sources.
    pub parsed: usize,
    /// Newly stored conversations.
    pub inserted: usize,
    /// Conversations already present (skipped by content-hash dedup).
    pub duplicate: usize,
    /// Sessions that failed to parse (isolated, logged).
    pub unparseable: usize,
    /// Messages embedded this pass.
    pub embedded: usize,
}

/// Connector roots to watch for changes (used by the daemon's file watcher).
pub fn watch_roots() -> Vec<std::path::PathBuf> {
    ct_connectors::builtin()
        .iter()
        .flat_map(|c| c.roots())
        .collect()
}

/// Detect-and-parse across the built-in connectors, isolating per-session
/// failures so one bad file never aborts the run (FR-ING-07). `limit` caps the
/// number of parsed conversations when set.
pub fn collect(limit: Option<usize>) -> (Vec<Conversation>, usize) {
    let mut conversations = Vec::new();
    let mut unparseable = 0usize;

    for connector in ct_connectors::builtin() {
        if !connector.detect() {
            continue;
        }
        let sessions = match connector.discover() {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "warn: discovery failed for {}: {e}",
                    connector.tool().slug()
                );
                continue;
            }
        };

        // Aggregate per-connector parse failures by reason instead of flooding
        // the log with a warning per session (a heavy Cursor user can have
        // thousands of empty tabs / unparseable entries).
        let mut skipped_here = 0usize;
        let mut by_reason: BTreeMap<String, usize> = BTreeMap::new();

        for session in sessions {
            if let Some(limit) = limit {
                if conversations.len() >= limit {
                    return (conversations, unparseable);
                }
            }
            match connector.parse(&session) {
                Ok(mut convo) => {
                    // Scrub secrets before anything reaches the index (FR-PRIV).
                    redact_conversation(&mut convo);
                    conversations.push(convo);
                }
                Err(e) => {
                    unparseable += 1;
                    skipped_here += 1;
                    *by_reason.entry(reason_category(&e)).or_insert(0) += 1;
                }
            }
        }

        if skipped_here > 0 {
            // Show reasons by frequency, e.g. "147× tab had no usable messages, …".
            let mut reasons: Vec<(String, usize)> = by_reason.into_iter().collect();
            reasons.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            let breakdown = reasons
                .iter()
                .take(4)
                .map(|(r, n)| format!("{n}× {r}"))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "warn: {} — skipped {skipped_here} session(s): {breakdown}",
                connector.tool().slug(),
            );
        }
    }

    (conversations, unparseable)
}

/// A coarse, id-stripped category for a parse error, for grouping in the log.
fn reason_category(e: &ConnectorError) -> String {
    let reason = match e {
        ConnectorError::Parse { reason, .. } | ConnectorError::UnknownFormat { reason, .. } => {
            reason.clone()
        }
        other => other.to_string(),
    };
    // Replace id-like tokens (containing digits) so similar errors collapse.
    reason
        .split_whitespace()
        .map(|w| {
            if w.chars().any(|c| c.is_ascii_digit()) {
                "<id>"
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Embed all messages without a vector, in batches. Returns the count embedded.
pub fn embed_pending(store: &mut Store, embedder: &dyn Embedder) -> Result<usize> {
    let mut total = 0;
    loop {
        let batch = store.pending_embeddings(256)?;
        if batch.is_empty() {
            break;
        }
        let texts: Vec<String> = batch.iter().map(|(_, c)| c.clone()).collect();
        let vecs = embedder.embed(&texts)?;
        let rows: Vec<(i64, Vec<f32>)> = batch.iter().map(|(rowid, _)| *rowid).zip(vecs).collect();
        store.store_embeddings(embedder.id(), &rows)?;
        total += rows.len();
    }
    Ok(total)
}

/// One full indexing pass: collect, persist (dedup), and embed new messages.
pub fn index_once(
    store: &mut Store,
    embedder: &dyn Embedder,
    limit: Option<usize>,
) -> Result<IndexReport> {
    let (conversations, unparseable) = collect(limit);
    let mut report = IndexReport {
        parsed: conversations.len(),
        unparseable,
        ..Default::default()
    };

    for convo in &conversations {
        match store.upsert_conversation(convo)? {
            Upsert::Inserted => report.inserted += 1,
            Upsert::Duplicate => report.duplicate += 1,
            Upsert::Forgotten => {} // tombstoned: intentionally not re-indexed
        }
    }

    report.embedded = embed_pending(store, embedder)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::redact_conversation;
    use ct_core::model::{Conversation, GitContext, Kind, Message, Role, Source, Tool};

    fn msg(content: &str) -> Message {
        Message {
            id: "m".into(),
            role: Role::User,
            content: content.into(),
            timestamp: None,
            code_snippets: vec![],
            tool_calls: vec![],
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn indexing_redacts_secrets_in_messages() {
        let mut convo = Conversation {
            id: "cv_t".into(),
            tool: Tool::ClaudeCode,
            kind: Kind::Thread,
            title: None,
            project: None,
            model: None,
            started_at: None,
            ended_at: None,
            git_context: GitContext::default(),
            source: Source {
                path: "/x".into(),
                offset: None,
                fingerprint: "t/v1".into(),
            },
            content_hash: "h".into(),
            messages: vec![msg(
                "deploy with OPENAI_API_KEY=sk-proj-abcdEFGH1234abcdEFGH5678",
            )],
        };
        redact_conversation(&mut convo);
        assert!(convo.messages[0].content.contains("[REDACTED]"));
        assert!(!convo.messages[0].content.contains("sk-proj-abcdEFGH"));
    }
}
