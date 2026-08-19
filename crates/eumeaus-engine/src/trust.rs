//! Thin engine-level wrapper around `eumeaus-plugin-host`'s local trust
//! store (SPEC.md §8 open question 2), so `eumeaus-cli`'s `trust
//! add/list/remove` only ever depends on `eumeaus-engine` — consistent
//! with [`crate::plugins`].

pub use eumeaus_plugin_host::trust_store::TrustedKey;

use crate::EngineError;

pub fn add(name: &str, public_key_hex: &str) -> Result<(), EngineError> {
    Ok(eumeaus_plugin_host::trust_store::add(name, public_key_hex)?)
}

/// Every trusted key, alphabetical by name.
pub fn list() -> Result<Vec<TrustedKey>, EngineError> {
    Ok(eumeaus_plugin_host::trust_store::list()?)
}

/// Not an error if `name` didn't exist.
pub fn remove(name: &str) -> Result<(), EngineError> {
    Ok(eumeaus_plugin_host::trust_store::remove(name)?)
}

/// Looks up `name`, for building a `TrustPolicy::RequireSignature` without
/// the caller needing to know a raw hex key.
pub fn resolve(name: &str) -> Result<ed25519_dalek::VerifyingKey, EngineError> {
    Ok(eumeaus_plugin_host::trust_store::resolve(name)?)
}
