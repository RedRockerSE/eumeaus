-- Additive, idempotent schema changes made after the public v0.1.0-v0.1.4
-- releases (there is no migration/schema_version-checking system — see
-- Case::open's doc comment). Every statement here MUST be safe to run
-- twice: once as part of SCHEMA_SQL at Case::create time (a fresh
-- database, nothing here exists yet), and again, unconditionally, on
-- every Case::open (an already-existing case file that predates this
-- file never got these tables any other way). CREATE TABLE/INDEX IF NOT
-- EXISTS only — anything else here will error the second time it runs
-- against a real user's case.

-- Phase one of entity image upload: attach an image BLOB to any entity
-- (no entity type is privileged as a root — SPEC.md §4.2). Mirrors
-- entity_attributes' shape (own `id` PK, non-unique `fact_id`) rather
-- than making fact_id the primary key, so a single fact/finding can
-- later carry more than one image (e.g. a future plugin scraping a
-- profile with several gallery photos in one CheckResult) without a
-- structural change.
CREATE TABLE IF NOT EXISTS entity_images (
    id TEXT PRIMARY KEY,
    entity_id TEXT NOT NULL REFERENCES entities (id),
    fact_id TEXT NOT NULL REFERENCES facts (id),
    mime_type TEXT NOT NULL,
    data BLOB NOT NULL
);

CREATE INDEX IF NOT EXISTS entity_images_entity_id_idx
    ON entity_images (entity_id);

-- Persists the Link graph's drag-to-reposition (SPEC.md §9.3): one row per
-- entity the user has actually dragged, entity_id itself as the PK since
-- there is at most one current position per entity (no history/provenance
-- needed — this is view state, not case data). An entity with no row here
-- just hasn't been dragged yet; the GUI's own circle layout fills the gap.
CREATE TABLE IF NOT EXISTS entity_positions (
    entity_id TEXT PRIMARY KEY REFERENCES entities (id),
    x REAL NOT NULL,
    y REAL NOT NULL
);
