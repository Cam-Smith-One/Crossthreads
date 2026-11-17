//! Bring-your-own-key storage for LLM providers, in the OS keychain (the one
//! Crossthreads item; see [`crate::secrets`]). Best-effort: every call degrades to
//! `false`/`None` when no keychain is available (e.g. headless CI), so the env /
//! agent-login resolution still works without it. A stored key is treated as the
//! top-precedence credential for its provider (see [`crate::auth::resolve`]).
//!
//! The active-provider preference is stored here too, so the UI can pin which
//! provider LLM features use.

use crate::auth::Provider;

fn key_user(p: Provider) -> &'static str {
    match p {
        Provider::Anthropic => "llm-key-anthropic",
        Provider::OpenAi => "llm-key-openai",
        Provider::Google => "llm-key-google",
    }
}

const PROVIDER_USER: &str = "llm-active-provider";

/// The stored BYO key for a provider, if any.
pub fn get_key(p: Provider) -> Option<String> {
    crate::secrets::get(key_user(p))
}

/// Store (non-empty) or clear (empty) a provider's BYO key. Returns whether the
/// keychain accepted the write.
pub fn set_key(p: Provider, key: &str) -> bool {
    crate::secrets::set(key_user(p), Some(key))
}

/// The pinned active provider, if the user set one.
pub fn active_provider() -> Option<Provider> {
    crate::secrets::get(PROVIDER_USER)
        .as_deref()
        .and_then(Provider::parse)
}

/// Pin (or clear, with an empty/`None` value) the active provider.
pub fn set_active_provider(p: Option<Provider>) -> bool {
    crate::secrets::set(PROVIDER_USER, p.map(|p| p.id()))
}
