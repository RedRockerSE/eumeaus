//! Entity/relationship CRUD, merge/split, and audit trail queries (M2,
//! SPEC.md §4.2/§4.4). Manual entries are always sourced `"user"` with
//! [`ConfidenceStatus::Found`] — a human directly asserting data isn't
//! "checking" anything.
//!
//! Free functions over `&Connection`/`&mut Connection` rather than `Case`
//! methods directly, so `case.rs` stays focused on lifecycle; `Case`'s
//! methods are thin delegates into here.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

use crate::{
    now_unix_ms, Actor, Attribute, AttributeRecord, AuditEvent, AuditTarget, ConfidenceStatus,
    EngineError, Entity, EntityFilter, EntityId, EntityType, FactId, Provenance, RelationshipId,
    RelationshipType,
};

fn normalize_key(raw: &str) -> String {
    raw.trim().to_lowercase()
}

fn entity_from_row(row: &Row) -> rusqlite::Result<Entity> {
    let id: String = row.get(0)?;
    let entity_type: String = row.get(1)?;
    Ok(Entity {
        id: EntityId(Uuid::parse_str(&id).expect("stored entity id is a valid uuid")),
        entity_type: entity_type
            .parse()
            .expect("EntityType::from_str is infallible"),
        canonical_key: row.get(2)?,
        display_label: row.get(3)?,
    })
}

fn ensure_entity_exists(conn: &Connection, id: EntityId) -> Result<(), EngineError> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM entities WHERE id = ?1",
            params![id.0.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    found.map(|_| ()).ok_or(EngineError::EntityNotFound(id))
}

pub(crate) fn get_entity(conn: &Connection, id: EntityId) -> Result<Entity, EngineError> {
    conn.query_row(
        "SELECT id, entity_type, canonical_key, display_label FROM entities WHERE id = ?1",
        params![id.0.to_string()],
        entity_from_row,
    )
    .optional()?
    .ok_or(EngineError::EntityNotFound(id))
}

/// Looks up an entity the same way exact-key auto-merge would ((entity_type,
/// normalized key)) — used by `scan run --target-type --target-value`
/// (SPEC.md §3.4) to resolve a scan's target without requiring callers to
/// already know its `EntityId`.
pub(crate) fn find_entity_by_key(
    conn: &Connection,
    entity_type: EntityType,
    key: &str,
) -> Result<Option<Entity>, EngineError> {
    conn.query_row(
        "SELECT id, entity_type, canonical_key, display_label FROM entities
         WHERE entity_type = ?1 AND canonical_key = ?2",
        params![entity_type.to_string(), normalize_key(key)],
        entity_from_row,
    )
    .optional()
    .map_err(EngineError::from)
}

pub(crate) fn list_entities(
    conn: &Connection,
    filter: EntityFilter,
) -> Result<Vec<Entity>, EngineError> {
    const BASE: &str = "SELECT id, entity_type, canonical_key, display_label FROM entities";
    let rows: Vec<Entity> = match filter.entity_type {
        Some(entity_type) => {
            let mut stmt = conn.prepare(&format!(
                "{BASE} WHERE entity_type = ?1 ORDER BY created_at"
            ))?;
            let mapped = stmt
                .query_map(params![entity_type.to_string()], entity_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            mapped
        }
        None => {
            let mut stmt = conn.prepare(&format!("{BASE} ORDER BY created_at"))?;
            let mapped = stmt
                .query_map([], entity_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            mapped
        }
    };
    Ok(rows)
}

/// Inserts a new entity, or — when `key` collides on `(entity_type,
/// canonical_key)` with an existing one — appends to it instead (SPEC.md
/// §4.4's exact-key auto-merge). Either way, records one fact carrying
/// `provenance`, plus one `entity_attributes` row per attribute tied to it.
pub(crate) fn add_entity(
    conn: &mut Connection,
    entity_type: EntityType,
    key: Option<String>,
    attrs: Vec<Attribute>,
    provenance: Provenance,
) -> Result<EntityId, EngineError> {
    let display_label = key.clone();
    add_entity_impl(
        conn,
        entity_type,
        key,
        display_label,
        attrs,
        provenance,
        ConfidenceStatus::Found,
        None,
    )
}

/// Same as [`add_entity`], but for a plugin-sourced finding during a scan
/// (M4): carries the plugin's own confidence for this finding, a distinct
/// `display_label` (a plugin's `EntityFinding.display_label` need not equal
/// its `canonical_key`), and tags the resulting fact with `scan_id`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_entity_from_scan(
    conn: &mut Connection,
    entity_type: EntityType,
    key: Option<String>,
    display_label: Option<String>,
    attrs: Vec<Attribute>,
    provenance: Provenance,
    confidence: ConfidenceStatus,
    scan_id: Uuid,
) -> Result<EntityId, EngineError> {
    add_entity_impl(
        conn,
        entity_type,
        key,
        display_label,
        attrs,
        provenance,
        confidence,
        Some(scan_id),
    )
}

#[allow(clippy::too_many_arguments)]
fn add_entity_impl(
    conn: &mut Connection,
    entity_type: EntityType,
    key: Option<String>,
    display_label: Option<String>,
    attrs: Vec<Attribute>,
    provenance: Provenance,
    confidence: ConfidenceStatus,
    scan_id: Option<Uuid>,
) -> Result<EntityId, EngineError> {
    let entity_type_str = entity_type.to_string();
    let canonical_key = key.as_deref().map(normalize_key);
    let now = now_unix_ms();

    let tx = conn.transaction()?;

    let existing: Option<String> = match &canonical_key {
        Some(ck) => tx
            .query_row(
                "SELECT id FROM entities WHERE entity_type = ?1 AND canonical_key = ?2",
                params![entity_type_str, ck],
                |row| row.get(0),
            )
            .optional()?,
        None => None,
    };

    let entity_id = match existing {
        Some(id_str) => {
            tx.execute(
                "UPDATE entities SET updated_at = ?1 WHERE id = ?2",
                params![now, id_str],
            )?;
            Uuid::parse_str(&id_str).expect("stored entity id is a valid uuid")
        }
        None => {
            let id = Uuid::new_v4();
            let display_label = display_label.unwrap_or_else(|| entity_type_str.clone());
            tx.execute(
                "INSERT INTO entities (id, entity_type, canonical_key, display_label, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![id.to_string(), entity_type_str, canonical_key, display_label, now],
            )?;
            id
        }
    };

    let fact_id = Uuid::new_v4();
    tx.execute(
        "INSERT INTO facts
            (id, entity_id, relationship_id, scan_id, source, source_version,
             confidence_status, source_url, retrieval_method, raw_response_sha256, collected_at)
         VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            fact_id.to_string(),
            entity_id.to_string(),
            scan_id.map(|id| id.to_string()),
            provenance.source,
            provenance.source_version,
            confidence.to_string(),
            provenance.source_url,
            provenance.retrieval_method,
            provenance.raw_response_sha256,
            provenance.collected_at_unix_ms,
        ],
    )?;

    {
        let mut insert_attr = tx.prepare(
            "INSERT INTO entity_attributes (id, entity_id, fact_id, key, value) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for attr in &attrs {
            insert_attr.execute(params![
                Uuid::new_v4().to_string(),
                entity_id.to_string(),
                fact_id.to_string(),
                attr.key,
                attr.value,
            ])?;
        }
    }

    tx.commit()?;
    Ok(EntityId(entity_id))
}

/// Absorbs `b` into `a`: re-points `b`'s facts, attributes, and
/// relationship endpoints at `a`, deletes `b`'s now-empty entity row, and
/// records the merge as an `audit_events` row (SPEC.md §4.4) — the
/// underlying facts' own data is never touched, only which entity currently
/// owns them.
pub(crate) fn merge_entities(
    conn: &mut Connection,
    a: EntityId,
    b: EntityId,
    actor: Actor,
) -> Result<EntityId, EngineError> {
    if a == b {
        return Err(EngineError::CannotMergeSelf(a));
    }

    let tx = conn.transaction()?;
    ensure_entity_exists(&tx, a)?;
    ensure_entity_exists(&tx, b)?;

    let (a_str, b_str) = (a.0.to_string(), b.0.to_string());
    tx.execute(
        "UPDATE facts SET entity_id = ?1 WHERE entity_id = ?2",
        params![a_str, b_str],
    )?;
    tx.execute(
        "UPDATE entity_attributes SET entity_id = ?1 WHERE entity_id = ?2",
        params![a_str, b_str],
    )?;
    tx.execute(
        "UPDATE relationships SET from_entity_id = ?1 WHERE from_entity_id = ?2",
        params![a_str, b_str],
    )?;
    tx.execute(
        "UPDATE relationships SET to_entity_id = ?1 WHERE to_entity_id = ?2",
        params![a_str, b_str],
    )?;
    tx.execute("DELETE FROM entities WHERE id = ?1", params![b_str])?;

    let now = now_unix_ms();
    tx.execute(
        "UPDATE entities SET updated_at = ?1 WHERE id = ?2",
        params![now, a_str],
    )?;
    tx.execute(
        "INSERT INTO audit_events (id, entity_id, relationship_id, event_type, description, actor, occurred_at)
         VALUES (?1, ?2, NULL, 'merge', ?3, ?4, ?5)",
        params![
            Uuid::new_v4().to_string(),
            a_str,
            format!("merged entity {b} into {a}"),
            actor.name,
            now,
        ],
    )?;

    tx.commit()?;
    Ok(a)
}

/// Moves the given facts (and their attributes) off `id` onto a brand-new
/// entity, and records the split as two `audit_events` rows — one on each
/// side — so each entity's own audit trail shows what happened to it
/// (SPEC.md §4.4).
pub(crate) fn split_entity(
    conn: &mut Connection,
    id: EntityId,
    fact_ids: Vec<FactId>,
    actor: Actor,
) -> Result<EntityId, EngineError> {
    if fact_ids.is_empty() {
        return Err(EngineError::NotImplemented(
            "Case::split_entity requires at least one fact id",
        ));
    }

    let tx = conn.transaction()?;
    let source = get_entity(&tx, id)?;

    for fact_id in &fact_ids {
        let owner: Option<String> = tx
            .query_row(
                "SELECT entity_id FROM facts WHERE id = ?1",
                params![fact_id.0.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if owner.as_deref() != Some(id.0.to_string().as_str()) {
            return Err(EngineError::FactNotFound(*fact_id, id));
        }
    }

    let new_id = Uuid::new_v4();
    let now = now_unix_ms();
    let display_label = format!("{} (split)", source.display_label);
    tx.execute(
        "INSERT INTO entities (id, entity_type, canonical_key, display_label, created_at, updated_at)
         VALUES (?1, ?2, NULL, ?3, ?4, ?4)",
        params![
            new_id.to_string(),
            source.entity_type.to_string(),
            display_label,
            now,
        ],
    )?;

    {
        let mut move_fact = tx.prepare("UPDATE facts SET entity_id = ?1 WHERE id = ?2")?;
        let mut move_attrs =
            tx.prepare("UPDATE entity_attributes SET entity_id = ?1 WHERE fact_id = ?2")?;
        for fact_id in &fact_ids {
            move_fact.execute(params![new_id.to_string(), fact_id.0.to_string()])?;
            move_attrs.execute(params![new_id.to_string(), fact_id.0.to_string()])?;
        }
    }

    tx.execute(
        "UPDATE entities SET updated_at = ?1 WHERE id = ?2",
        params![now, id.0.to_string()],
    )?;
    tx.execute(
        "INSERT INTO audit_events (id, entity_id, relationship_id, event_type, description, actor, occurred_at)
         VALUES (?1, ?2, NULL, 'split', ?3, ?4, ?5)",
        params![
            Uuid::new_v4().to_string(),
            id.0.to_string(),
            format!("split {} fact(s) off into new entity {new_id}", fact_ids.len()),
            actor.name,
            now,
        ],
    )?;
    tx.execute(
        "INSERT INTO audit_events (id, entity_id, relationship_id, event_type, description, actor, occurred_at)
         VALUES (?1, ?2, NULL, 'split', ?3, ?4, ?5)",
        params![
            Uuid::new_v4().to_string(),
            new_id.to_string(),
            format!("created via split from entity {id}"),
            actor.name,
            now,
        ],
    )?;

    tx.commit()?;
    Ok(EntityId(new_id))
}

pub(crate) fn add_relationship(
    conn: &mut Connection,
    from: EntityId,
    to: EntityId,
    rel_type: RelationshipType,
    attrs: Vec<Attribute>,
    provenance: Provenance,
) -> Result<RelationshipId, EngineError> {
    add_relationship_impl(
        conn,
        from,
        to,
        rel_type,
        attrs,
        provenance,
        ConfidenceStatus::Found,
        None,
    )
}

/// Same as [`add_relationship`], but for a plugin-sourced finding during a
/// scan (M4) — see [`add_entity_from_scan`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_relationship_from_scan(
    conn: &mut Connection,
    from: EntityId,
    to: EntityId,
    rel_type: RelationshipType,
    attrs: Vec<Attribute>,
    provenance: Provenance,
    confidence: ConfidenceStatus,
    scan_id: Uuid,
) -> Result<RelationshipId, EngineError> {
    add_relationship_impl(
        conn,
        from,
        to,
        rel_type,
        attrs,
        provenance,
        confidence,
        Some(scan_id),
    )
}

#[allow(clippy::too_many_arguments)]
fn add_relationship_impl(
    conn: &mut Connection,
    from: EntityId,
    to: EntityId,
    rel_type: RelationshipType,
    attrs: Vec<Attribute>,
    provenance: Provenance,
    confidence: ConfidenceStatus,
    scan_id: Option<Uuid>,
) -> Result<RelationshipId, EngineError> {
    let tx = conn.transaction()?;
    ensure_entity_exists(&tx, from)?;
    ensure_entity_exists(&tx, to)?;

    let rel_id = Uuid::new_v4();
    let now = now_unix_ms();
    tx.execute(
        "INSERT INTO relationships (id, from_entity_id, to_entity_id, relationship_type, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            rel_id.to_string(),
            from.0.to_string(),
            to.0.to_string(),
            rel_type.to_string(),
            now,
        ],
    )?;

    let fact_id = Uuid::new_v4();
    tx.execute(
        "INSERT INTO facts
            (id, entity_id, relationship_id, scan_id, source, source_version,
             confidence_status, source_url, retrieval_method, raw_response_sha256, collected_at)
         VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            fact_id.to_string(),
            rel_id.to_string(),
            scan_id.map(|id| id.to_string()),
            provenance.source,
            provenance.source_version,
            confidence.to_string(),
            provenance.source_url,
            provenance.retrieval_method,
            provenance.raw_response_sha256,
            provenance.collected_at_unix_ms,
        ],
    )?;

    {
        let mut insert_attr = tx.prepare(
            "INSERT INTO relationship_attributes (id, relationship_id, fact_id, key, value) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for attr in &attrs {
            insert_attr.execute(params![
                Uuid::new_v4().to_string(),
                rel_id.to_string(),
                fact_id.to_string(),
                attr.key,
                attr.value,
            ])?;
        }
    }

    tx.commit()?;
    Ok(RelationshipId(rel_id))
}

/// All attribute facts on `id`, newest first within each key. SPEC.md
/// §4.4: the first record per key is the "current" one; `conflicting`
/// marks a key where facts disagree, so a viewer never mistakes "current"
/// for "only".
pub(crate) fn list_attribute_records(
    conn: &Connection,
    id: EntityId,
) -> Result<Vec<AttributeRecord>, EngineError> {
    ensure_entity_exists(conn, id)?;

    let mut stmt = conn.prepare(
        "SELECT ea.key, ea.value, f.source, f.collected_at
         FROM entity_attributes ea
         JOIN facts f ON f.id = ea.fact_id
         WHERE ea.entity_id = ?1
         ORDER BY ea.key, f.collected_at DESC",
    )?;
    let rows: Vec<(String, String, String, i64)> = stmt
        .query_map(params![id.0.to_string()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<_, _>>()?;

    let mut distinct_values: HashMap<String, HashSet<String>> = HashMap::new();
    for (key, value, ..) in &rows {
        distinct_values
            .entry(key.clone())
            .or_default()
            .insert(value.clone());
    }

    let mut seen_current: HashSet<String> = HashSet::new();
    let mut records = Vec::with_capacity(rows.len());
    for (key, value, source, collected_at) in rows {
        let is_current = seen_current.insert(key.clone());
        let conflicting = distinct_values.get(&key).is_some_and(|v| v.len() > 1);
        records.push(AttributeRecord {
            key,
            value,
            source,
            collected_at_unix_ms: collected_at,
            is_current,
            conflicting,
        });
    }
    Ok(records)
}

pub(crate) fn audit_trail(
    conn: &Connection,
    target: AuditTarget,
) -> Result<Vec<AuditEvent>, EngineError> {
    let (column, id_str) = match target {
        AuditTarget::Entity(id) => ("entity_id", id.0.to_string()),
        AuditTarget::Relationship(id) => ("relationship_id", id.0.to_string()),
        AuditTarget::Scan(_) => {
            return Err(EngineError::NotImplemented(
                "Case::audit_trail for scans (lands in M4)",
            ))
        }
    };

    let mut stmt = conn.prepare(&format!(
        "SELECT id, event_type, description, actor, occurred_at
         FROM audit_events WHERE {column} = ?1 ORDER BY occurred_at"
    ))?;
    let rows = stmt
        .query_map(params![id_str], |row| {
            let id: String = row.get(0)?;
            Ok(AuditEvent {
                id: Uuid::parse_str(&id).expect("stored audit event id is a valid uuid"),
                event_type: row.get(1)?,
                description: row.get(2)?,
                actor: row.get(3)?,
                occurred_at_unix_ms: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA_SQL: &str = include_str!("schema.sql");

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }

    fn test_provenance() -> Provenance {
        Provenance {
            source: "user".to_string(),
            source_version: "0.1.0".to_string(),
            source_url: None,
            retrieval_method: None,
            raw_response_sha256: None,
            collected_at_unix_ms: 1000,
        }
    }

    fn attr(key: &str, value: &str) -> Attribute {
        Attribute {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    fn actor() -> Actor {
        Actor {
            name: "tester".to_string(),
        }
    }

    #[test]
    fn add_entity_creates_entity_with_attributes_and_a_fact() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Username,
            Some("Alice".to_string()),
            vec![attr("bio", "hello")],
            test_provenance(),
        )
        .unwrap();

        let entity = get_entity(&conn, id).unwrap();
        assert_eq!(entity.entity_type, EntityType::Username);
        assert_eq!(entity.canonical_key.as_deref(), Some("alice"));
        assert_eq!(entity.display_label, "Alice");

        let attrs = list_attribute_records(&conn, id).unwrap();
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0].key, "bio");
        assert_eq!(attrs[0].value, "hello");
        assert!(attrs[0].is_current);
        assert!(!attrs[0].conflicting);
    }

    #[test]
    fn add_entity_with_same_canonical_key_auto_merges() {
        let mut conn = test_conn();
        let first = add_entity(
            &mut conn,
            EntityType::Username,
            Some("bob".to_string()),
            vec![attr("k", "v1")],
            test_provenance(),
        )
        .unwrap();
        let second = add_entity(
            &mut conn,
            EntityType::Username,
            Some("BOB".to_string()), // different case, same normalized key
            vec![attr("k", "v2")],
            test_provenance(),
        )
        .unwrap();

        assert_eq!(
            first, second,
            "exact-key match must auto-merge, not duplicate"
        );
        let attrs = list_attribute_records(&conn, first).unwrap();
        assert_eq!(
            attrs.len(),
            2,
            "both facts' attributes should be attached to the one entity"
        );

        let count: i64 = conn
            .query_row("SELECT count(*) FROM entities", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn add_entity_without_key_never_auto_merges() {
        let mut conn = test_conn();
        let a = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();
        let b = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn merge_entities_moves_facts_and_records_audit_event() {
        let mut conn = test_conn();
        let a = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![attr("name", "Alice")],
            test_provenance(),
        )
        .unwrap();
        let b = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![attr("nickname", "Al")],
            test_provenance(),
        )
        .unwrap();

        let survivor = merge_entities(&mut conn, a, b, actor()).unwrap();
        assert_eq!(survivor, a);

        assert!(matches!(
            get_entity(&conn, b).unwrap_err(),
            EngineError::EntityNotFound(_)
        ));

        let attrs = list_attribute_records(&conn, a).unwrap();
        assert_eq!(attrs.len(), 2, "b's attributes must move to a");

        let events = audit_trail(&conn, AuditTarget::Entity(a)).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "merge");
        assert_eq!(events[0].actor, "tester");
    }

    #[test]
    fn merge_entities_rejects_self_merge() {
        let mut conn = test_conn();
        let a = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();
        let err = merge_entities(&mut conn, a, a, actor()).unwrap_err();
        assert!(matches!(err, EngineError::CannotMergeSelf(_)));
    }

    #[test]
    fn merge_entities_errors_on_missing_entity() {
        let mut conn = test_conn();
        let a = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();
        let missing = EntityId(Uuid::new_v4());
        let err = merge_entities(&mut conn, a, missing, actor()).unwrap_err();
        assert!(matches!(err, EngineError::EntityNotFound(_)));
    }

    #[test]
    fn split_entity_moves_only_the_given_facts() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![attr("a", "1")],
            test_provenance(),
        )
        .unwrap();
        add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap(); // unrelated entity, just to prove this doesn't interfere

        let fact_id: String = conn
            .query_row(
                "SELECT id FROM facts WHERE entity_id = ?1",
                params![id.0.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let fact_id = FactId(Uuid::parse_str(&fact_id).unwrap());

        let new_id = split_entity(&mut conn, id, vec![fact_id], actor()).unwrap();
        assert_ne!(new_id, id);

        let original_attrs = list_attribute_records(&conn, id).unwrap();
        assert!(original_attrs.is_empty(), "the only fact moved away");

        let new_attrs = list_attribute_records(&conn, new_id).unwrap();
        assert_eq!(new_attrs.len(), 1);
        assert_eq!(new_attrs[0].key, "a");

        assert_eq!(
            audit_trail(&conn, AuditTarget::Entity(id)).unwrap().len(),
            1
        );
        assert_eq!(
            audit_trail(&conn, AuditTarget::Entity(new_id))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn split_entity_rejects_a_fact_that_does_not_belong_to_it() {
        let mut conn = test_conn();
        let a = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![attr("a", "1")],
            test_provenance(),
        )
        .unwrap();
        let b = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![attr("b", "2")],
            test_provenance(),
        )
        .unwrap();

        let b_fact_id: String = conn
            .query_row(
                "SELECT id FROM facts WHERE entity_id = ?1",
                params![b.0.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let b_fact_id = FactId(Uuid::parse_str(&b_fact_id).unwrap());

        let err = split_entity(&mut conn, a, vec![b_fact_id], actor()).unwrap_err();
        assert!(matches!(err, EngineError::FactNotFound(_, _)));
    }

    #[test]
    fn add_relationship_links_two_entities_with_an_attribute() {
        let mut conn = test_conn();
        let a = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();
        let b = add_entity(
            &mut conn,
            EntityType::Organization,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();

        let rel_id = add_relationship(
            &mut conn,
            a,
            b,
            RelationshipType::MemberOf,
            vec![attr("role", "engineer")],
            test_provenance(),
        )
        .unwrap();

        let rel_type: String = conn
            .query_row(
                "SELECT relationship_type FROM relationships WHERE id = ?1",
                params![rel_id.0.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rel_type, "MemberOf");

        let attr_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM relationship_attributes WHERE relationship_id = ?1",
                params![rel_id.0.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attr_count, 1);
    }

    #[test]
    fn add_relationship_errors_when_an_endpoint_is_missing() {
        let mut conn = test_conn();
        let a = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();
        let missing = EntityId(Uuid::new_v4());

        let err = add_relationship(
            &mut conn,
            a,
            missing,
            RelationshipType::RelatedTo,
            vec![],
            test_provenance(),
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::EntityNotFound(_)));
    }

    #[test]
    fn list_attribute_records_flags_conflicting_values() {
        let mut conn = test_conn();
        let first = add_entity(
            &mut conn,
            EntityType::Username,
            Some("carol".to_string()),
            vec![attr("color", "blue")],
            test_provenance(),
        )
        .unwrap();

        // Same canonical key -> auto-merges into `first` (a second fact on
        // the same entity), but disagrees on "color".
        let mut later = test_provenance();
        later.collected_at_unix_ms = 2000;
        let second = add_entity(
            &mut conn,
            EntityType::Username,
            Some("carol".to_string()),
            vec![attr("color", "red")],
            later,
        )
        .unwrap();
        assert_eq!(first, second);

        let records = list_attribute_records(&conn, first).unwrap();
        assert_eq!(records.len(), 2);
        let current = records.iter().find(|r| r.is_current).unwrap();
        assert_eq!(
            current.value, "red",
            "most recent by collected_at should be current"
        );
        assert!(records.iter().all(|r| r.conflicting));
    }
}
