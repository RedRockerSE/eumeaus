//! GUI-only local settings (SPEC.md §9.3, the exploratory test's §4.1
//! finding: re-typing the plugins directory on every scan/plugin-list
//! call is real friction). Not an `eumeaus-engine` or CLI concern —
//! plugins aren't case-scoped anywhere else in this project either
//! (`plugin_state.rs`'s own module doc), so this is a GUI-local, OS-user-
//! wide convenience file, same "not a secret, plain file" reasoning
//! `eumeaus-plugin-host`'s `trust_store.rs` already uses for trusted
//! keys — just for one path string instead of a list of keys.
//!
//! Deliberately its own settings file (`gui_settings.toml`), not folded
//! into `trusted_keys.toml`: unrelated concerns that happen to share a
//! config directory shouldn't share a file/schema.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default)]
    plugins_dir: Option<String>,
}

/// `EUMEAUS_GUI_SETTINGS_PATH` overrides the default location — used by
/// tests (so they never touch a real developer's `~/.config/eumeaus/`),
/// same convention as `trust_store.rs`'s `EUMEAUS_TRUST_STORE_PATH`.
fn settings_path() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("EUMEAUS_GUI_SETTINGS_PATH") {
        return Ok(PathBuf::from(path));
    }
    let dir = dirs::config_dir()
        .ok_or_else(|| "could not determine the OS config directory".to_string())?
        .join("eumeaus");
    Ok(dir.join("gui_settings.toml"))
}

fn load(path: &Path) -> Result<SettingsFile, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("invalid {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SettingsFile::default()),
        Err(e) => Err(format!("reading {}: {e}", path.display())),
    }
}

fn save(path: &Path, settings: &SettingsFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    let text = toml::to_string_pretty(settings).expect("SettingsFile always serializes");
    std::fs::write(path, text).map_err(|e| format!("writing {}: {e}", path.display()))
}

fn do_settings_get_plugins_dir() -> Result<Option<String>, String> {
    Ok(load(&settings_path()?)?.plugins_dir)
}

fn do_settings_set_plugins_dir(dir: &str) -> Result<(), String> {
    let path = settings_path()?;
    let mut settings = load(&path)?;
    settings.plugins_dir = Some(dir.to_string());
    save(&path, &settings)
}

#[tauri::command]
pub async fn settings_get_plugins_dir() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(do_settings_get_plugins_dir)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn settings_set_plugins_dir(dir: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || do_settings_set_plugins_dir(&dir))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct SettingsGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        _dir: tempfile::TempDir,
    }

    impl Drop for SettingsGuard {
        fn drop(&mut self) {
            // SAFETY: serialized by ENV_LOCK, held for this guard's whole
            // lifetime.
            unsafe {
                std::env::remove_var("EUMEAUS_GUI_SETTINGS_PATH");
            }
        }
    }

    fn temp_settings() -> SettingsGuard {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui_settings.toml");
        // SAFETY: serialized by ENV_LOCK, held until the returned guard
        // drops.
        unsafe {
            std::env::set_var("EUMEAUS_GUI_SETTINGS_PATH", &path);
        }
        SettingsGuard {
            _lock: lock,
            _dir: dir,
        }
    }

    #[test]
    fn get_on_a_fresh_settings_file_is_none_not_an_error() {
        let _guard = temp_settings();
        assert_eq!(do_settings_get_plugins_dir().unwrap(), None);
    }

    #[test]
    fn set_then_get_round_trips() {
        let _guard = temp_settings();
        do_settings_set_plugins_dir("/home/investigator/plugins").unwrap();
        assert_eq!(
            do_settings_get_plugins_dir().unwrap(),
            Some("/home/investigator/plugins".to_string())
        );
    }

    #[test]
    fn setting_it_again_overwrites_not_duplicates() {
        let _guard = temp_settings();
        do_settings_set_plugins_dir("/first").unwrap();
        do_settings_set_plugins_dir("/second").unwrap();
        assert_eq!(
            do_settings_get_plugins_dir().unwrap(),
            Some("/second".to_string())
        );
    }
}
