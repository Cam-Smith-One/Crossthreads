//! Every Crossthreads secret in **one** OS keychain item.
//!
//! macOS asks before a program reads a keychain item it isn't yet trusted for,
//! once per item. Crossthreads used to keep each secret (the federation token,
//! each provider's API key, the active-provider choice) in its own item, so a
//! new daemon or CLI process could ask several times in a row, and again after
//! every update (an unsigned build gets a new identity each time). Keeping them
//! together means at most one prompt per program, and "Always Allow" covers
//! everything.
//!
//! The item holds a small JSON object of name → value. It is read at most once
//! per process and cached; writes update the cache. Items from older versions
//! are folded in the first time the combined item is missing, then deleted.
//! Everything degrades to "nothing stored" when no keychain is available (e.g.
//! headless CI), so callers keep their other fallbacks.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

const SERVICE: &str = "crossthreads";
const ITEM: &str = "secrets";

/// Names used by the separate items of earlier versions, folded in once.
const LEGACY: &[&str] = &[
    "federation-token",
    "llm-key-anthropic",
    "llm-key-openai",
    "llm-key-google",
    "llm-active-provider",
];

type Secrets = BTreeMap<String, String>;

fn state() -> &'static Mutex<Option<Secrets>> {
    static STATE: OnceLock<Mutex<Option<Secrets>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

fn entry(user: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, user).ok()
}

/// Read the combined item, folding in (and then deleting) older separate items
/// the first time.
fn load_from_keychain() -> Secrets {
    if let Some(json) = entry(ITEM).and_then(|e| e.get_password().ok()) {
        return parse(&json);
    }
    let mut found = Secrets::new();
    for name in LEGACY {
        if let Some(v) = entry(name)
            .and_then(|e| e.get_password().ok())
            .filter(|v| !v.trim().is_empty())
        {
            found.insert((*name).to_string(), v);
        }
    }
    if !found.is_empty() && write_to_keychain(&found) {
        for name in LEGACY {
            if let Some(e) = entry(name) {
                let _ = e.delete_credential();
            }
        }
    }
    found
}

fn write_to_keychain(s: &Secrets) -> bool {
    let Some(e) = entry(ITEM) else {
        return false;
    };
    if s.is_empty() {
        let _ = e.delete_credential();
        return true;
    }
    serde_json::to_string(s)
        .ok()
        .is_some_and(|json| e.set_password(&json).is_ok())
}

/// Parse the stored JSON; anything unreadable counts as empty.
pub fn parse(json: &str) -> Secrets {
    serde_json::from_str(json).unwrap_or_default()
}

fn with<T>(f: impl FnOnce(&mut Secrets) -> T) -> T {
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    let secrets = guard.get_or_insert_with(load_from_keychain);
    f(secrets)
}

/// A stored secret.
pub fn get(name: &str) -> Option<String> {
    with(|s| s.get(name).cloned())
}

/// Store (`Some`, non-empty) or remove (`None` or empty) a secret. Returns
/// whether the keychain accepted the change.
pub fn set(name: &str, value: Option<&str>) -> bool {
    with(|s| {
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => {
                s.insert(name.to_string(), v.to_string());
            }
            None => {
                s.remove(name);
            }
        }
        write_to_keychain(s)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_json_is_empty_not_an_error() {
        assert!(parse("not json").is_empty());
        let s = parse(r#"{"federation-token":"t","llm-key-openai":"k"}"#);
        assert_eq!(s.get("federation-token").map(String::as_str), Some("t"));
        assert_eq!(s.len(), 2);
    }
}
