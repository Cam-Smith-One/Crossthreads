//! Bring-your-own-key storage for LLM providers, in the OS keychain (same
//! backend as the federation token). Best-effort: every call degrades to
//! `false`/`None` when no keychain is available (e.g. headless CI), so the env /
//! agent-login resolution still works without it. A stored key is treated as the
//! top-precedence credential for its provider (see [`crate::auth::resolve`]).
//!
//! The active-provider preference is stored here too, so the UI can pin which
//! provider LLM features use.

use crate::auth::Provider;

const SERVICE: &str = "crossthreads";

fn key_user(p: Provider) -> &'static str {
    match p {
        Provider::Anthropic => "llm-key-anthropic",
        Provider::OpenAi => "llm-key-openai",
    }
}

const PROVIDER_USER: &str = "llm-active-provider";

/// The stored BYO key for a provider, if any.
pub fn get_key(p: Provider) -> Option<String> {
    keyring::Entry::new(SERVICE, key_user(p))
        .ok()?
        .get_password()
        .ok()
        .filter(|v| !v.trim().is_empty())
}

/// Store (non-empty) or clear (empty) a provider's BYO key. Returns whether the
/// keychain accepted the write.
pub fn set_key(p: Provider, key: &str) -> bool {
    let Ok(entry) = keyring::Entry::new(SERVICE, key_user(p)) else {
        return false;
    };
    if key.trim().is_empty() {
        let _ = entry.delete_credential();
        true
    } else {
        entry.set_password(key).is_ok()
    }
}

/// The pinned active provider, if the user set one.
pub fn active_provider() -> Option<Provider> {
    let raw = keyring::Entry::new(SERVICE, PROVIDER_USER)
        .ok()?
        .get_password()
        .ok()?;
    Provider::parse(&raw)
}

/// Pin (or clear, with an empty/`None` value) the active provider.
pub fn set_active_provider(p: Option<Provider>) -> bool {
    let Ok(entry) = keyring::Entry::new(SERVICE, PROVIDER_USER) else {
        return false;
    };
    match p {
        Some(p) => entry
            .set_password(match p {
                Provider::Anthropic => "anthropic",
                Provider::OpenAi => "openai",
            })
            .is_ok(),
        None => {
            let _ = entry.delete_credential();
            true
        }
    }
}
