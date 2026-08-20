//! G3 (SPEC.md §9.6): scan run + live progress. Wraps `Case::create_scan`
//! plus the new `Case::resume_scan_with_progress`, added for this
//! milestone. See that method's doc in eumeaus-engine for why a plain
//! `resume_scan` plus polling doesn't work here: it holds the case's one
//! connection for the whole scan.
//!
//! `scan_run` returns as soon as the scan is *created* (same
//! create-then-resume split `eumeaus-cli`'s `scan run` already uses, so
//! the id is known even if this window closes mid-scan), then runs the
//! actual scan in a background task. That task owns the progress
//! channel's receiver end and forwards each event to the frontend via a
//! Tauri event (`scan-progress`) as it arrives — the receiver runs
//! independently of the `Mutex`-guarded `Case`, so it isn't blocked by
//! the scan holding that lock.

use std::path::Path;
use std::sync::{Arc, Mutex};

use eumeaus_engine::{
    Case, EntityType, PluginRef, ScanConfig, ScanId, ScanProgressEvent, TargetEntity, TrustPolicy,
};
use serde::Serialize;
use tauri::Emitter;

use crate::case_state::AppState;

const NO_CASE_OPEN: &str = "no case is currently open — open a case first";
const SCAN_PROGRESS_EVENT: &str = "scan-progress";

#[derive(Serialize, Clone, Debug)]
pub struct ScanProgressDto {
    pub scan_id: String,
    pub plugin_name: String,
    pub status: String,
    pub error_message: Option<String>,
}

impl From<ScanProgressEvent> for ScanProgressDto {
    fn from(e: ScanProgressEvent) -> Self {
        ScanProgressDto {
            scan_id: e.scan_id.to_string(),
            plugin_name: e.plugin_name,
            status: e.status,
            error_message: e.error_message,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct ScanSummaryDto {
    pub id: String,
    pub status: String,
    pub target_entity_id: String,
    pub started_at_unix_ms: Option<i64>,
    pub completed_at_unix_ms: Option<i64>,
}

impl From<eumeaus_engine::ScanSummary> for ScanSummaryDto {
    fn from(s: eumeaus_engine::ScanSummary) -> Self {
        ScanSummaryDto {
            id: s.id.to_string(),
            status: s.status.to_string(),
            target_entity_id: s.target_entity_id.to_string(),
            started_at_unix_ms: s.started_at_unix_ms,
            completed_at_unix_ms: s.completed_at_unix_ms,
        }
    }
}

fn do_scan_create(
    cell: &Arc<Mutex<Option<Case>>>,
    plugins_dir: &Path,
    plugin_names: Vec<String>,
    target_type: &str,
    target_value: &str,
) -> Result<ScanId, String> {
    let mut guard = cell.lock().unwrap();
    let case = guard.as_mut().ok_or(NO_CASE_OPEN)?;

    // EntityType::from_str is Infallible (SPEC.md §4.3's Custom escape
    // hatch) — same reasoning as entity_state.rs's do_entity_list.
    let entity_type: EntityType = target_type.parse().expect("infallible");
    let target = case
        .find_entity_by_key(entity_type, target_value)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            format!("no entity found with type {target_type:?}, key {target_value:?}")
        })?;

    let plugins = plugin_names
        .into_iter()
        .map(|name| PluginRef { name })
        .collect();
    // Trust policy is a v1 CLI concern the GUI doesn't expose UI for yet
    // (SPEC.md §8.2's local trust store) — AllowUnsigned matches the
    // CLI's own default when neither --trusted-key nor --trust is given.
    case.create_scan(
        plugins_dir,
        plugins,
        TargetEntity { id: target.id },
        ScanConfig::default(),
        &TrustPolicy::AllowUnsigned,
    )
    .map_err(|e| e.to_string())
}

fn do_scan_resume(
    cell: &Arc<Mutex<Option<Case>>>,
    scan_id: ScanId,
    progress: &eumeaus_engine::ScanProgressSender,
) -> Result<(), String> {
    let mut guard = cell.lock().unwrap();
    let case = guard.as_mut().ok_or(NO_CASE_OPEN)?;
    case.resume_scan_with_progress(scan_id, progress)
        .map_err(|e| e.to_string())
}

fn do_scan_list(cell: &Arc<Mutex<Option<Case>>>) -> Result<Vec<ScanSummaryDto>, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    case.list_scans()
        .map(|scans| scans.into_iter().map(ScanSummaryDto::from).collect())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn scan_run(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    plugins_dir: String,
    plugin: Vec<String>,
    target_type: String,
    target_value: String,
) -> Result<String, String> {
    let cell = state.0.clone();
    let scan_id = tauri::async_runtime::spawn_blocking(move || {
        do_scan_create(
            &cell,
            Path::new(&plugins_dir),
            plugin,
            &target_type,
            &target_value,
        )
    })
    .await
    .map_err(|e| e.to_string())??;

    let cell_for_resume = state.0.clone();
    tauri::async_runtime::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ScanProgressEvent>();

        let forward = tauri::async_runtime::spawn(async move {
            while let Some(event) = rx.recv().await {
                let _ = app.emit(SCAN_PROGRESS_EVENT, ScanProgressDto::from(event));
            }
        });

        // tx moves in here and drops when the closure returns, which is
        // what lets forward's `rx.recv()` loop above see the end of the
        // scan and exit.
        let _ = tauri::async_runtime::spawn_blocking(move || {
            do_scan_resume(&cell_for_resume, scan_id, &tx)
        })
        .await;

        let _ = forward.await;
    });

    Ok(scan_id.to_string())
}

#[tauri::command]
pub async fn scan_list(state: tauri::State<'_, AppState>) -> Result<Vec<ScanSummaryDto>, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_scan_list(&cell))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use eumeaus_engine::Provenance;

    fn manual_provenance() -> Provenance {
        Provenance {
            source: "user".to_string(),
            source_version: "0.1.0".to_string(),
            source_url: None,
            retrieval_method: None,
            raw_response_sha256: None,
            collected_at_unix_ms: 0,
        }
    }

    #[test]
    fn create_and_list_error_cleanly_with_no_case_open() {
        let cell: Arc<Mutex<Option<Case>>> = Arc::new(Mutex::new(None));
        let err =
            do_scan_create(&cell, Path::new("/nonexistent"), vec![], "Username", "x").unwrap_err();
        assert_eq!(err, NO_CASE_OPEN);
        assert_eq!(do_scan_list(&cell).unwrap_err(), NO_CASE_OPEN);
    }

    #[test]
    fn create_errors_when_target_entity_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "g3-no-target").unwrap();
        let cell = Arc::new(Mutex::new(Some(case)));

        let err = do_scan_create(&cell, dir.path(), vec![], "Username", "nobody").unwrap_err();
        assert!(err.contains("no entity found"));
    }

    // No real plugin fixture available from this crate (those live in
    // eumeaus-engine's own scan.rs tests, built as engine-crate examples
    // — see plugin-development's note on fixtures being duplicated per
    // crate rather than shared, not shared cross-crate). What's testable
    // here without one: create_scan correctly surfaces engine's own
    // NoCompatiblePlugins error (an empty plugins_dir discovers nothing)
    // as a plain string, and list_scans' data shape / do_* wiring is
    // otherwise identical to entity_state.rs's already-tested pattern.
    // The live per-plugin-progress path itself is exercised by
    // eumeaus-engine's own resume_scan_with_progress test (a real
    // scan_ok fixture plugin) and by G3's live tauri dev verification
    // against the real username-search plugin.
    #[test]
    fn create_surfaces_no_compatible_plugins_as_a_plain_error_string() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g3-no-plugins").unwrap();
        case.add_entity(
            EntityType::Username,
            Some("dave".to_string()),
            vec![],
            manual_provenance(),
        )
        .unwrap();
        let cell = Arc::new(Mutex::new(Some(case)));
        let empty_plugins_dir = dir.path().join("plugins");

        let err =
            do_scan_create(&cell, &empty_plugins_dir, vec![], "Username", "dave").unwrap_err();
        assert!(err.contains("Username"));

        // Nothing was created — list_scans stays empty.
        assert!(do_scan_list(&cell).unwrap().is_empty());
    }
}
