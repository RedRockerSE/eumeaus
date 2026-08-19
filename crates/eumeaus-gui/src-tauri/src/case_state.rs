//! G1 (SPEC.md §9.6): case lifecycle from the GUI, backed by the real
//! keychain + SQLCipher path (§4) — not a mock. One `Case` lives behind a
//! blocking `Mutex` in Tauri's managed state (§9.2's design): commands run
//! the actual blocking `Case::create`/`open`/`close`/`list` calls on a
//! blocking-pool thread via `tauri::async_runtime::spawn_blocking` rather
//! than inside the async command body directly, same reasoning as
//! eumeaus-engine's own scan.rs bridge — these are sync, file-locking,
//! keychain-touching calls, not async I/O.
//!
//! Each `#[tauri::command]` is a thin wrapper around a plain, directly
//! testable `do_*` function — `tauri::State`/`spawn_blocking` aren't
//! exercised by the unit tests below, only the state-machine logic
//! (reject a second concurrent case, `close` clears it, `list` doesn't
//! touch state) is.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eumeaus_engine::Case;
use serde::Serialize;

/// A GUI text field never passes through a shell, so unlike a CLI arg a
/// leading `~` is never expanded before it reaches us — discovered live
/// (a directory literally named `~` got created during G1 testing).
/// `dirs::home_dir()` is already a workspace dependency (used by
/// `trust_store.rs`).
fn expand_home(raw: &str) -> PathBuf {
    match raw
        .strip_prefix("~/")
        .or_else(|| (raw == "~").then_some(""))
    {
        Some(rest) => dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(raw)),
        None => PathBuf::from(raw),
    }
}

pub struct AppState(pub Arc<Mutex<Option<Case>>>);

impl Default for AppState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

#[derive(Serialize, Debug)]
pub struct CaseInfo {
    pub id: String,
    pub name: String,
    pub path: String,
}

impl From<&Case> for CaseInfo {
    fn from(case: &Case) -> Self {
        CaseInfo {
            id: case.id().to_string(),
            name: case.name().to_string(),
            path: case.path().display().to_string(),
        }
    }
}

fn do_case_create(
    cell: &Arc<Mutex<Option<Case>>>,
    dir: &Path,
    name: &str,
) -> Result<CaseInfo, String> {
    let mut guard = cell.lock().unwrap();
    if guard.is_some() {
        return Err("a case is already open in this window — close it first".to_string());
    }
    let case = Case::create(dir, name).map_err(|e| e.to_string())?;
    let info = CaseInfo::from(&case);
    *guard = Some(case);
    Ok(info)
}

fn do_case_open(cell: &Arc<Mutex<Option<Case>>>, path: &Path) -> Result<CaseInfo, String> {
    let mut guard = cell.lock().unwrap();
    if guard.is_some() {
        return Err("a case is already open in this window — close it first".to_string());
    }
    let case = Case::open(path).map_err(|e| e.to_string())?;
    let info = CaseInfo::from(&case);
    *guard = Some(case);
    Ok(info)
}

fn do_case_close(cell: &Arc<Mutex<Option<Case>>>) -> Result<(), String> {
    let mut guard = cell.lock().unwrap();
    match guard.take() {
        Some(case) => case.close().map_err(|e| e.to_string()),
        None => Err("no case is currently open".to_string()),
    }
}

fn do_case_current(cell: &Arc<Mutex<Option<Case>>>) -> Option<CaseInfo> {
    cell.lock().unwrap().as_ref().map(CaseInfo::from)
}

fn do_case_list(dir: &Path) -> Result<Vec<CaseInfo>, String> {
    Case::list(dir)
        .map(|summaries| {
            summaries
                .into_iter()
                .map(|s| CaseInfo {
                    id: s.id.to_string(),
                    name: s.name,
                    path: s.path.display().to_string(),
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn case_create(
    state: tauri::State<'_, AppState>,
    dir: String,
    name: String,
) -> Result<CaseInfo, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_case_create(&cell, &expand_home(&dir), &name))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn case_open(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<CaseInfo, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_case_open(&cell, &expand_home(&path)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn case_close(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_case_close(&cell))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn case_current(state: tauri::State<'_, AppState>) -> Result<Option<CaseInfo>, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_case_current(&cell))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn case_list(dir: String) -> Result<Vec<CaseInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || do_case_list(&expand_home(&dir)))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_cell() -> Arc<Mutex<Option<Case>>> {
        Arc::new(Mutex::new(None))
    }

    #[test]
    fn expand_home_handles_bare_tilde_and_tilde_slash_only() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("~/Desktop"), home.join("Desktop"));
        // Not a home-relative path — left untouched, same as a shell
        // would leave "~user" or "~foo" alone without expansion here too.
        assert_eq!(expand_home("~foo"), PathBuf::from("~foo"));
        assert_eq!(expand_home("/tmp/x"), PathBuf::from("/tmp/x"));
    }

    #[test]
    fn create_then_close_round_trips_and_frees_the_slot() {
        let dir = tempfile::tempdir().unwrap();
        let cell = tmp_cell();

        let info = do_case_create(&cell, dir.path(), "gui-case").unwrap();
        assert_eq!(info.name, "gui-case");
        assert!(cell.lock().unwrap().is_some());

        do_case_close(&cell).unwrap();
        assert!(cell.lock().unwrap().is_none());
    }

    #[test]
    fn create_rejects_a_second_case_while_one_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let cell = tmp_cell();
        do_case_create(&cell, dir.path(), "first").unwrap();

        let err = do_case_create(&cell, dir.path(), "second").unwrap_err();
        assert!(err.contains("already open"));
    }

    #[test]
    fn close_with_nothing_open_errors_instead_of_silently_succeeding() {
        let cell = tmp_cell();
        let err = do_case_close(&cell).unwrap_err();
        assert!(err.contains("no case is currently open"));
    }

    #[test]
    fn current_reflects_open_and_closed_state() {
        let dir = tempfile::tempdir().unwrap();
        let cell = tmp_cell();
        assert!(do_case_current(&cell).is_none());

        do_case_create(&cell, dir.path(), "watched").unwrap();
        assert_eq!(do_case_current(&cell).unwrap().name, "watched");

        do_case_close(&cell).unwrap();
        assert!(do_case_current(&cell).is_none());
    }

    // The real cross-client proof — a case created here (via the exact
    // do_case_create path the GUI command calls) opens correctly through
    // eumeaus-engine's own Case::open directly, i.e. what eumeaus-cli
    // itself calls — SPEC.md §9.6 G1's verification bar.
    #[test]
    fn gui_created_case_opens_via_the_plain_engine_api_like_the_cli_does() {
        let dir = tempfile::tempdir().unwrap();
        let cell = tmp_cell();
        let info = do_case_create(&cell, dir.path(), "cli-compat").unwrap();
        do_case_close(&cell).unwrap();

        let reopened = Case::open(Path::new(&info.path)).unwrap();
        assert_eq!(reopened.name(), "cli-compat");
        assert_eq!(reopened.id().to_string(), info.id);
    }

    #[test]
    fn list_does_not_require_or_touch_open_state() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "listed").unwrap();
        case.close().unwrap();

        let found = do_case_list(dir.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "listed");
    }
}
