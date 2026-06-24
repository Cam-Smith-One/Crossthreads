//! Bring-your-own-key storage for LLM providers, in the OS keychain (same
//! backend as the federation token). Best-effort: every call degrades to
//! `false`/`None` when no keychain is available (e.g. headless CI), so the env /
//! agent-login resolution still works without it. A stored key is treated as the
//! top-precedence credential for its provider (see [`crate::auth::resolve`]).
//!
//! The active-provider preference is stored here too, so the UI can pin which
//! provider LLM features use.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::auth::Provider;

const SERVICE: &str = "crossthreads";

fn key_user(p: Provider) -> &'static str {
    match p {
        Provider::Anthropic => "llm-key-anthropic",
        Provider::OpenAi => "llm-key-openai",
        Provider::Google => "llm-key-google",
    }
}

const PROVIDER_USER: &str = "llm-active-provider";

/// Process-lifetime cache of resolved keychain items, keyed by the entry's user
/// string. Reading the OS keychain prompts the user on macOS each time the
/// process isn't yet trusted for that item; the LLM hot path (provider
/// resolution on every completion) would otherwise re-read on every call and
/// spam prompts. We read each item at most once per launch and reuse the value;
/// writes update the cache in place. `None` means "looked up, absent".
fn cache() -> &'static Mutex<HashMap<&'static str, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Read an item through the cache, hitting the keychain only on a miss.
fn cached_read(user: &'static str) -> Option<String> {
    if let Some(hit) = cache().lock().ok().and_then(|c| c.get(user).cloned()) {
        return hit;
    }
    let value = keyring::Entry::new(SERVICE, user)
        .ok()
        .and_then(|e| e.get_password().ok())
        .filter(|v| !v.trim().is_empty());
    if let Ok(mut c) = cache().lock() {
        c.insert(user, value.clone());
    }
    value
}

/// Update the cache after a write so the new value is seen without re-prompting.
fn cache_store(user: &'static str, value: Option<String>) {
    if let Ok(mut c) = cache().lock() {
        c.insert(user, value);
    }
}

/// The stored BYO key for a provider, if any.
pub fn get_key(p: Provider) -> Option<String> {
    cached_read(key_user(p))
}

/// Store (non-empty) or clear (empty) a provider's BYO key. Returns whether the
/// keychain accepted the write.
pub fn set_key(p: Provider, key: &str) -> bool {
    let user = key_user(p);
    let Ok(entry) = keyring::Entry::new(SERVICE, user) else {
        return false;
    };
    let ok = if key.trim().is_empty() {
        let _ = entry.delete_credential();
        cache_store(user, None);
        true
    } else {
        let ok = entry.set_password(key).is_ok();
        if ok {
            cache_store(user, Some(key.to_string()));
        }
        ok
    };
    ok
}

/// The pinned active provider, if the user set one.
pub fn active_provider() -> Option<Provider> {
    cached_read(PROVIDER_USER)
        .as_deref()
        .and_then(Provider::parse)
}

/// Pin (or clear, with an empty/`None` value) the active provider.
pub fn set_active_provider(p: Option<Provider>) -> bool {
    let Ok(entry) = keyring::Entry::new(SERVICE, PROVIDER_USER) else {
        return false;
    };
    match p {
        Some(p) => {
            let ok = entry.set_password(p.id()).is_ok();
            if ok {
                cache_store(PROVIDER_USER, Some(p.id().to_string()));
            }
            ok
        }
        None => {
            let _ = entry.delete_credential();
            cache_store(PROVIDER_USER, None);
            true
        }
    }
}
