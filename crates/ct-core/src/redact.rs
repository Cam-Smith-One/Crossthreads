//! Secret redaction (FR-PRIV).
//!
//! AI coding conversations frequently contain credentials pasted into a prompt
//! or echoed by a tool — API keys, tokens, private keys, `.env` values. We scrub
//! those *before* anything is written to the index, so secrets never land in the
//! stored content, the FTS index, or the embeddings.
//!
//! The patterns are deliberately conservative (specific key prefixes, long
//! high-entropy values, explicit `KEY=value` assignments) to avoid mangling
//! ordinary prose. Redaction is applied centrally during indexing.

use std::sync::OnceLock;

use regex::Regex;

const PLACEHOLDER: &str = "[REDACTED]";

/// One redaction rule: a compiled pattern and its replacement (which may refer
/// to capture groups, e.g. `$key`).
struct Rule {
    re: Regex,
    replacement: &'static str,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let r = |pat: &str, replacement: &'static str| Rule {
            re: Regex::new(pat).expect("valid redaction regex"),
            replacement,
        };
        vec![
            // PEM private key blocks (any type).
            r(
                r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
                PLACEHOLDER,
            ),
            // Provider tokens with distinctive prefixes.
            r(r"sk-ant-[A-Za-z0-9_-]{20,}", PLACEHOLDER), // Anthropic
            r(r"sk-(?:proj-)?[A-Za-z0-9_-]{20,}", PLACEHOLDER), // OpenAI
            r(r"gh[posru]_[A-Za-z0-9]{20,}", PLACEHOLDER), // GitHub tokens
            r(r"github_pat_[A-Za-z0-9_]{22,}", PLACEHOLDER), // GitHub fine-grained PAT
            r(r"xox[baprs]-[A-Za-z0-9-]{10,}", PLACEHOLDER), // Slack
            r(r"AKIA[0-9A-Z]{16}", PLACEHOLDER),          // AWS access key id
            r(r"AIza[0-9A-Za-z_-]{35}", PLACEHOLDER),     // Google API key
            r(r"(?:sk|rk)_live_[A-Za-z0-9]{20,}", PLACEHOLDER), // Stripe live keys
            // JWTs (three base64url segments).
            r(
                r"eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
                PLACEHOLDER,
            ),
            // `Authorization: Bearer <token>` / `Bearer <token>`.
            r(r"(?i)bearer\s+[A-Za-z0-9._\-]{16,}", "Bearer [REDACTED]"),
            // Explicit secret assignments: KEY=value / KEY: value (env, configs).
            r(
                r#"(?i)(?P<key>\b(?:api[_-]?key|secret|access[_-]?token|auth[_-]?token|token|password|passwd|pwd|client[_-]?secret)\b)\s*[:=]\s*["']?[A-Za-z0-9._\-/+]{8,}["']?"#,
                "$key=[REDACTED]",
            ),
        ]
    })
}

/// Return `text` with any detected secrets replaced by `[REDACTED]`. Cheap when
/// there's nothing to redact (regex scans, no allocation unless it matches).
pub fn redact(text: &str) -> std::borrow::Cow<'_, str> {
    let mut out: Option<String> = None;
    for rule in rules() {
        let src: &str = out.as_deref().unwrap_or(text);
        if let std::borrow::Cow::Owned(replaced) = rule.re.replace_all(src, rule.replacement) {
            out = Some(replaced);
        }
    }
    match out {
        Some(s) => std::borrow::Cow::Owned(s),
        None => std::borrow::Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_known_secret_shapes() {
        let cases = [
            "my key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123",
            "OPENAI_API_KEY=sk-proj-abcdEFGH1234abcdEFGH5678",
            "token: ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "aws id AKIAIOSFODNN7EXAMPLE here",
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz123",
            "password = hunter2hunter2",
            "GOOGLE=AIzaSyA1234567890abcdefghijklmnopqrstuvw",
        ];
        for c in cases {
            let red = redact(c);
            assert!(red.contains("[REDACTED]"), "not redacted: {c} -> {red}");
        }
    }

    #[test]
    fn redacts_pem_private_key_block() {
        let pem = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEabc\nxyz==\n-----END RSA PRIVATE KEY-----\nafter";
        let red = redact(pem);
        assert!(red.contains("[REDACTED]"));
        assert!(!red.contains("MIIEabc"));
        assert!(red.contains("before") && red.contains("after"));
    }

    #[test]
    fn leaves_ordinary_prose_untouched() {
        let text = "Let's refactor the auth module and add a retry with backoff.";
        assert_eq!(redact(text), std::borrow::Cow::Borrowed(text));
        // A short token-looking word isn't redacted (too short / not a known shape).
        assert_eq!(redact("the secret sauce"), "the secret sauce");
    }
}
