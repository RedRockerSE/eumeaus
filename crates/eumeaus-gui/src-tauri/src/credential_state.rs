//! G5 (SPEC.md §9.6): credential management — wraps
//! `eumeaus_engine::credentials::set`/`list`/`remove`, the same calls
//! `eumeaus-cli`'s `credential set`/`list`/`remove` make. Global to the
//! OS user account, not case-scoped (CLAUDE.md), so — like
//! `plugin_state` — these don't touch `case_state::AppState`.
//!
//! `credential set`'s CLI form uses `rpassword`'s interactive TTY prompt
//! specifically so the secret never appears in shell history or `ps`
//! output; a GUI has no shell to leak into in the first place, so a
//! normal password `<input>` submitted through `invoke()` is the correct
//! GUI-native equivalent, not a shortcut around that concern (SPEC.md
//! §9.3 already calls this out as the same kind of
//! CLI-vs-GUI-native-ergonomics call `entity_state.rs`'s attribute
//! shape is).

use eumeaus_engine::credentials;

fn do_credential_set(name: &str, value: &str) -> Result<(), String> {
    credentials::set(name, value).map_err(|e| e.to_string())
}

fn do_credential_list() -> Result<Vec<String>, String> {
    credentials::list().map_err(|e| e.to_string())
}

fn do_credential_remove(name: &str) -> Result<(), String> {
    credentials::remove(name).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn credential_set(name: String, value: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || do_credential_set(&name, &value))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn credential_list() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(do_credential_list)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn credential_remove(name: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || do_credential_remove(&name))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real OS keychain round-trip (this sandbox has an unlocked Secret
    // Service — same CLAUDE.md gotcha eumeaus-engine's own keychain tests
    // rely on). Cleans up after itself regardless of pass/fail so it
    // doesn't leak a stray credential into the OS keychain across runs.
    #[test]
    fn set_list_remove_round_trips_through_the_real_keychain() {
        let name = "eumeaus-gui-test-credential-g5";
        let result = (|| -> Result<(), String> {
            do_credential_set(name, "s3cret")?;
            let listed = do_credential_list()?;
            assert!(listed.iter().any(|n| n == name));
            Ok(())
        })();
        let _ = do_credential_remove(name);
        result.unwrap();
    }
}
