//! GUI UX redesign (Claude Design handover, SPEC.md §9.3's Overview
//! screen): case-wide stats and a mixed audit feed. Wraps
//! `Case::case_stats`/`audit_trail_all`, added to eumeaus-engine
//! specifically for this screen — no CLI command surfaces either today.

use std::sync::{Arc, Mutex};

use eumeaus_engine::{AuditEvent, Case, CaseStats};
use serde::Serialize;

use crate::case_state::AppState;

const NO_CASE_OPEN: &str = "no case is currently open — open a case first";

#[derive(Serialize, Debug)]
pub struct CaseStatsDto {
    pub entity_count: i64,
    pub fact_count: i64,
    pub relationship_count: i64,
    pub conflicting_entity_count: i64,
}

impl From<CaseStats> for CaseStatsDto {
    fn from(s: CaseStats) -> Self {
        CaseStatsDto {
            entity_count: s.entity_count,
            fact_count: s.fact_count,
            relationship_count: s.relationship_count,
            conflicting_entity_count: s.conflicting_entity_count,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct AuditEventDto {
    pub id: String,
    pub event_type: String,
    pub description: String,
    pub actor: String,
    pub occurred_at_unix_ms: i64,
}

impl From<AuditEvent> for AuditEventDto {
    fn from(e: AuditEvent) -> Self {
        AuditEventDto {
            id: e.id.to_string(),
            event_type: e.event_type,
            description: e.description,
            actor: e.actor,
            occurred_at_unix_ms: e.occurred_at_unix_ms,
        }
    }
}

fn do_case_stats(cell: &Arc<Mutex<Option<Case>>>) -> Result<CaseStatsDto, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    case.case_stats()
        .map(CaseStatsDto::from)
        .map_err(|e| e.to_string())
}

fn do_audit_list(
    cell: &Arc<Mutex<Option<Case>>>,
    limit: u32,
) -> Result<Vec<AuditEventDto>, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    case.audit_trail_all(limit)
        .map(|events| events.into_iter().map(AuditEventDto::from).collect())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn case_stats(state: tauri::State<'_, AppState>) -> Result<CaseStatsDto, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_case_stats(&cell))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn audit_list(
    state: tauri::State<'_, AppState>,
    limit: u32,
) -> Result<Vec<AuditEventDto>, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_audit_list(&cell, limit))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use eumeaus_engine::{Actor, Attribute, EntityType, Provenance};

    fn tmp_cell_with_case(case: Case) -> Arc<Mutex<Option<Case>>> {
        Arc::new(Mutex::new(Some(case)))
    }

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
    fn stats_and_audit_error_cleanly_with_no_case_open() {
        let cell: Arc<Mutex<Option<Case>>> = Arc::new(Mutex::new(None));
        assert_eq!(do_case_stats(&cell).unwrap_err(), NO_CASE_OPEN);
        assert_eq!(do_audit_list(&cell, 10).unwrap_err(), NO_CASE_OPEN);
    }

    #[test]
    fn case_stats_reflects_entities_added_via_the_real_engine_api() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-overview-stats").unwrap();
        case.add_entity(
            EntityType::Person,
            None,
            vec![Attribute {
                key: "name".to_string(),
                value: "Alice".to_string(),
            }],
            manual_provenance(),
        )
        .unwrap();
        let cell = tmp_cell_with_case(case);

        let stats = do_case_stats(&cell).unwrap();
        assert_eq!(stats.entity_count, 1);
        assert_eq!(stats.fact_count, 1);
        assert_eq!(stats.relationship_count, 0);
        assert_eq!(stats.conflicting_entity_count, 0);
    }

    #[test]
    fn audit_list_shows_a_merge_and_respects_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-overview-audit").unwrap();
        let a = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        let b = case
            .add_entity(EntityType::Person, None, vec![], manual_provenance())
            .unwrap();
        case.merge_entities(
            a,
            b,
            Actor {
                name: "user".to_string(),
            },
        )
        .unwrap();
        let cell = tmp_cell_with_case(case);

        let events = do_audit_list(&cell, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "merge");

        let limited = do_audit_list(&cell, 0).unwrap();
        assert!(limited.is_empty());
    }
}
