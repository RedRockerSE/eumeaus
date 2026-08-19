//! Local trust store (SPEC.md §8 open question 2): named Ed25519 public
//! keys the investigator has explicitly decided to trust, so `scan run
//! --trust <name>` / `plugin verify --trust <name>` don't require
//! retyping a 64-hex-character key every invocation — `--trusted-key
//! <hex>` still works too, unchanged.
//!
//! v1 has no baked-in "official" trusted key. SPEC.md §1 already rules out
//! a plugin marketplace/registry for v1, and this is a solo project with
//! no real key-custody process — inventing a fake first-party signing
//! authority and baking its public key into the binary would be security
//! theater, not a real answer to §8's question. Instead: the investigator
//! *is* the signing authority in v1, for whichever keys they've explicitly
//! added here. Third-party plugin trust distribution remains open, since
//! there's no third-party plugin ecosystem yet for it to serve.
//!
//! A plain TOML file, not the OS keychain `credentials.rs` uses: public
//! keys aren't secrets, so there's nothing gained by locking them behind
//! Secret Service — and it means testing this needs no running keyring
//! daemon (unlike the credential-store tests, see CLAUDE.md's keychain
//! gotcha).

use std::path::{Path, PathBuf};

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

use crate::PluginError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedKey {
    pub name: String,
    /// Hex-encoded 32-byte Ed25519 public key.
    pub public_key: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustStoreFile {
    #[serde(default)]
    keys: Vec<TrustedKey>,
}

/// `EUMEAUS_TRUST_STORE_PATH` overrides the default location — used by
/// tests (so they never touch a real developer's `~/.config/eumeaus/`) and
/// available to a user who wants the store somewhere else.
fn store_path() -> Result<PathBuf, PluginError> {
    if let Ok(path) = std::env::var("EUMEAUS_TRUST_STORE_PATH") {
        return Ok(PathBuf::from(path));
    }
    let dir = dirs::config_dir()
        .ok_or_else(|| {
            PluginError::TrustStore("could not determine the OS config directory".to_string())
        })?
        .join("eumeaus");
    Ok(dir.join("trusted_keys.toml"))
}

fn load(path: &Path) -> Result<TrustStoreFile, PluginError> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| PluginError::TrustStore(format!("invalid {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TrustStoreFile::default()),
        Err(e) => Err(PluginError::TrustStore(format!(
            "reading {}: {e}",
            path.display()
        ))),
    }
}

fn save(path: &Path, store: &TrustStoreFile) -> Result<(), PluginError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| PluginError::TrustStore(format!("creating {}: {e}", parent.display())))?;
    }
    let text = toml::to_string_pretty(store).expect("TrustStoreFile always serializes");
    std::fs::write(path, text)
        .map_err(|e| PluginError::TrustStore(format!("writing {}: {e}", path.display())))
}

/// Validates a key at `add` time (64 hex chars, a valid Ed25519 point), so
/// a typo is caught here rather than silently later at `scan run --trust
/// <name>`.
fn parse_hex_key(public_key_hex: &str) -> Result<VerifyingKey, PluginError> {
    if public_key_hex.len() != 64 {
        return Err(PluginError::TrustStore(
            "public key must be exactly 64 hex characters (32 bytes)".to_string(),
        ));
    }
    let mut bytes = [0u8; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&public_key_hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| PluginError::TrustStore(format!("invalid hex: {e}")))?;
    }
    VerifyingKey::from_bytes(&bytes)
        .map_err(|e| PluginError::TrustStore(format!("invalid Ed25519 public key: {e}")))
}

/// Adds (or, re-adding the same name, overwrites) a trusted key.
pub fn add(name: &str, public_key_hex: &str) -> Result<(), PluginError> {
    parse_hex_key(public_key_hex)?;

    let path = store_path()?;
    let mut store = load(&path)?;
    store.keys.retain(|k| k.name != name);
    store.keys.push(TrustedKey {
        name: name.to_string(),
        public_key: public_key_hex.to_lowercase(),
    });
    store.keys.sort_by(|a, b| a.name.cmp(&b.name));
    save(&path, &store)
}

/// Every trusted key, alphabetical by name.
pub fn list() -> Result<Vec<TrustedKey>, PluginError> {
    Ok(load(&store_path()?)?.keys)
}

/// Not an error if `name` didn't exist — same policy as
/// `credentials::remove`.
pub fn remove(name: &str) -> Result<(), PluginError> {
    let path = store_path()?;
    let mut store = load(&path)?;
    store.keys.retain(|k| k.name != name);
    save(&path, &store)
}

/// Looks up `name` for `TrustPolicy::RequireSignature`.
pub fn resolve(name: &str) -> Result<VerifyingKey, PluginError> {
    let key = list()?
        .into_iter()
        .find(|k| k.name == name)
        .ok_or_else(|| {
            PluginError::TrustStore(format!(
                "no trusted key named {name:?} (see `eumeaus trust list`)"
            ))
        })?;
    parse_hex_key(&key.public_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Every test here mutates the process-global
    /// `EUMEAUS_TRUST_STORE_PATH` env var, which races across `cargo
    /// test`'s default parallel test threads if unguarded — serializes
    /// them against each other. Same pattern as
    /// eumeaus-username-search-plugin's `sites.toml` tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// A fresh, real Ed25519 public key, hex-encoded — not every 32-byte
    /// string decompresses to a valid curve point, so a hand-crafted
    /// repeated-byte pattern isn't good enough here (`parse_hex_key`
    /// actually validates the point, same as production code would reject
    /// a typo'd `trust add`).
    fn sample_key() -> String {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;
        let signing_key = SigningKey::generate(&mut OsRng);
        signing_key
            .verifying_key()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    struct StoreGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        _dir: tempfile::TempDir,
    }

    impl Drop for StoreGuard {
        fn drop(&mut self) {
            // SAFETY: serialized by ENV_LOCK, held for this guard's whole
            // lifetime (it's constructed before the env var is set, and
            // dropped — releasing the lock — only after this runs).
            unsafe {
                std::env::remove_var("EUMEAUS_TRUST_STORE_PATH");
            }
        }
    }

    fn temp_store() -> StoreGuard {
        // A panic in one test must not poison this lock for every test
        // that runs after it.
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trusted_keys.toml");
        // SAFETY: serialized by ENV_LOCK, held until the returned guard
        // drops.
        unsafe {
            std::env::set_var("EUMEAUS_TRUST_STORE_PATH", &path);
        }
        StoreGuard {
            _lock: lock,
            _dir: dir,
        }
    }

    #[test]
    fn list_on_a_fresh_store_is_empty_not_an_error() {
        let _guard = temp_store();
        assert!(list().unwrap().is_empty());
    }

    #[test]
    fn add_then_list_then_remove_round_trips() {
        let _guard = temp_store();

        add("alice", &sample_key()).unwrap();
        add("bob", &sample_key()).unwrap();

        let keys = list().unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].name, "alice");
        assert_eq!(keys[1].name, "bob");

        remove("alice").unwrap();
        let keys = list().unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].name, "bob");
    }

    #[test]
    fn re_adding_the_same_name_overwrites_it() {
        let _guard = temp_store();

        add("alice", &sample_key()).unwrap();
        let second_key = sample_key();
        add("alice", &second_key).unwrap();

        let keys = list().unwrap();
        assert_eq!(keys.len(), 1, "re-adding must overwrite, not duplicate");
        assert_eq!(keys[0].public_key, second_key);
    }

    #[test]
    fn removing_a_name_that_never_existed_is_not_an_error() {
        let _guard = temp_store();
        remove("nobody").unwrap();
    }

    #[test]
    fn add_rejects_a_malformed_key() {
        let _guard = temp_store();
        let err = add("bad", "not-hex-at-all").unwrap_err();
        assert!(matches!(err, PluginError::TrustStore(_)));
        assert!(
            list().unwrap().is_empty(),
            "a rejected add must not be stored"
        );
    }

    #[test]
    fn resolve_finds_an_added_key_and_errors_on_an_unknown_name() {
        let _guard = temp_store();
        let hex = sample_key();
        add("carol", &hex).unwrap();

        let resolved = resolve("carol").unwrap();
        assert_eq!(resolved, parse_hex_key(&hex).unwrap());

        let err = resolve("nobody").unwrap_err();
        assert!(matches!(err, PluginError::TrustStore(_)));
    }
}
