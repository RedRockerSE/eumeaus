//! G2 (SPEC.md §9.6): entity/fact browsing, read-only — wraps
//! `Case::list_entities`/`get_entity`/`list_attribute_records`, the same
//! calls `entity list`/`entity show` make (`eumeaus-cli/src/main.rs`).
//! Write path (add/merge/split) starts at G4.
//!
//! Reads the currently open case from `case_state::AppState`'s shared
//! `Mutex<Option<Case>>` — no case open is a normal, expected error here
//! ("open a case first"), not a bug.

use std::sync::{Arc, Mutex};

use eumeaus_engine::{Case, EntityFilter, EntityId};
use serde::Serialize;

use crate::case_state::AppState;

#[derive(Serialize, Debug)]
pub struct EntitySummary {
    pub id: String,
    pub entity_type: String,
    pub canonical_key: Option<String>,
    pub display_label: String,
}

impl From<eumeaus_engine::Entity> for EntitySummary {
    fn from(e: eumeaus_engine::Entity) -> Self {
        EntitySummary {
            id: e.id.to_string(),
            entity_type: e.entity_type.to_string(),
            canonical_key: e.canonical_key,
            display_label: e.display_label,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct AttributeRecordDto {
    pub fact_id: String,
    pub key: String,
    pub value: String,
    pub source: String,
    pub collected_at_unix_ms: i64,
    pub is_current: bool,
    pub conflicting: bool,
}

impl From<eumeaus_engine::AttributeRecord> for AttributeRecordDto {
    fn from(a: eumeaus_engine::AttributeRecord) -> Self {
        AttributeRecordDto {
            fact_id: a.fact_id.to_string(),
            key: a.key,
            value: a.value,
            source: a.source,
            collected_at_unix_ms: a.collected_at_unix_ms,
            is_current: a.is_current,
            conflicting: a.conflicting,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct EntityDetail {
    #[serde(flatten)]
    pub summary: EntitySummary,
    pub attributes: Vec<AttributeRecordDto>,
}

const NO_CASE_OPEN: &str = "no case is currently open — open a case first";

fn do_entity_list(
    cell: &Arc<Mutex<Option<Case>>>,
    entity_type: Option<String>,
) -> Result<Vec<EntitySummary>, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    let filter = EntityFilter {
        // EntityType::from_str is Infallible (SPEC.md §4.3's Custom escape
        // hatch) — an unrecognized string just becomes Custom(s), same as
        // the CLI's own `entity list --type` handling.
        entity_type: entity_type.map(|t| t.parse().expect("infallible")),
    };
    case.list_entities(filter)
        .map(|entities| entities.into_iter().map(EntitySummary::from).collect())
        .map_err(|e| e.to_string())
}

fn do_entity_show(cell: &Arc<Mutex<Option<Case>>>, id: &str) -> Result<EntityDetail, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    let entity_id = EntityId(id.parse().map_err(|e| format!("invalid entity id: {e}"))?);

    let entity = case.get_entity(entity_id).map_err(|e| e.to_string())?;
    let attributes = case
        .list_attribute_records(entity_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(AttributeRecordDto::from)
        .collect();

    Ok(EntityDetail {
        summary: EntitySummary::from(entity),
        attributes,
    })
}

#[tauri::command]
pub async fn entity_list(
    state: tauri::State<'_, AppState>,
    entity_type: Option<String>,
) -> Result<Vec<EntitySummary>, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_list(&cell, entity_type))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn entity_show(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<EntityDetail, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_entity_show(&cell, &id))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use eumeaus_engine::{Attribute, Provenance};

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
    fn list_and_show_error_cleanly_with_no_case_open() {
        let cell: Arc<Mutex<Option<Case>>> = Arc::new(Mutex::new(None));
        assert_eq!(do_entity_list(&cell, None).unwrap_err(), NO_CASE_OPEN);
        assert_eq!(
            do_entity_show(&cell, "00000000-0000-0000-0000-000000000000").unwrap_err(),
            NO_CASE_OPEN
        );
    }

    #[test]
    fn show_rejects_a_malformed_id_before_touching_the_case() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "g2-bad-id").unwrap();
        let cell = tmp_cell_with_case(case);

        let err = do_entity_show(&cell, "not-a-uuid").unwrap_err();
        assert!(err.contains("invalid entity id"));
    }

    // The real data-shape proof (SPEC.md §9.6 G2's verify bar): an entity
    // added exactly the way eumeaus-cli's `entity add` does (Case::add_entity
    // with user provenance) lists and shows through these commands with no
    // translation bugs — same ids, same attribute/fact fields the CLI's
    // own `entity show` prints.
    #[test]
    fn cli_created_entity_lists_and_shows_correctly() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g2-roundtrip").unwrap();

        let entity_id = case
            .add_entity(
                eumeaus_engine::EntityType::Username,
                Some("octocat".to_string()),
                vec![Attribute {
                    key: "bio".to_string(),
                    value: "test account".to_string(),
                }],
                manual_provenance(),
            )
            .unwrap();

        let cell = tmp_cell_with_case(case);

        let listed = do_entity_list(&cell, None).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, entity_id.to_string());
        assert_eq!(listed[0].entity_type, "Username");
        assert_eq!(listed[0].canonical_key.as_deref(), Some("octocat"));

        let detail = do_entity_show(&cell, &entity_id.to_string()).unwrap();
        assert_eq!(detail.summary.id, entity_id.to_string());
        assert_eq!(detail.attributes.len(), 1);
        assert_eq!(detail.attributes[0].key, "bio");
        assert_eq!(detail.attributes[0].value, "test account");
        assert_eq!(detail.attributes[0].source, "user");
        assert!(detail.attributes[0].is_current);
    }

    #[test]
    fn list_filters_by_entity_type() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g2-filter").unwrap();
        case.add_entity(
            eumeaus_engine::EntityType::Username,
            Some("a".to_string()),
            vec![],
            manual_provenance(),
        )
        .unwrap();
        case.add_entity(
            eumeaus_engine::EntityType::Email,
            Some("b@example.com".to_string()),
            vec![],
            manual_provenance(),
        )
        .unwrap();

        let cell = tmp_cell_with_case(case);
        let usernames = do_entity_list(&cell, Some("Username".to_string())).unwrap();
        assert_eq!(usernames.len(), 1);
        assert_eq!(usernames[0].entity_type, "Username");
    }
}
