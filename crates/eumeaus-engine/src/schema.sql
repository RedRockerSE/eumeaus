-- Core schema per SPEC.md §4.2. Applied once, inside a transaction, when a
-- case is created. No entity type is privileged as a root.

CREATE TABLE case_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE entities (
    id TEXT PRIMARY KEY,
    entity_type TEXT NOT NULL,
    canonical_key TEXT,
    display_label TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Backs exact-key auto-merge on ingestion (SPEC.md §4.4): a second fact with
-- the same (entity_type, canonical_key) merges into the existing entity
-- instead of creating a new row.
CREATE UNIQUE INDEX entities_type_canonical_key_idx
    ON entities (entity_type, canonical_key)
    WHERE canonical_key IS NOT NULL;

CREATE TABLE relationships (
    id TEXT PRIMARY KEY,
    from_entity_id TEXT NOT NULL REFERENCES entities (id),
    to_entity_id TEXT NOT NULL REFERENCES entities (id),
    relationship_type TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE scans (
    id TEXT PRIMARY KEY,
    target_entity_id TEXT NOT NULL REFERENCES entities (id),
    config_snapshot TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at INTEGER,
    completed_at INTEGER
);

CREATE TABLE scan_plugin_runs (
    id TEXT PRIMARY KEY,
    scan_id TEXT NOT NULL REFERENCES scans (id),
    plugin_name TEXT NOT NULL,
    plugin_version TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at INTEGER,
    completed_at INTEGER,
    error_message TEXT
);

-- Append-only provenance/audit log. Rows here are never updated or deleted;
-- corrections are new fact rows (SPEC.md §4.2).
CREATE TABLE facts (
    id TEXT PRIMARY KEY,
    entity_id TEXT REFERENCES entities (id),
    relationship_id TEXT REFERENCES relationships (id),
    scan_id TEXT REFERENCES scans (id),
    source TEXT NOT NULL,
    source_version TEXT NOT NULL,
    confidence_status TEXT NOT NULL,
    source_url TEXT,
    retrieval_method TEXT,
    raw_response_sha256 TEXT,
    collected_at INTEGER NOT NULL,
    CHECK ((entity_id IS NULL) <> (relationship_id IS NULL))
);

CREATE TABLE entity_attributes (
    id TEXT PRIMARY KEY,
    entity_id TEXT NOT NULL REFERENCES entities (id),
    fact_id TEXT NOT NULL REFERENCES facts (id),
    key TEXT NOT NULL,
    value TEXT NOT NULL
);

-- Records merges, splits, and other structural edits; distinct from `facts`,
-- which record collected data. Append-only.
CREATE TABLE audit_events (
    id TEXT PRIMARY KEY,
    entity_id TEXT REFERENCES entities (id),
    relationship_id TEXT REFERENCES relationships (id),
    event_type TEXT NOT NULL,
    description TEXT NOT NULL,
    actor TEXT NOT NULL,
    occurred_at INTEGER NOT NULL,
    CHECK ((entity_id IS NULL) OR (relationship_id IS NULL))
);
