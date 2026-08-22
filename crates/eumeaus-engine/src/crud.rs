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
    EngineError, Entity, EntityFilter, EntityId, EntityImageData, EntityImageSummary,
    EntityPosition, EntityType, FactId, ImageId, Provenance, Relationship, RelationshipId,
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

fn ensure_relationship_exists(conn: &Connection, id: RelationshipId) -> Result<(), EngineError> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM relationships WHERE id = ?1",
            params![id.0.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    found
        .map(|_| ())
        .ok_or(EngineError::RelationshipNotFound(id))
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

/// Adds a new fact (and its attributes) directly to an *existing* entity,
/// identified by id rather than by `(entity_type, canonical_key)` —
/// unlike [`add_entity`], there's no auto-merge resolution step, because
/// the caller already knows exactly which entity this belongs to.
///
/// This matters for entities added *without* a canonical key
/// (`add_entity`'s `key: None` path): re-calling `add_entity` with the
/// same type but no key never auto-merges (SPEC.md §4.4 — there's no key
/// to match on), so it would silently create a *second*, unrelated
/// entity instead of adding a fact to the one the caller meant. This
/// function has no such trap: it always targets the entity_id given,
/// keyed or not — the GUI's entity-detail "add fact" action (added after
/// the exploratory test found no correct way to add a fact to an
/// existing entity) needs exactly this.
pub(crate) fn add_fact_to_entity(
    conn: &mut Connection,
    entity_id: EntityId,
    attrs: Vec<Attribute>,
    provenance: Provenance,
) -> Result<FactId, EngineError> {
    let now = now_unix_ms();
    let tx = conn.transaction()?;
    ensure_entity_exists(&tx, entity_id)?;

    tx.execute(
        "UPDATE entities SET updated_at = ?1 WHERE id = ?2",
        params![now, entity_id.0.to_string()],
    )?;

    let fact_id = Uuid::new_v4();
    tx.execute(
        "INSERT INTO facts
            (id, entity_id, relationship_id, scan_id, source, source_version,
             confidence_status, source_url, retrieval_method, raw_response_sha256, collected_at)
         VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            fact_id.to_string(),
            entity_id.0.to_string(),
            provenance.source,
            provenance.source_version,
            ConfidenceStatus::Found.to_string(),
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
                entity_id.0.to_string(),
                fact_id.to_string(),
                attr.key,
                attr.value,
            ])?;
        }
    }

    tx.commit()?;
    Ok(FactId(fact_id))
}

/// Attaches an image to an existing entity — the image-upload equivalent
/// of [`add_fact_to_entity`]: one new fact carrying `provenance`, plus one
/// `entity_images` row tied to it. No entity type is privileged (SPEC.md
/// §4.2) — this works on any `entity_id`, same as `add_fact_to_entity`.
pub(crate) fn add_image_to_entity(
    conn: &mut Connection,
    entity_id: EntityId,
    mime_type: String,
    data: Vec<u8>,
    provenance: Provenance,
) -> Result<FactId, EngineError> {
    let now = now_unix_ms();
    let tx = conn.transaction()?;
    ensure_entity_exists(&tx, entity_id)?;

    tx.execute(
        "UPDATE entities SET updated_at = ?1 WHERE id = ?2",
        params![now, entity_id.0.to_string()],
    )?;

    let fact_id = Uuid::new_v4();
    tx.execute(
        "INSERT INTO facts
            (id, entity_id, relationship_id, scan_id, source, source_version,
             confidence_status, source_url, retrieval_method, raw_response_sha256, collected_at)
         VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            fact_id.to_string(),
            entity_id.0.to_string(),
            provenance.source,
            provenance.source_version,
            ConfidenceStatus::Found.to_string(),
            provenance.source_url,
            provenance.retrieval_method,
            provenance.raw_response_sha256,
            provenance.collected_at_unix_ms,
        ],
    )?;

    tx.execute(
        "INSERT INTO entity_images (id, entity_id, fact_id, mime_type, data) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            Uuid::new_v4().to_string(),
            entity_id.0.to_string(),
            fact_id.to_string(),
            mime_type,
            data,
        ],
    )?;

    tx.commit()?;
    Ok(FactId(fact_id))
}

/// Metadata (no BLOB) for every image on an entity, newest first. Mirrors
/// [`attribute_records_from_table`]'s "most recent wins, but nothing is
/// ever hidden" rule: the first (newest) row is flagged `is_current` for
/// e.g. a profile-picture display, but every image stays listed — a
/// gallery, not a single overwritten slot.
pub(crate) fn list_entity_images(
    conn: &Connection,
    entity_id: EntityId,
) -> Result<Vec<EntityImageSummary>, EngineError> {
    ensure_entity_exists(conn, entity_id)?;
    let mut stmt = conn.prepare(
        "SELECT i.id, i.fact_id, i.mime_type, f.collected_at
         FROM entity_images i JOIN facts f ON f.id = i.fact_id
         WHERE i.entity_id = ?1
         ORDER BY f.collected_at DESC",
    )?;
    let rows: Vec<(String, String, String, i64)> = stmt
        .query_map(params![entity_id.0.to_string()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<_, _>>()?;

    Ok(rows
        .into_iter()
        .enumerate()
        .map(
            |(i, (id, fact_id, mime_type, collected_at))| EntityImageSummary {
                id: ImageId(Uuid::parse_str(&id).expect("stored image id is a valid uuid")),
                fact_id: FactId(Uuid::parse_str(&fact_id).expect("stored fact id is a valid uuid")),
                mime_type,
                collected_at_unix_ms: collected_at,
                is_current: i == 0,
            },
        )
        .collect())
}

/// The bytes for one image, fetched by its own id (not `fact_id` — a
/// single fact may in principle carry more than one image row).
pub(crate) fn get_entity_image(
    conn: &Connection,
    image_id: ImageId,
) -> Result<EntityImageData, EngineError> {
    conn.query_row(
        "SELECT mime_type, data FROM entity_images WHERE id = ?1",
        params![image_id.0.to_string()],
        |row| {
            Ok(EntityImageData {
                mime_type: row.get(0)?,
                data: row.get(1)?,
            })
        },
    )
    .optional()?
    .ok_or(EngineError::ImageNotFound(image_id))
}

/// Upserts the dragged position for one entity in the Link graph. `x`/`y`
/// are opaque SVG user-space coordinates as the GUI defines them — the
/// engine doesn't interpret them, just stores the last one it was given.
pub(crate) fn set_entity_position(
    conn: &Connection,
    entity_id: EntityId,
    x: f64,
    y: f64,
) -> Result<(), EngineError> {
    ensure_entity_exists(conn, entity_id)?;
    conn.execute(
        "INSERT INTO entity_positions (entity_id, x, y) VALUES (?1, ?2, ?3)
         ON CONFLICT (entity_id) DO UPDATE SET x = excluded.x, y = excluded.y",
        params![entity_id.0.to_string(), x, y],
    )?;
    Ok(())
}

/// Every entity that currently has a dragged position — the GUI loads this
/// once on the Graph screen mounting and falls back to its own circle
/// layout for any entity id not present in the result.
pub(crate) fn list_entity_positions(conn: &Connection) -> Result<Vec<EntityPosition>, EngineError> {
    let mut stmt = conn.prepare("SELECT entity_id, x, y FROM entity_positions")?;
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let x: f64 = row.get(1)?;
        let y: f64 = row.get(2)?;
        Ok((id, x, y))
    })?;
    rows.map(|r| {
        let (id, x, y) = r?;
        Ok(EntityPosition {
            entity_id: EntityId(Uuid::parse_str(&id).expect("stored entity id is a valid uuid")),
            x,
            y,
        })
    })
    .collect()
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
    // Must run before the entities delete below — foreign_keys=ON (Case::open)
    // would otherwise reject deleting a row entity_positions still references.
    // `a` keeps its own row (if any) untouched; `b`'s would otherwise be an
    // orphan referencing a now-deleted entity.
    tx.execute(
        "DELETE FROM entity_positions WHERE entity_id = ?1",
        params![b_str],
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
/// entity of `entity_type` — deliberately not always `source`'s own type
/// (e.g. splitting a username fact off a Person should be able to produce
/// a Username entity, not another Person) — and records the split as two
/// `audit_events` rows — one on each side — so each entity's own audit
/// trail shows what happened to it (SPEC.md §4.4).
///
/// `key` is optional, same as [`add_entity`]'s own `key`, and drives the
/// new entity's `canonical_key`/`display_label` the same way: without one
/// the new entity falls back to "{source label} (split)" and a NULL
/// canonical_key — which works fine for browsing, but leaves the new
/// entity unreachable as a scan target, since both the GUI's target picker
/// and `find_entity_by_key` require a real canonical_key. Splitting a
/// Username entity back out of a Person and actually wanting to scan it
/// needs a key that's the split-off value itself (e.g. the username), not
/// the source entity's own name — `split_entity` can't infer that from
/// which facts moved, so the caller has to supply it.
pub(crate) fn split_entity(
    conn: &mut Connection,
    id: EntityId,
    fact_ids: Vec<FactId>,
    entity_type: EntityType,
    key: Option<String>,
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
    let canonical_key = key.as_deref().map(normalize_key);
    let display_label = key.unwrap_or_else(|| format!("{} (split)", source.display_label));
    tx.execute(
        "INSERT INTO entities (id, entity_type, canonical_key, display_label, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![
            new_id.to_string(),
            entity_type.to_string(),
            canonical_key,
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

/// Permanently deletes a fact and its attribute row(s) (SPEC.md §8 open
/// question 4, now resolved): true deletion, not crypto-shredding — the
/// `facts` table's append-only-ness exists for investigative integrity
/// (nobody quietly rewrites history), not to make legitimate redaction
/// impossible. What survives instead is the audit trail: a `redact` event
/// records that a fact existed and was removed — its id, source, and
/// collection time — plus `reason`, but never the redacted value itself
/// (which lived only in `entity_attributes`/`relationship_attributes`,
/// now deleted).
///
/// Fact-level only: the entity/relationship's own `canonical_key`/
/// `display_label` (or `relationship_type`) are untouched, since those
/// live on the entity/relationship row itself, not per-fact — full entity
/// erasure is a different, larger operation this doesn't attempt.
pub(crate) fn redact_fact(
    conn: &mut Connection,
    fact_id: FactId,
    actor: Actor,
    reason: &str,
) -> Result<(), EngineError> {
    let tx = conn.transaction()?;

    let (entity_id, relationship_id, source, collected_at): (
        Option<String>,
        Option<String>,
        String,
        i64,
    ) = tx
        .query_row(
            "SELECT entity_id, relationship_id, source, collected_at FROM facts WHERE id = ?1",
            params![fact_id.0.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?
        .ok_or(EngineError::UnknownFact(fact_id))?;

    tx.execute(
        "DELETE FROM entity_attributes WHERE fact_id = ?1",
        params![fact_id.0.to_string()],
    )?;
    tx.execute(
        "DELETE FROM relationship_attributes WHERE fact_id = ?1",
        params![fact_id.0.to_string()],
    )?;
    tx.execute(
        "DELETE FROM entity_images WHERE fact_id = ?1",
        params![fact_id.0.to_string()],
    )?;
    tx.execute(
        "DELETE FROM facts WHERE id = ?1",
        params![fact_id.0.to_string()],
    )?;

    let now = now_unix_ms();
    let description =
        format!("redacted fact {fact_id} (source: {source}, collected_at: {collected_at}) — reason: {reason}");
    tx.execute(
        "INSERT INTO audit_events (id, entity_id, relationship_id, event_type, description, actor, occurred_at)
         VALUES (?1, ?2, ?3, 'redact', ?4, ?5, ?6)",
        params![
            Uuid::new_v4().to_string(),
            entity_id,
            relationship_id,
            description,
            actor.name,
            now,
        ],
    )?;

    tx.commit()?;
    Ok(())
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
/// for "only". Shared by [`list_attribute_records`] (`entity_attributes`)
/// and [`list_relationship_attribute_records`] (`relationship_attributes`)
/// — the two tables are structurally identical (SPEC.md §4.2's
/// `relationship_attributes` mirrors `entity_attributes`).
fn attribute_records_from_table(
    conn: &Connection,
    table: &str,
    id_column: &str,
    id_str: &str,
) -> Result<Vec<AttributeRecord>, EngineError> {
    let sql = format!(
        "SELECT t.fact_id, t.key, t.value, f.source, f.collected_at
         FROM {table} t
         JOIN facts f ON f.id = t.fact_id
         WHERE t.{id_column} = ?1
         ORDER BY t.key, f.collected_at DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(String, String, String, String, i64)> = stmt
        .query_map(params![id_str], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<Result<_, _>>()?;

    let mut distinct_values: HashMap<String, HashSet<String>> = HashMap::new();
    for (_, key, value, ..) in &rows {
        distinct_values
            .entry(key.clone())
            .or_default()
            .insert(value.clone());
    }

    let mut seen_current: HashSet<String> = HashSet::new();
    let mut records = Vec::with_capacity(rows.len());
    for (fact_id, key, value, source, collected_at) in rows {
        let is_current = seen_current.insert(key.clone());
        let conflicting = distinct_values.get(&key).is_some_and(|v| v.len() > 1);
        records.push(AttributeRecord {
            fact_id: FactId(Uuid::parse_str(&fact_id).expect("stored fact id is a valid uuid")),
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

pub(crate) fn list_attribute_records(
    conn: &Connection,
    id: EntityId,
) -> Result<Vec<AttributeRecord>, EngineError> {
    ensure_entity_exists(conn, id)?;
    attribute_records_from_table(conn, "entity_attributes", "entity_id", &id.0.to_string())
}

/// Same as [`list_attribute_records`], but for a relationship's attributes
/// — not in SPEC.md §3.1 at all, but `case export --format report` needs a
/// way to include them (there's no `relationship show` CLI command).
pub(crate) fn list_relationship_attribute_records(
    conn: &Connection,
    id: RelationshipId,
) -> Result<Vec<AttributeRecord>, EngineError> {
    ensure_relationship_exists(conn, id)?;
    attribute_records_from_table(
        conn,
        "relationship_attributes",
        "relationship_id",
        &id.0.to_string(),
    )
}

fn relationship_from_row(row: &Row) -> rusqlite::Result<Relationship> {
    let id: String = row.get(0)?;
    let from: String = row.get(1)?;
    let to: String = row.get(2)?;
    let relationship_type: String = row.get(3)?;
    Ok(Relationship {
        id: RelationshipId(Uuid::parse_str(&id).expect("stored relationship id is a valid uuid")),
        from: EntityId(Uuid::parse_str(&from).expect("stored entity id is a valid uuid")),
        to: EntityId(Uuid::parse_str(&to).expect("stored entity id is a valid uuid")),
        relationship_type: relationship_type
            .parse()
            .expect("RelationshipType::from_str is infallible"),
        created_at_unix_ms: row.get(4)?,
    })
}

/// Every relationship in the case, oldest first. Not in SPEC.md §3.1 (no
/// `relationship list` CLI command exists either) — added purely for
/// `case export --format report`, which needs the full graph.
pub(crate) fn list_relationships(conn: &Connection) -> Result<Vec<Relationship>, EngineError> {
    let mut stmt = conn.prepare(
        "SELECT id, from_entity_id, to_entity_id, relationship_type, created_at
         FROM relationships ORDER BY created_at",
    )?;
    let rows = stmt
        .query_map([], relationship_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
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

/// Every audit event in the case, newest first — added for the GUI's
/// Overview screen (SPEC.md §9.3), which shows a mixed feed across every
/// entity/relationship rather than one target's history the way
/// `audit_trail` (and `eumeaus-cli`'s `audit show`) always has.
pub(crate) fn audit_trail_all(
    conn: &Connection,
    limit: u32,
) -> Result<Vec<AuditEvent>, EngineError> {
    let mut stmt = conn.prepare(
        "SELECT id, event_type, description, actor, occurred_at
         FROM audit_events ORDER BY occurred_at DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit], |row| {
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

/// Case-wide counts for the GUI's Overview screen (SPEC.md §9.3) — no
/// CLI command surfaces these today, so this is purely additive.
/// `conflicting_entity_count` mirrors `attribute_records_from_table`'s own
/// per-key "more than one distinct value" definition of conflicting,
/// aggregated across every entity in one query rather than looping
/// `list_attribute_records` per entity.
pub(crate) fn case_stats(conn: &Connection) -> Result<crate::CaseStats, EngineError> {
    let entity_count = conn.query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))?;
    let fact_count = conn.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))?;
    let relationship_count =
        conn.query_row("SELECT COUNT(*) FROM relationships", [], |r| r.get(0))?;
    let conflicting_entity_count = conn.query_row(
        "SELECT COUNT(DISTINCT entity_id) FROM (
            SELECT entity_id FROM entity_attributes
            GROUP BY entity_id, key
            HAVING COUNT(DISTINCT value) > 1
         )",
        [],
        |r| r.get(0),
    )?;
    Ok(crate::CaseStats {
        entity_count,
        fact_count,
        relationship_count,
        conflicting_entity_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA_SQL: &str = include_str!("schema.sql");
    const SCHEMA_ADDITIONS_SQL: &str = include_str!("schema_additions.sql");

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        conn.execute_batch(SCHEMA_ADDITIONS_SQL).unwrap();
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
    fn add_fact_to_entity_adds_a_fact_without_touching_existing_ones() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Username,
            Some("carol".to_string()),
            vec![attr("bio", "first")],
            test_provenance(),
        )
        .unwrap();

        let fact_id = add_fact_to_entity(
            &mut conn,
            id,
            vec![attr("location", "here")],
            test_provenance(),
        )
        .unwrap();

        let attrs = list_attribute_records(&conn, id).unwrap();
        assert_eq!(attrs.len(), 2, "the original fact's attribute must survive");
        assert!(attrs.iter().any(|a| a.key == "bio" && a.value == "first"));
        let new_attr = attrs.iter().find(|a| a.key == "location").unwrap();
        assert_eq!(new_attr.value, "here");
        assert_eq!(new_attr.fact_id, fact_id);
    }

    #[test]
    fn add_fact_to_entity_works_on_a_keyless_entity_without_creating_a_duplicate() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            None, // no canonical key — add_entity's auto-merge can never target this
            vec![attr("alias", "J. Doe")],
            test_provenance(),
        )
        .unwrap();

        add_fact_to_entity(
            &mut conn,
            id,
            vec![attr("note", "seen once")],
            test_provenance(),
        )
        .unwrap();

        // Still exactly one Person entity — a naive "re-call add_entity"
        // approach would have silently created a second, unrelated one.
        let all = list_entities(
            &conn,
            EntityFilter {
                entity_type: Some(EntityType::Person),
            },
        )
        .unwrap();
        assert_eq!(all.len(), 1);
        let attrs = list_attribute_records(&conn, id).unwrap();
        assert_eq!(attrs.len(), 2);
    }

    #[test]
    fn add_fact_to_entity_errors_on_an_unknown_entity() {
        let mut conn = test_conn();
        let bogus = EntityId(Uuid::new_v4());

        let err = add_fact_to_entity(&mut conn, bogus, vec![], test_provenance()).unwrap_err();
        assert!(matches!(err, EngineError::EntityNotFound(_)));
    }

    #[test]
    fn add_image_to_entity_attaches_an_image_and_creates_a_fact() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            Some("dave".to_string()),
            vec![],
            test_provenance(),
        )
        .unwrap();

        let fact_id = add_image_to_entity(
            &mut conn,
            id,
            "image/png".to_string(),
            vec![1, 2, 3, 4],
            test_provenance(),
        )
        .unwrap();

        let images = list_entity_images(&conn, id).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].fact_id, fact_id);
        assert_eq!(images[0].mime_type, "image/png");
        assert!(images[0].is_current);

        let data = get_entity_image(&conn, images[0].id).unwrap();
        assert_eq!(data.mime_type, "image/png");
        assert_eq!(data.data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn add_image_to_entity_works_on_a_keyless_entity() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();

        add_image_to_entity(
            &mut conn,
            id,
            "image/jpeg".to_string(),
            vec![9, 9],
            test_provenance(),
        )
        .unwrap();

        let images = list_entity_images(&conn, id).unwrap();
        assert_eq!(images.len(), 1);
    }

    #[test]
    fn add_image_to_entity_errors_on_an_unknown_entity() {
        let mut conn = test_conn();
        let bogus = EntityId(Uuid::new_v4());

        let err = add_image_to_entity(
            &mut conn,
            bogus,
            "image/png".to_string(),
            vec![],
            test_provenance(),
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::EntityNotFound(_)));
    }

    #[test]
    fn list_entity_images_orders_newest_first_and_flags_current() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            Some("erin".to_string()),
            vec![],
            test_provenance(),
        )
        .unwrap();

        let mut older = test_provenance();
        older.collected_at_unix_ms = 1000;
        let older_fact =
            add_image_to_entity(&mut conn, id, "image/png".to_string(), vec![1], older).unwrap();

        let mut newer = test_provenance();
        newer.collected_at_unix_ms = 2000;
        let newer_fact =
            add_image_to_entity(&mut conn, id, "image/png".to_string(), vec![2], newer).unwrap();

        let images = list_entity_images(&conn, id).unwrap();
        assert_eq!(
            images.len(),
            2,
            "neither image is ever hidden, it's a gallery"
        );
        assert_eq!(images[0].fact_id, newer_fact);
        assert!(images[0].is_current);
        assert_eq!(images[1].fact_id, older_fact);
        assert!(!images[1].is_current);
    }

    #[test]
    fn list_entity_images_is_a_gallery_not_a_single_slot() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            Some("frank".to_string()),
            vec![],
            test_provenance(),
        )
        .unwrap();

        for i in 0..3u8 {
            add_image_to_entity(
                &mut conn,
                id,
                "image/png".to_string(),
                vec![i],
                test_provenance(),
            )
            .unwrap();
        }

        let images = list_entity_images(&conn, id).unwrap();
        assert_eq!(
            images.len(),
            3,
            "no dedup/overwrite for images unlike keyed attributes"
        );
    }

    #[test]
    fn get_entity_image_errors_on_an_unknown_image_id() {
        let conn = test_conn();
        let bogus = crate::ImageId(Uuid::new_v4());

        let err = get_entity_image(&conn, bogus).unwrap_err();
        assert!(matches!(err, EngineError::ImageNotFound(_)));
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
    fn set_entity_position_upserts_and_list_returns_it() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();

        set_entity_position(&conn, id, 1.5, 2.5).unwrap();
        let positions = list_entity_positions(&conn).unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].entity_id, id);
        assert_eq!(positions[0].x, 1.5);
        assert_eq!(positions[0].y, 2.5);

        // Dragging again updates the same row rather than adding a second one.
        set_entity_position(&conn, id, 9.0, 9.0).unwrap();
        let positions = list_entity_positions(&conn).unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].x, 9.0);
        assert_eq!(positions[0].y, 9.0);
    }

    #[test]
    fn set_entity_position_errors_on_missing_entity() {
        let conn = test_conn();
        let bogus = EntityId(Uuid::new_v4());
        assert!(matches!(
            set_entity_position(&conn, bogus, 0.0, 0.0).unwrap_err(),
            EngineError::EntityNotFound(_)
        ));
    }

    #[test]
    fn list_entity_positions_omits_entities_that_were_never_dragged() {
        let mut conn = test_conn();
        let dragged = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();
        let _untouched = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();
        set_entity_position(&conn, dragged, 3.0, 4.0).unwrap();

        let positions = list_entity_positions(&conn).unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].entity_id, dragged);
    }

    #[test]
    fn merge_entities_drops_the_losers_dragged_position() {
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
        set_entity_position(&conn, b, 5.0, 6.0).unwrap();

        // Would fail under foreign_keys=ON if the loser's entity_positions
        // row weren't cleared before its entities row is deleted.
        merge_entities(&mut conn, a, b, actor()).unwrap();

        let positions = list_entity_positions(&conn).unwrap();
        assert!(positions.is_empty());
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

        let new_id = split_entity(
            &mut conn,
            id,
            vec![fact_id],
            EntityType::Person,
            None,
            actor(),
        )
        .unwrap();
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

        let err = split_entity(
            &mut conn,
            a,
            vec![b_fact_id],
            EntityType::Person,
            None,
            actor(),
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::FactNotFound(_, _)));
    }

    #[test]
    fn split_entity_creates_the_requested_type_not_the_sources() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![attr("username", "octocat")],
            test_provenance(),
        )
        .unwrap();
        let fact_id: String = conn
            .query_row(
                "SELECT id FROM facts WHERE entity_id = ?1",
                params![id.0.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let fact_id = FactId(Uuid::parse_str(&fact_id).unwrap());

        let new_id = split_entity(
            &mut conn,
            id,
            vec![fact_id],
            EntityType::Username,
            None,
            actor(),
        )
        .unwrap();

        let source = get_entity(&conn, id).unwrap();
        let new_entity = get_entity(&conn, new_id).unwrap();
        assert_eq!(source.entity_type, EntityType::Person, "source untouched");
        assert_eq!(new_entity.entity_type, EntityType::Username);
    }

    #[test]
    fn split_entity_without_a_key_falls_back_to_a_null_canonical_key_and_a_split_suffix_label() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![attr("note", "seen downtown")],
            test_provenance(),
        )
        .unwrap();
        let source = get_entity(&conn, id).unwrap();
        let fact_id: String = conn
            .query_row(
                "SELECT id FROM facts WHERE entity_id = ?1",
                params![id.0.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let fact_id = FactId(Uuid::parse_str(&fact_id).unwrap());

        let new_id = split_entity(
            &mut conn,
            id,
            vec![fact_id],
            EntityType::Document,
            None,
            actor(),
        )
        .unwrap();

        let new_entity = get_entity(&conn, new_id).unwrap();
        assert_eq!(new_entity.canonical_key, None);
        assert_eq!(
            new_entity.display_label,
            format!("{} (split)", source.display_label)
        );
    }

    // The bug report this fixes: splitting a username fact off a Person
    // into a new Username entity with no way to give it a canonical_key
    // meant the new entity could never be a scan target — both the GUI's
    // target picker and find_entity_by_key require one (see split_entity's
    // own doc comment). Supplying `key` here proves the new entity is
    // actually reachable the same way any normally-added entity is.
    #[test]
    fn split_entity_with_a_key_produces_a_findable_scan_target() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![attr("username", "octocat")],
            test_provenance(),
        )
        .unwrap();
        let fact_id: String = conn
            .query_row(
                "SELECT id FROM facts WHERE entity_id = ?1",
                params![id.0.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let fact_id = FactId(Uuid::parse_str(&fact_id).unwrap());

        let new_id = split_entity(
            &mut conn,
            id,
            vec![fact_id],
            EntityType::Username,
            Some("Octocat".to_string()),
            actor(),
        )
        .unwrap();

        let new_entity = get_entity(&conn, new_id).unwrap();
        assert_eq!(new_entity.canonical_key.as_deref(), Some("octocat"));
        assert_eq!(new_entity.display_label, "Octocat");

        let found = find_entity_by_key(&conn, EntityType::Username, "octocat")
            .unwrap()
            .expect("split-off entity must be findable by its new key");
        assert_eq!(found.id, new_id);
    }

    #[test]
    fn redact_fact_deletes_only_the_targeted_facts_attributes() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Username,
            Some("carol".to_string()),
            vec![attr("full_name", "Wrongly Collected")],
            test_provenance(),
        )
        .unwrap();
        // A second, separate fact on the *same* entity (same canonical
        // key auto-merges rather than creating a new entity), which
        // redaction must leave untouched.
        let mut later = test_provenance();
        later.collected_at_unix_ms = 2000;
        add_entity(
            &mut conn,
            EntityType::Username,
            Some("carol".to_string()),
            vec![attr("nickname", "keep-me")],
            later,
        )
        .unwrap();

        let target_fact_id: String = conn
            .query_row(
                "SELECT fact_id FROM entity_attributes WHERE key = 'full_name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let target_fact_id = FactId(Uuid::parse_str(&target_fact_id).unwrap());

        redact_fact(
            &mut conn,
            target_fact_id,
            actor(),
            "wrongly collected, legal request",
        )
        .unwrap();

        let attrs = list_attribute_records(&conn, id).unwrap();
        assert_eq!(
            attrs.len(),
            1,
            "only the untargeted fact's attribute survives"
        );
        assert_eq!(attrs[0].key, "nickname");

        let fact_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts WHERE id = ?1",
                params![target_fact_id.0.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fact_count, 0, "the redacted fact row itself must be gone");

        // The entity itself survives, as a graph node.
        let entity = get_entity(&conn, id).unwrap();
        assert_eq!(entity.id, id);
    }

    #[test]
    fn redact_fact_deletes_the_associated_image_too() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();
        let fact_id = add_image_to_entity(
            &mut conn,
            id,
            "image/png".to_string(),
            vec![1, 2, 3],
            test_provenance(),
        )
        .unwrap();

        redact_fact(&mut conn, fact_id, actor(), "wrong photo uploaded").unwrap();

        assert!(list_entity_images(&conn, id).unwrap().is_empty());
        let fact_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts WHERE id = ?1",
                params![fact_id.0.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fact_count, 0);

        let events = audit_trail(&conn, AuditTarget::Entity(id)).unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            !events[0].description.contains("image/png"),
            "the audit trail must not need to carry image content"
        );
    }

    #[test]
    fn redact_fact_records_a_permanent_audit_event_without_leaking_the_value() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![attr("ssn", "123-45-6789")],
            test_provenance(),
        )
        .unwrap();
        let fact_id: String = conn
            .query_row(
                "SELECT id FROM facts WHERE entity_id = ?1",
                params![id.0.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let fact_id = FactId(Uuid::parse_str(&fact_id).unwrap());

        redact_fact(&mut conn, fact_id, actor(), "PII, court order 2026-CV-1234").unwrap();

        let events = audit_trail(&conn, AuditTarget::Entity(id)).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "redact");
        assert_eq!(events[0].actor, "tester");
        assert!(events[0].description.contains(&fact_id.to_string()));
        assert!(events[0].description.contains("court order 2026-CV-1234"));
        assert!(
            !events[0].description.contains("123-45-6789"),
            "the redacted value itself must never appear in the audit trail: {}",
            events[0].description
        );
    }

    #[test]
    fn redact_fact_works_on_a_relationship_backed_fact_too() {
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
            vec![attr("role", "wrongly attributed")],
            test_provenance(),
        )
        .unwrap();
        let fact_id: String = conn
            .query_row(
                "SELECT id FROM facts WHERE relationship_id = ?1",
                params![rel_id.0.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let fact_id = FactId(Uuid::parse_str(&fact_id).unwrap());

        redact_fact(&mut conn, fact_id, actor(), "wrong attribution").unwrap();

        let attrs = list_relationship_attribute_records(&conn, rel_id).unwrap();
        assert!(attrs.is_empty());

        let events = audit_trail(&conn, AuditTarget::Relationship(rel_id)).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "redact");
    }

    #[test]
    fn redact_fact_errors_on_an_unknown_fact_id() {
        let mut conn = test_conn();
        let missing = FactId(Uuid::new_v4());
        let err = redact_fact(&mut conn, missing, actor(), "n/a").unwrap_err();
        assert!(matches!(err, EngineError::UnknownFact(_)));
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

    #[test]
    fn list_attribute_records_includes_the_originating_fact_id() {
        let mut conn = test_conn();
        let id = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![attr("a", "1")],
            test_provenance(),
        )
        .unwrap();

        let fact_id: String = conn
            .query_row(
                "SELECT id FROM facts WHERE entity_id = ?1",
                params![id.0.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let fact_id = FactId(Uuid::parse_str(&fact_id).unwrap());

        let attrs = list_attribute_records(&conn, id).unwrap();
        assert_eq!(
            attrs[0].fact_id, fact_id,
            "entity split --facts needs this id, and has no other way to learn it"
        );
    }

    #[test]
    fn list_relationships_and_their_attribute_records() {
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

        let rels = list_relationships(&conn).unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].id, rel_id);
        assert_eq!(rels[0].from, a);
        assert_eq!(rels[0].to, b);
        assert_eq!(rels[0].relationship_type, RelationshipType::MemberOf);

        let attrs = list_relationship_attribute_records(&conn, rel_id).unwrap();
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0].key, "role");
        assert_eq!(attrs[0].value, "engineer");
        assert!(attrs[0].is_current);
    }

    #[test]
    fn list_relationship_attribute_records_errors_on_missing_relationship() {
        let conn = test_conn();
        let missing = RelationshipId(Uuid::new_v4());
        let err = list_relationship_attribute_records(&conn, missing).unwrap_err();
        assert!(matches!(err, EngineError::RelationshipNotFound(_)));
    }

    #[test]
    fn case_stats_counts_entities_facts_relationships_and_conflicts() {
        let mut conn = test_conn();
        let a = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![attr("name", "Alice"), attr("nickname", "Al")],
            test_provenance(),
        )
        .unwrap();
        // A second fact for the same key with a different value — makes
        // "name" a conflicting attribute on entity a.
        add_entity(
            &mut conn,
            EntityType::Person,
            Some("bob-key".to_string()),
            vec![attr("name", "Bob")],
            test_provenance(),
        )
        .unwrap();
        let b_for_rel = add_entity(
            &mut conn,
            EntityType::Organization,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();
        add_relationship(
            &mut conn,
            a,
            b_for_rel,
            RelationshipType::MemberOf,
            vec![],
            test_provenance(),
        )
        .unwrap();
        // Second fact on entity a's "name" key, different value: conflict.
        add_attribute_to_existing_entity(&mut conn, a, "name", "Alicia");

        let stats = case_stats(&conn).unwrap();
        assert_eq!(stats.entity_count, 3);
        // One fact per add_entity/add_relationship call regardless of how
        // many attrs it carries (all attrs share that one fact_id) — a's
        // creation (1), bob's (1), b_for_rel's (1), the relationship (1),
        // plus the extra "name" fact added below (1) = 5.
        assert_eq!(stats.fact_count, 5);
        assert_eq!(stats.relationship_count, 1);
        assert_eq!(stats.conflicting_entity_count, 1);
    }

    /// Test-only helper: adds one more fact/entity_attribute row directly,
    /// simulating a later correction to an existing entity's attribute
    /// (append-only, same as `add_entity`'s own fact-per-attribute shape)
    /// without going through the public API, which has no "add a fact to
    /// an existing entity" operation yet (SPEC.md §8 leaves that to plugin
    /// ingestion / a future manual-edit command).
    fn add_attribute_to_existing_entity(
        conn: &mut Connection,
        id: EntityId,
        key: &str,
        value: &str,
    ) {
        let p = test_provenance();
        let fact_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO facts (id, entity_id, source, source_version, confidence_status, collected_at)
             VALUES (?1, ?2, ?3, ?4, 'FOUND', ?5)",
            params![fact_id.to_string(), id.0.to_string(), p.source, p.source_version, now_unix_ms()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entity_attributes (id, entity_id, fact_id, key, value)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                Uuid::new_v4().to_string(),
                id.0.to_string(),
                fact_id.to_string(),
                key,
                value
            ],
        )
        .unwrap();
    }

    #[test]
    fn audit_trail_all_returns_every_event_newest_first() {
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
            vec![attr("name", "Bob")],
            test_provenance(),
        )
        .unwrap();
        let c = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![attr("name", "Carol")],
            test_provenance(),
        )
        .unwrap();
        merge_entities(&mut conn, a, b, actor()).unwrap();
        merge_entities(&mut conn, a, c, actor()).unwrap();

        let events = audit_trail_all(&conn, 10).unwrap();
        assert_eq!(events.len(), 2);
        // Newest first: the a+c merge happened after the a+b merge.
        assert!(events[0].occurred_at_unix_ms >= events[1].occurred_at_unix_ms);
        assert!(events.iter().all(|e| e.event_type == "merge"));
    }

    #[test]
    fn audit_trail_all_respects_the_limit() {
        let mut conn = test_conn();
        let a = add_entity(
            &mut conn,
            EntityType::Person,
            None,
            vec![],
            test_provenance(),
        )
        .unwrap();
        for _ in 0..3 {
            let b = add_entity(
                &mut conn,
                EntityType::Person,
                None,
                vec![],
                test_provenance(),
            )
            .unwrap();
            merge_entities(&mut conn, a, b, actor()).unwrap();
        }
        let events = audit_trail_all(&conn, 2).unwrap();
        assert_eq!(events.len(), 2);
    }
}
