//! Credential storage (SPEC.md §4.5, §7 M6): OS-native credential store,
//! app-scoped, referenced by logical name (e.g. `"twitter_api_key"`).
//! Never stored in the case file; never passed to a plugin subprocess via
//! argv or environment variables (both visible to other processes on the
//! same machine) — only injected into the gRPC request body, by
//! [`resolve_credentials`], at invocation time.
//!
//! Deliberately global to the OS user account, not scoped to a case:
//! SPEC.md's wording ("an app-scoped namespace") and CLI surface
//! (`credential set/list/remove`, no `--case`) both point the same way —
//! an API key is a property of the investigator's environment, not of one
//! investigation.

use std::collections::HashMap;

use crate::{manifest::PluginManifest, PluginError};

const SERVICE: &str = "eumeaus-credentials";
/// A reserved name a real credential can never collide with (`Entry::new`
/// only rejects an empty username, not a hyphen-and-underscore one)  —
/// holds the JSON-encoded list of credential names this store has, since
/// OS keychains have no "list entries for this service" API of their own.
const INDEX_ENTRY: &str = "__eumeaus_credential_index__";

fn entry(name: &str) -> Result<keyring::Entry, PluginError> {
    keyring::Entry::new(SERVICE, name).map_err(|e| PluginError::Credential(e.to_string()))
}

fn load_index() -> Result<Vec<String>, PluginError> {
    match entry(INDEX_ENTRY)?.get_password() {
        Ok(json) => serde_json::from_str(&json)
            .map_err(|e| PluginError::Credential(format!("corrupt credential index: {e}"))),
        Err(keyring::Error::NoEntry) => Ok(Vec::new()),
        Err(e) => Err(PluginError::Credential(e.to_string())),
    }
}

fn save_index(names: &[String]) -> Result<(), PluginError> {
    let json = serde_json::to_string(names).expect("Vec<String> always serializes");
    entry(INDEX_ENTRY)?
        .set_password(&json)
        .map_err(|e| PluginError::Credential(e.to_string()))
}

/// Stores (or overwrites) a credential under `name`.
pub fn set(name: &str, value: &str) -> Result<(), PluginError> {
    entry(name)?
        .set_password(value)
        .map_err(|e| PluginError::Credential(e.to_string()))?;

    let mut names = load_index()?;
    if !names.iter().any(|n| n == name) {
        names.push(name.to_string());
        names.sort();
        save_index(&names)?;
    }
    Ok(())
}

/// Names of every credential currently stored (not their values).
pub fn list() -> Result<Vec<String>, PluginError> {
    load_index()
}

/// Removes a credential. Not an error if it didn't exist.
pub fn remove(name: &str) -> Result<(), PluginError> {
    match entry(name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(PluginError::Credential(e.to_string())),
    }

    let mut names = load_index()?;
    let before = names.len();
    names.retain(|n| n != name);
    if names.len() != before {
        save_index(&names)?;
    }
    Ok(())
}

fn get(name: &str) -> Result<Option<String>, PluginError> {
    match entry(name)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(PluginError::Credential(e.to_string())),
    }
}

/// Looks up every credential `manifest.permissions.requested_credentials`
/// names, for injection into that plugin's `CheckRequest.resolved_credentials`
/// (SPEC.md §4.5). A name the store doesn't have is a hard error — the
/// plugin declared it needs this to function, so running it without is
/// worse than not running it; the caller is expected to turn this into a
/// per-plugin `scan_plugin_runs` `ERROR` rather than letting one plugin's
/// missing credential abort the whole scan (SPEC.md §5).
///
/// Deliberately synchronous, and expected to be called *before* entering
/// any async runtime the caller might use for the actual plugin
/// invocation: `keyring`'s Secret Service backend does its own internal
/// async D-Bus work, which — like a plugin's blocking HTTP client (see the
/// `plugin-development` skill) — must never run nested inside an
/// already-running one.
pub fn resolve_credentials(
    manifest: &PluginManifest,
) -> Result<HashMap<String, String>, PluginError> {
    let mut resolved = HashMap::new();
    for name in &manifest.permissions.requested_credentials {
        let value = get(name)?.ok_or_else(|| {
            PluginError::MissingCredential(name.clone(), manifest.plugin.name.clone())
        })?;
        resolved.insert(name.clone(), value);
    }
    Ok(resolved)
}
