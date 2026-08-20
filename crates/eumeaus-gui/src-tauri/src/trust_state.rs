//! G5 (SPEC.md §9.6): local trust store management — wraps
//! `eumeaus_engine::trust::add`/`list`/`remove`, the same calls
//! `eumeaus-cli`'s `trust add`/`list`/`remove` make (SPEC.md §8 open
//! question 2). A plain-file store of named public keys, not a secret —
//! doesn't touch the OS keychain (`credential_state.rs`'s domain) or a
//! case (`case_state::AppState`'s).

use eumeaus_engine::trust::{self, TrustedKey};
use serde::Serialize;

#[derive(Serialize, Debug)]
pub struct TrustedKeyDto {
    pub name: String,
    pub public_key: String,
}

impl From<TrustedKey> for TrustedKeyDto {
    fn from(k: TrustedKey) -> Self {
        TrustedKeyDto {
            name: k.name,
            public_key: k.public_key,
        }
    }
}

fn do_trust_add(name: &str, public_key: &str) -> Result<(), String> {
    trust::add(name, public_key).map_err(|e| e.to_string())
}

fn do_trust_list() -> Result<Vec<TrustedKeyDto>, String> {
    trust::list()
        .map(|keys| keys.into_iter().map(TrustedKeyDto::from).collect())
        .map_err(|e| e.to_string())
}

fn do_trust_remove(name: &str) -> Result<(), String> {
    trust::remove(name).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn trust_add(name: String, public_key: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || do_trust_add(&name, &public_key))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn trust_list() -> Result<Vec<TrustedKeyDto>, String> {
    tauri::async_runtime::spawn_blocking(do_trust_list)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn trust_remove(name: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || do_trust_remove(&name))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real trust-store file round-trip (a local plain file, unlike
    // credential_state.rs's keychain — see trust_store.rs's own doc for
    // why these aren't treated as secrets). Cleans up after itself.
    #[test]
    fn add_list_remove_round_trips_through_the_real_trust_store() {
        let name = "eumeaus-gui-test-trust-g5";
        // A syntactically valid 32-byte Ed25519 public key, hex-encoded —
        // trust::add doesn't validate it's on-curve, only well-formed.
        let public_key = "aa".repeat(32);

        let result = (|| -> Result<(), String> {
            do_trust_add(name, &public_key)?;
            let listed = do_trust_list()?;
            assert!(listed
                .iter()
                .any(|k| k.name == name && k.public_key == public_key));
            Ok(())
        })();
        let _ = do_trust_remove(name);
        result.unwrap();
    }
}
