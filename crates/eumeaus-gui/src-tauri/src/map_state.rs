//! Map screen (SPEC.md §9.3): every plottable location in the case,
//! wrapping `Case::map_points`. See `eumeaus_engine::MapPoint`'s own doc
//! for the two ways an entity contributes a point (a `Location` entity
//! with a coordinate `canonical_key`, or a `lat`/`lon` attribute pair on
//! any entity) — this module is a thin DTO/command wrapper, same
//! precedent as `overview_state.rs`.

use std::sync::{Arc, Mutex};

use eumeaus_engine::{Case, MapPoint, MapPointSource};
use serde::Serialize;

use crate::case_state::AppState;

const NO_CASE_OPEN: &str = "no case is currently open — open a case first";

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MapPointSourceDto {
    LocationEntity,
    Attribute,
}

impl From<MapPointSource> for MapPointSourceDto {
    fn from(s: MapPointSource) -> Self {
        match s {
            MapPointSource::LocationEntity => MapPointSourceDto::LocationEntity,
            MapPointSource::Attribute => MapPointSourceDto::Attribute,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct MapPointDto {
    pub entity_id: String,
    pub entity_type: String,
    pub display_label: String,
    pub lat: f64,
    pub lon: f64,
    pub source: MapPointSourceDto,
    pub related_entity_ids: Vec<String>,
}

impl From<MapPoint> for MapPointDto {
    fn from(p: MapPoint) -> Self {
        MapPointDto {
            entity_id: p.entity_id.to_string(),
            entity_type: p.entity_type.to_string(),
            display_label: p.display_label,
            lat: p.lat,
            lon: p.lon,
            source: p.source.into(),
            related_entity_ids: p
                .related_entity_ids
                .iter()
                .map(|id| id.to_string())
                .collect(),
        }
    }
}

fn do_map_points(cell: &Arc<Mutex<Option<Case>>>) -> Result<Vec<MapPointDto>, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;
    case.map_points()
        .map(|points| points.into_iter().map(MapPointDto::from).collect())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn map_points(state: tauri::State<'_, AppState>) -> Result<Vec<MapPointDto>, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || do_map_points(&cell))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use eumeaus_engine::{Attribute, EntityType, Provenance, RelationshipType};

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
    fn map_points_errors_cleanly_with_no_case_open() {
        let cell: Arc<Mutex<Option<Case>>> = Arc::new(Mutex::new(None));
        assert_eq!(do_map_points(&cell).unwrap_err(), NO_CASE_OPEN);
    }

    #[test]
    fn map_points_resolves_a_location_and_its_related_entity() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-map-points").unwrap();
        let person = case
            .add_entity(
                EntityType::Person,
                Some("nadia".to_string()),
                vec![],
                manual_provenance(),
            )
            .unwrap();
        let location = case
            .add_entity(
                EntityType::Location,
                Some("40.689247,-74.044503".to_string()),
                vec![],
                manual_provenance(),
            )
            .unwrap();
        case.add_relationship(
            person,
            location,
            RelationshipType::LocatedAt,
            vec![],
            manual_provenance(),
        )
        .unwrap();
        let cell = tmp_cell_with_case(case);

        let points = do_map_points(&cell).unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].entity_id, location.to_string());
        assert_eq!(points[0].source, MapPointSourceDto::LocationEntity);
        assert_eq!(points[0].related_entity_ids, vec![person.to_string()]);
    }

    #[test]
    fn map_points_skips_entities_with_no_usable_coordinate() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-map-points-empty").unwrap();
        case.add_entity(
            EntityType::Person,
            Some("no-location".to_string()),
            vec![Attribute {
                key: "note".to_string(),
                value: "nothing geographic here".to_string(),
            }],
            manual_provenance(),
        )
        .unwrap();
        let cell = tmp_cell_with_case(case);

        assert!(do_map_points(&cell).unwrap().is_empty());
    }
}
