//! Small text helpers shared across connectors.

use ct_core::model::CodeSnippet;

/// Extract fenced code blocks (```lang\n…\n```) from markdown-ish content.
///
/// Best-effort: tolerates unterminated fences (takes to end of text) and
/// missing language tags. Returns snippets in document order.
pub fn extract_code_snippets(content: &str) -> Vec<CodeSnippet> {
    let mut snippets = Vec::new();
    let mut lines = content.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            let lang = rest.trim();
            let language = if lang.is_empty() {
                None
            } else {
                // A fence info string may carry more than a language; keep the
                // first token (e.g. "rust title=foo" -> "rust").
                Some(lang.split_whitespace().next().unwrap_or(lang).to_string())
            };

            let mut body = String::new();
            let mut closed = false;
            for inner in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    closed = true;
                    break;
                }
                body.push_str(inner);
                body.push('\n');
            }
            // Drop a trailing newline we added for join purposes.
            if body.ends_with('\n') {
                body.pop();
            }
            if !body.is_empty() || closed {
                snippets.push(CodeSnippet {
                    language,
                    text: body,
                    linked_file: None,
                });
            }
        }
    }

    snippets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_language_and_body() {
        let md = "before\n```rust\nfn main() {}\n```\nafter";
        let s = extract_code_snippets(md);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].language.as_deref(), Some("rust"));
        assert_eq!(s[0].text, "fn main() {}");
    }

    #[test]
    fn handles_no_language_and_multiple_blocks() {
        let md = "```\nplain\n```\ntext\n```py\nx=1\n```";
        let s = extract_code_snippets(md);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].language, None);
        assert_eq!(s[1].language.as_deref(), Some("py"));
        assert_eq!(s[1].text, "x=1");
    }

    #[test]
    fn no_fences_yields_nothing() {
        assert!(extract_code_snippets("just prose here").is_empty());
    }
}
