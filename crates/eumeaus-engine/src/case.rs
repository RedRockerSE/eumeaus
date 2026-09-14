//! `Case` lifecycle: create/open/close over a SQLCipher-encrypted SQLite
//! file (SPEC.md §4.1). The encryption key lives in the OS-native
//! credential store, referenced by the case's UUID (see [`crate::keystore`]);
//! it never touches the case file itself.
//!
//! Opening a case requires knowing its UUID before the database can be
//! decrypted, which is itself stored *inside* the encrypted database — so a
//! small plaintext sidecar file (`<case>.eum.meta`, just the UUID) sits next
//! to the case file purely to break that chicken-and-egg. It carries no
//! secret; the encryption key stays in the OS keychain.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, ErrorCode};
use uuid::Uuid;

use crate::{
    crud, keystore, Actor, Attribute, AttributeRecord, AuditEvent, AuditTarget, CaseStats,
    CaseSummary, DocumentId, EngineError, Entity, EntityDocumentData, EntityDocumentSummary,
    EntityFilter, EntityId, EntityImageData, EntityImageSummary, EntityPosition, EntityType,
    FactId, ImageId, MapPoint, PluginRef, Provenance, Relationship, RelationshipId,
    RelationshipType, ScanConfig, ScanId, ScanStatus, ScanSummary, TargetEntity,
};

const SCHEMA_SQL: &str = include_str!("schema.sql");
// Idempotent CREATE TABLE/INDEX IF NOT EXISTS statements only — applied
// both at Case::create time (below) and unconditionally on every
// Case::open, since there is no schema-version-checked migration system.
// See schema_additions.sql's own header comment for why.
const SCHEMA_ADDITIONS_SQL: &str = include_str!("schema_additions.sql");
const SCHEMA_VERSION: &str = "2";

pub enum ExportFormat {
    /// Plaintext (unencrypted) SQLite copy — no key needed to read it back,
    /// so treat the output as sensitive. Documented at [`Case::export`].
    Sqlite,
    /// Human-readable JSON dump of the entity/relationship graph. See
    /// [`Case::export`].
    Report,
    /// A SQLCipher copy re-keyed with a passphrase instead of the local
    /// keychain key (SPEC.md §8 open question 1) — meant to be handed to
    /// another investigator/machine and turned back into a normal case via
    /// [`Case::import`]. The `String` is the passphrase; must be non-empty.
    Portable(String),
    /// Self-contained HTML dump of the same data `Report` covers, styled
    /// for a human reader rather than machine parsing (SPEC.md §8 open
    /// question 6) — openable in any browser, print-to-PDF-able from
    /// there. See [`Case::export`].
    Html,
}

/// Opaque handle over an open, decrypted case DB connection. Held under
/// SQLite's own `locking_mode = EXCLUSIVE` (see `init_case_file`'s doc
/// comment for why) rather than a separate `std::fs`-level lock, so the
/// OS file lock is released the same way: when `conn` is dropped (or
/// [`Case::close`] is called).
pub struct Case {
    path: PathBuf,
    case_id: Uuid,
    name: String,
    conn: Connection,
}

impl std::fmt::Debug for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Case")
            .field("path", &self.path)
            .field("case_id", &self.case_id)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Case {
    /// Creates a fresh encrypted case at `<path>/<name>.eum`, generates and
    /// stores its encryption key in the OS keychain, and applies the core
    /// schema (SPEC.md §4.2).
    pub fn create(path: &Path, name: &str) -> Result<Case, EngineError> {
        fs::create_dir_all(path)?;
        let case_path = path.join(format!("{name}.eum"));
        if case_path.exists() {
            return Err(EngineError::CaseAlreadyExists(case_path));
        }

        let case_id = Uuid::new_v4();
        let hex_key = keystore::create_key(case_id)?;

        match Self::init_case_file(&case_path, case_id, name, &hex_key) {
            Ok(case) => Ok(case),
            Err(err) => {
                let _ = fs::remove_file(&case_path);
                let _ = fs::remove_file(meta_path_for(&case_path));
                let _ = keystore::delete_key(case_id);
                Err(err)
            }
        }
    }

    fn init_case_file(
        case_path: &Path,
        case_id: Uuid,
        name: &str,
        hex_key: &str,
    ) -> Result<Case, EngineError> {
        let mut conn = Connection::open(case_path)?;
        apply_key(&conn, hex_key)?;
        set_exclusive_locking(&conn)?;

        let now = crate::now_unix_ms();
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA_SQL)?;
        tx.execute_batch(SCHEMA_ADDITIONS_SQL)?;
        {
            let mut insert_meta =
                tx.prepare("INSERT INTO case_meta (key, value) VALUES (?1, ?2)")?;
            insert_meta.execute(params!["case_id", case_id.to_string()])?;
            insert_meta.execute(params!["name", name])?;
            insert_meta.execute(params!["schema_version", SCHEMA_VERSION])?;
            insert_meta.execute(params!["created_at", now.to_string()])?;
        }
        tx.commit()?;

        fs::write(meta_path_for(case_path), case_id.to_string())?;

        Ok(Case {
            path: case_path.to_path_buf(),
            case_id,
            name: name.to_string(),
            conn,
        })
    }

    /// Opens an existing case, acquiring an exclusive OS file lock for the
    /// duration. A second attempt to open the same case file fails fast
    /// with [`EngineError::CaseAlreadyOpen`] rather than risking
    /// concurrent-write corruption.
    pub fn open(path: &Path) -> Result<Case, EngineError> {
        if !path.exists() {
            return Err(EngineError::CaseNotFound(path.to_path_buf()));
        }

        let case_id = read_case_id(path)?;
        let hex_key = keystore::load_key(case_id)?;

        let conn = Connection::open(path)?;
        apply_key(&conn, &hex_key)?;
        set_exclusive_locking(&conn)?;
        // The first statement that actually touches the file (this one)
        // is where a second process's conflicting lock surfaces as
        // SQLITE_BUSY — verify_decryption maps that to CaseAlreadyOpen.
        verify_decryption(&conn, path)?;
        // No schema-version-checked migration system exists — this case
        // file may predate any table added to schema_additions.sql after
        // it was created. Re-applying idempotent IF NOT EXISTS DDL on
        // every open is the whole migration story (see the constant's
        // doc comment above).
        conn.execute_batch(SCHEMA_ADDITIONS_SQL)?;
        crate::scan::reconcile_orphaned_runs(&conn)?;

        let name = conn.query_row(
            "SELECT value FROM case_meta WHERE key = 'name'",
            [],
            |row| row.get(0),
        )?;

        Ok(Case {
            path: path.to_path_buf(),
            case_id,
            name,
            conn,
        })
    }

    /// Closes the case, releasing the file lock and the database
    /// connection. Equivalent to dropping the `Case`; provided explicitly
    /// so callers can observe close-time errors.
    pub fn close(self) -> Result<(), EngineError> {
        drop(self);
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn id(&self) -> Uuid {
        self.case_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Test-only escape hatch: `scan.rs`'s tests need to poke
    /// `scan_plugin_runs` rows directly (e.g. to simulate a crash by
    /// forcing a row to `RUNNING`) to test crash reconciliation without
    /// actually killing a process. Not part of the public API.
    #[cfg(test)]
    pub(crate) fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Lists every `.eum` file directly inside `dir` (SPEC.md §3.4 `case
    /// list`), without opening or decrypting any of them — see
    /// [`CaseSummary`].
    pub fn list(dir: &Path) -> Result<Vec<CaseSummary>, EngineError> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(EngineError::Io(e)),
        };

        let mut summaries = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("eum") {
                continue;
            }
            let id = match read_case_id(&path) {
                Ok(id) => id,
                Err(_) => {
                    eprintln!(
                        "warning: skipping {}: missing or invalid sidecar metadata",
                        path.display()
                    );
                    continue;
                }
            };
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            summaries.push(CaseSummary { path, name, id });
        }
        summaries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(summaries)
    }

    /// Exports the case per `format`. `ExportFormat::Sqlite` produces a
    /// plaintext (unencrypted) SQLite copy of the whole database via
    /// SQLCipher's `sqlcipher_export()` — treat the output file as
    /// sensitive; it is *not* a portability/handoff mechanism (that's
    /// `ExportFormat::Portable`, below). `ExportFormat::Report`/`::Html`
    /// produce, respectively, a JSON and an HTML dump of every
    /// entity/relationship, their attributes, and their audit trail —
    /// SPEC.md §8 open question 6 (evidentiary report format); pair either
    /// with `crate::report::sign_export` for tamper-evidence, since unlike
    /// the SQLCipher formats these are plaintext. `ExportFormat::Portable`
    /// answers §8's open question 1: it produces a SQLCipher file re-keyed
    /// with the given passphrase (SQLCipher's own passphrase mode —
    /// PBKDF2 over the passphrase, a random salt stored in the file
    /// header; no hand-rolled KDF here) — safe to hand to another
    /// investigator/machine, who turns it back into a normal local case
    /// with [`Case::import`].
    pub fn export(&self, dest: &Path, format: ExportFormat) -> Result<(), EngineError> {
        if dest.exists() {
            return Err(EngineError::ExportDestinationExists(dest.to_path_buf()));
        }
        match format {
            ExportFormat::Sqlite => export_sqlite(&self.conn, dest, ""),
            ExportFormat::Report => export_report(self, dest),
            ExportFormat::Html => export_html(self, dest),
            ExportFormat::Portable(passphrase) => {
                if passphrase.is_empty() {
                    return Err(EngineError::EmptyPassphrase);
                }
                export_sqlite(&self.conn, dest, &passphrase)
            }
        }
    }

    /// Imports a portable export (`ExportFormat::Portable`) back into a
    /// normal local case: decrypts `source` with `passphrase`, then
    /// re-encrypts it under a brand-new random key generated and stored in
    /// *this* machine's OS keychain — the same shape as [`Case::create`]
    /// (fresh UUID, fresh key, fresh `.eum.meta` sidecar), rather than
    /// teaching [`Case::open`] to understand two different key sources.
    /// From the moment this returns, the result is an entirely ordinary
    /// case; `source`'s passphrase is never needed again.
    pub fn import(
        source: &Path,
        passphrase: &str,
        dest_dir: &Path,
        name: &str,
    ) -> Result<Case, EngineError> {
        if passphrase.is_empty() {
            return Err(EngineError::EmptyPassphrase);
        }
        if !source.exists() {
            return Err(EngineError::CaseNotFound(source.to_path_buf()));
        }

        fs::create_dir_all(dest_dir)?;
        let dest_path = dest_dir.join(format!("{name}.eum"));
        if dest_path.exists() {
            return Err(EngineError::CaseAlreadyExists(dest_path));
        }

        let case_id = Uuid::new_v4();
        let hex_key = keystore::create_key(case_id)?;

        match Self::import_impl(source, passphrase, &dest_path, case_id, name, &hex_key) {
            Ok(case) => Ok(case),
            Err(err) => {
                let _ = fs::remove_file(&dest_path);
                let _ = fs::remove_file(meta_path_for(&dest_path));
                let _ = keystore::delete_key(case_id);
                Err(err)
            }
        }
    }

    fn import_impl(
        source: &Path,
        passphrase: &str,
        dest_path: &Path,
        case_id: Uuid,
        name: &str,
        hex_key: &str,
    ) -> Result<Case, EngineError> {
        let dest_str = dest_path.to_str().ok_or_else(|| {
            EngineError::CaseCorrupt(
                dest_path.to_path_buf(),
                "destination path is not valid UTF-8".to_string(),
            )
        })?;

        {
            // `PRAGMA key` doesn't reliably accept a bound parameter (it's
            // parsed differently from a normal expression position), so the
            // passphrase is escaped and interpolated instead — standard SQL
            // string-literal escaping (doubling an embedded `'`) is
            // sufficient and correct here, there's no other special
            // character in a single-quoted SQL string literal.
            let source_conn = Connection::open(source)?;
            let escaped_passphrase = passphrase.replace('\'', "''");
            source_conn.execute_batch(&format!("PRAGMA key = '{escaped_passphrase}'"))?;
            verify_decryption(&source_conn, source)?;

            // Unlike the passphrase above, the destination's key is the
            // keychain's raw hex key — same `x'<hex>'` raw-key syntax
            // `apply_key` uses elsewhere, so a later `Case::open` decrypts
            // this file the normal way. That syntax only works as a
            // literal in the SQL text (not as a bound parameter's runtime
            // value), but `hex_key` is always exactly 64 lowercase hex
            // digits from `keystore::create_key`, so interpolating it here
            // carries no injection risk.
            source_conn.execute(
                &format!("ATTACH DATABASE ?1 AS import_target KEY \"x'{hex_key}'\""),
                params![dest_str],
            )?;
            source_conn.query_row("SELECT sqlcipher_export('import_target')", [], |row| {
                row.get::<_, Option<i64>>(0)
            })?;
            source_conn.execute("DETACH DATABASE import_target", [])?;
        }

        // sqlcipher_export copied the portable file's *original*
        // case_meta rows (its old case_id/name) verbatim — re-point them
        // at this machine's actual new identity so case_meta agrees with
        // the sidecar/keychain UUID Case::open relies on.
        let conn = Connection::open(dest_path)?;
        apply_key(&conn, hex_key)?;
        set_exclusive_locking(&conn)?;
        conn.execute(
            "UPDATE case_meta SET value = ?1 WHERE key = 'case_id'",
            params![case_id.to_string()],
        )?;
        conn.execute(
            "UPDATE case_meta SET value = ?1 WHERE key = 'name'",
            params![name],
        )?;

        fs::write(meta_path_for(dest_path), case_id.to_string())?;

        Ok(Case {
            path: dest_path.to_path_buf(),
            case_id,
            name: name.to_string(),
            conn,
        })
    }

    pub fn list_scans(&self) -> Result<Vec<ScanSummary>, EngineError> {
        crate::scan::list_scans(&self.conn)
    }

    pub fn add_entity(
        &mut self,
        entity_type: EntityType,
        key: Option<String>,
        attrs: Vec<Attribute>,
        provenance: Provenance,
    ) -> Result<EntityId, EngineError> {
        crud::add_entity(&mut self.conn, entity_type, key, attrs, provenance)
    }

    /// Same as [`Case::add_entity`], but also reports whether a new entity
    /// row was actually inserted (`true`) vs. an existing `(entity_type,
    /// canonical_key)` match was appended to instead (`false`) — the GUI's
    /// auto-scan-on-add feature (SPEC.md §9.3) needs this so re-adding/
    /// touching an already-known entity doesn't re-trigger a scan.
    pub fn add_entity_with_outcome(
        &mut self,
        entity_type: EntityType,
        key: Option<String>,
        attrs: Vec<Attribute>,
        provenance: Provenance,
    ) -> Result<(EntityId, bool), EngineError> {
        crud::add_entity_with_outcome(&mut self.conn, entity_type, key, attrs, provenance)
    }

    pub fn add_fact_to_entity(
        &mut self,
        entity_id: EntityId,
        attrs: Vec<Attribute>,
        provenance: Provenance,
    ) -> Result<FactId, EngineError> {
        crud::add_fact_to_entity(&mut self.conn, entity_id, attrs, provenance)
    }

    pub fn add_image_to_entity(
        &mut self,
        entity_id: EntityId,
        mime_type: String,
        data: Vec<u8>,
        provenance: Provenance,
    ) -> Result<FactId, EngineError> {
        crud::add_image_to_entity(&mut self.conn, entity_id, mime_type, data, provenance)
    }

    pub fn list_entity_images(
        &self,
        entity_id: EntityId,
    ) -> Result<Vec<EntityImageSummary>, EngineError> {
        crud::list_entity_images(&self.conn, entity_id)
    }

    pub fn get_entity_image(&self, image_id: ImageId) -> Result<EntityImageData, EngineError> {
        crud::get_entity_image(&self.conn, image_id)
    }

    pub fn add_document_to_entity(
        &mut self,
        entity_id: EntityId,
        file_name: String,
        mime_type: String,
        data: Vec<u8>,
        provenance: Provenance,
    ) -> Result<FactId, EngineError> {
        crud::add_document_to_entity(
            &mut self.conn,
            entity_id,
            file_name,
            mime_type,
            data,
            provenance,
        )
    }

    pub fn list_entity_documents(
        &self,
        entity_id: EntityId,
    ) -> Result<Vec<EntityDocumentSummary>, EngineError> {
        crud::list_entity_documents(&self.conn, entity_id)
    }

    pub fn get_entity_document(
        &self,
        document_id: DocumentId,
    ) -> Result<EntityDocumentData, EngineError> {
        crud::get_entity_document(&self.conn, document_id)
    }

    pub fn set_entity_position(
        &mut self,
        entity_id: EntityId,
        x: f64,
        y: f64,
    ) -> Result<(), EngineError> {
        crud::set_entity_position(&self.conn, entity_id, x, y)
    }

    pub fn list_entity_positions(&self) -> Result<Vec<EntityPosition>, EngineError> {
        crud::list_entity_positions(&self.conn)
    }

    pub fn merge_entities(
        &mut self,
        a: EntityId,
        b: EntityId,
        actor: Actor,
    ) -> Result<EntityId, EngineError> {
        crud::merge_entities(&mut self.conn, a, b, actor)
    }

    pub fn split_entity(
        &mut self,
        id: EntityId,
        fact_ids: Vec<FactId>,
        entity_type: EntityType,
        key: Option<String>,
        actor: Actor,
    ) -> Result<EntityId, EngineError> {
        crud::split_entity(&mut self.conn, id, fact_ids, entity_type, key, actor)
    }

    pub fn redact_fact(
        &mut self,
        fact_id: FactId,
        actor: Actor,
        reason: &str,
    ) -> Result<(), EngineError> {
        crud::redact_fact(&mut self.conn, fact_id, actor, reason)
    }

    /// Reversibly dismisses an entity (issue #9) — see
    /// `crud::hide_entity`'s doc comment for why this isn't a real delete.
    pub fn hide_entity(
        &mut self,
        id: EntityId,
        actor: Actor,
        reason: Option<&str>,
    ) -> Result<(), EngineError> {
        crud::hide_entity(&mut self.conn, id, actor, reason)
    }

    /// Reverses [`Case::hide_entity`].
    pub fn unhide_entity(&mut self, id: EntityId, actor: Actor) -> Result<(), EngineError> {
        crud::unhide_entity(&mut self.conn, id, actor)
    }

    pub fn add_relationship(
        &mut self,
        from: EntityId,
        to: EntityId,
        rel_type: RelationshipType,
        attrs: Vec<Attribute>,
        provenance: Provenance,
    ) -> Result<RelationshipId, EngineError> {
        crud::add_relationship(&mut self.conn, from, to, rel_type, attrs, provenance)
    }

    pub fn list_entities(&self, filter: EntityFilter) -> Result<Vec<Entity>, EngineError> {
        crud::list_entities(&self.conn, filter)
    }

    /// Not part of SPEC.md §3.1's illustrative API, but `entity show <id>`
    /// (§3.4) needs a way to fetch a single entity by id.
    pub fn get_entity(&self, id: EntityId) -> Result<Entity, EngineError> {
        crud::get_entity(&self.conn, id)
    }

    /// Not in §3.1 either; backs `scan run --target-type --target-value`
    /// (§3.4), which names a scan's target by key rather than id.
    pub fn find_entity_by_key(
        &self,
        entity_type: EntityType,
        key: &str,
    ) -> Result<Option<Entity>, EngineError> {
        crud::find_entity_by_key(&self.conn, entity_type, key)
    }

    /// Also not in §3.1; backs `entity show`'s attribute listing.
    pub fn list_attribute_records(
        &self,
        id: EntityId,
    ) -> Result<Vec<AttributeRecord>, EngineError> {
        crud::list_attribute_records(&self.conn, id)
    }

    pub fn audit_trail(&self, target: AuditTarget) -> Result<Vec<AuditEvent>, EngineError> {
        crud::audit_trail(&self.conn, target)
    }

    /// Every audit event in the case, newest first, capped at `limit` —
    /// backs the GUI's Overview screen (SPEC.md §9.3), which has no single
    /// target the way [`Case::audit_trail`] does.
    pub fn audit_trail_all(&self, limit: u32) -> Result<Vec<AuditEvent>, EngineError> {
        crud::audit_trail_all(&self.conn, limit)
    }

    /// Not in SPEC.md §3.1 (no `relationship list` CLI command exists
    /// either — see `crud::list_relationships`'s own doc); added for the
    /// GUI's Graph screen (SPEC.md §9.3), which needs the full graph.
    pub fn list_relationships(
        &self,
        include_hidden: bool,
    ) -> Result<Vec<Relationship>, EngineError> {
        crud::list_relationships(&self.conn, include_hidden)
    }

    /// Case-wide counts for the GUI's Overview screen (SPEC.md §9.3).
    pub fn case_stats(&self) -> Result<CaseStats, EngineError> {
        crud::case_stats(&self.conn)
    }

    /// Every plottable point for the GUI's Map screen (SPEC.md §9.3) —
    /// see [`crate::MapPoint`]'s own doc for the detection rules.
    pub fn map_points(&self) -> Result<Vec<MapPoint>, EngineError> {
        crud::list_map_points(&self.conn)
    }

    /// Runs `plugins` (or, if empty, every discovered plugin compatible
    /// with `target`'s entity type) against `target`, blocking until every
    /// one has reached SUCCESS/TIMEOUT/ERROR. See [`crate::scan::start`]
    /// for why this takes more/different parameters than SPEC.md §3.1's
    /// illustrative single-`PluginRef` signature.
    pub fn start_scan(
        &mut self,
        plugins_dir: &Path,
        plugins: Vec<PluginRef>,
        target: TargetEntity,
        config: ScanConfig,
        trust_policy: crate::TrustPolicy,
    ) -> Result<ScanId, EngineError> {
        let plugin_names: Vec<String> = plugins.into_iter().map(|p| p.name).collect();
        crate::scan::start(
            &mut self.conn,
            plugins_dir,
            &plugin_names,
            target,
            config,
            trust_policy,
        )
    }

    /// [`Case::start_scan`] split into "create the scan row and return its
    /// id" (this) plus "run it" ([`Case::resume_scan`], which already
    /// means "run every still-`PENDING` row"). Not in SPEC.md §3.1 at all
    /// — lets a caller learn the `ScanId` and print/log it before blocking
    /// on a scan that might run for a while or get killed mid-flight.
    pub fn create_scan(
        &mut self,
        plugins_dir: &Path,
        plugins: Vec<PluginRef>,
        target: TargetEntity,
        config: ScanConfig,
        trust_policy: &crate::TrustPolicy,
    ) -> Result<ScanId, EngineError> {
        let plugin_names: Vec<String> = plugins.into_iter().map(|p| p.name).collect();
        crate::scan::create(
            &mut self.conn,
            plugins_dir,
            &plugin_names,
            target,
            config,
            trust_policy,
        )
    }

    pub fn resume_scan(&mut self, scan_id: ScanId) -> Result<(), EngineError> {
        crate::scan::resume(&mut self.conn, scan_id.0, None)
    }

    /// Same as [`Case::resume_scan`], but also sends a
    /// [`crate::ScanProgressEvent`] on `progress` at each
    /// `scan_plugin_runs` status transition, synchronously, as it happens
    /// — added for the GUI (SPEC.md §9.6 G3), which has no other way to
    /// observe a scan mid-flight (see [`crate::ScanProgressEvent`]'s doc
    /// for why polling `scan_status` from elsewhere doesn't work here).
    pub fn resume_scan_with_progress(
        &mut self,
        scan_id: ScanId,
        progress: &crate::ScanProgressSender,
    ) -> Result<(), EngineError> {
        crate::scan::resume(&mut self.conn, scan_id.0, Some(progress))
    }

    pub fn scan_status(&self, scan_id: ScanId) -> Result<ScanStatus, EngineError> {
        crate::scan::status(&self.conn, scan_id.0)
    }
}

fn meta_path_for(case_path: &Path) -> PathBuf {
    let mut os_string = case_path.as_os_str().to_owned();
    os_string.push(".meta");
    PathBuf::from(os_string)
}

fn read_case_id(case_path: &Path) -> Result<Uuid, EngineError> {
    let meta_path = meta_path_for(case_path);
    let raw = fs::read_to_string(&meta_path).map_err(|_| {
        EngineError::CaseCorrupt(
            case_path.to_path_buf(),
            format!("missing sidecar metadata file {}", meta_path.display()),
        )
    })?;
    Uuid::parse_str(raw.trim()).map_err(|_| {
        EngineError::CaseCorrupt(
            case_path.to_path_buf(),
            "sidecar metadata file does not contain a valid case id".to_string(),
        )
    })
}

/// Puts `conn` into SQLite's own `EXCLUSIVE` locking mode, so its OS-level
/// file lock — once acquired on the connection's first real read or
/// write — is never released back to `NORMAL` until the connection is
/// dropped/closed. This is the *only* file lock a `Case` holds.
///
/// An earlier version of this instead opened a second `std::fs::File`
/// handle on the same path purely to hold a separate advisory
/// `File::try_lock()`. That worked on Linux (where it maps to `flock()`,
/// which SQLite's own Unix VFS locking — `fcntl()` byte-range locks —
/// never interacts with) but broke case creation/opening on Windows:
/// `File::try_lock()` there maps to `LockFileEx`, a *mandatory* lock, and
/// SQLite's Windows VFS also locks the file via `LockFileEx` — so our
/// own lock and SQLite's own subsequent open of the identical path
/// collided, surfacing to callers as a generic SQLite "disk I/O error"
/// (`SQLITE_IOERR`) on every `case create`/`case open`. Relying on
/// SQLite's own locking exclusively avoids ever taking two independent
/// locks on the same file, which is correct — and the only thing that
/// actually needs to be correct — on every platform SQLite supports.
fn set_exclusive_locking(conn: &Connection) -> Result<(), EngineError> {
    conn.execute_batch("PRAGMA locking_mode = EXCLUSIVE;")?;
    Ok(())
}

fn apply_key(conn: &Connection, hex_key: &str) -> Result<(), EngineError> {
    conn.execute_batch(&format!(
        "PRAGMA key = \"x'{hex_key}'\"; PRAGMA foreign_keys = ON;"
    ))?;
    Ok(())
}

/// Forces SQLCipher to actually touch the encrypted pages, so a wrong key
/// or a corrupt/tampered file fails here with a specific, clear error
/// (SPEC.md §5) instead of surfacing as a confusing failure on first real
/// query. Also the first statement to request a lock on `conn` since
/// [`set_exclusive_locking`] — if another connection already holds this
/// file's exclusive lock, that surfaces here as `SQLITE_BUSY`, mapped to
/// [`EngineError::CaseAlreadyOpen`] rather than a raw "database is
/// locked".
fn verify_decryption(conn: &Connection, path: &Path) -> Result<(), EngineError> {
    match conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    }) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(err, _)) if err.code == ErrorCode::NotADatabase => {
            Err(EngineError::CaseCorrupt(
                path.to_path_buf(),
                "SQLCipher key was rejected, or the file is corrupt/tampered".to_string(),
            ))
        }
        Err(rusqlite::Error::SqliteFailure(err, _)) if err.code == ErrorCode::DatabaseBusy => {
            Err(EngineError::CaseAlreadyOpen(path.to_path_buf()))
        }
        Err(e) => Err(EngineError::Sqlite(e)),
    }
}

/// SQLCipher's built-in decrypt-and-copy primitive: attach a second
/// database at `dest`, keyed with `key`, and ask SQLCipher to migrate
/// every table into it. `key = ""` means unencrypted (`ExportFormat::Sqlite`);
/// any other value is used as a SQLCipher passphrase (`ExportFormat::Portable`)
/// — unlike the keychain's raw hex key (see `import_impl`), a plain
/// passphrase has no special SQL syntax to preserve, so both it and `dest`
/// are passed as bound parameters rather than interpolated into the SQL
/// text — neither a path nor a passphrase containing a quote can break the
/// statement.
fn export_sqlite(conn: &Connection, dest: &Path, key: &str) -> Result<(), EngineError> {
    let dest_str = dest.to_str().ok_or_else(|| {
        EngineError::CaseCorrupt(
            dest.to_path_buf(),
            "destination path is not valid UTF-8".to_string(),
        )
    })?;

    let result: Result<(), EngineError> = (|| {
        conn.execute(
            "ATTACH DATABASE ?1 AS export_target KEY ?2",
            params![dest_str, key],
        )?;
        conn.query_row("SELECT sqlcipher_export('export_target')", [], |row| {
            row.get::<_, Option<i64>>(0)
        })?;
        conn.execute("DETACH DATABASE export_target", [])?;
        Ok(())
    })();

    if result.is_err() {
        let _ = conn.execute("DETACH DATABASE export_target", []);
        let _ = fs::remove_file(dest);
    }
    result
}

/// A JSON dump of the full entity/relationship graph, each with its
/// attribute facts and audit trail — everything `entity show`/`audit show`
/// can print, gathered case-wide into one file. `include_hidden: true`
/// throughout — a case archive shouldn't silently drop hidden entities
/// (issue #9) the way interactive browsing does by default; hiding is a
/// dismiss-from-view, not a redaction.
fn export_report(case: &Case, dest: &Path) -> Result<(), EngineError> {
    let attrs_json = |attrs: &[AttributeRecord]| -> serde_json::Value {
        attrs
            .iter()
            .map(|a| {
                serde_json::json!({
                    "fact_id": a.fact_id.0.to_string(),
                    "key": a.key,
                    "value": a.value,
                    "source": a.source,
                    "collected_at_unix_ms": a.collected_at_unix_ms,
                    "is_current": a.is_current,
                    "conflicting": a.conflicting,
                })
            })
            .collect()
    };
    let audit_json = |events: &[AuditEvent]| -> serde_json::Value {
        events
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id.to_string(),
                    "event_type": e.event_type,
                    "description": e.description,
                    "actor": e.actor,
                    "occurred_at_unix_ms": e.occurred_at_unix_ms,
                })
            })
            .collect()
    };

    let mut entities_json = Vec::new();
    for entity in crud::list_entities(
        &case.conn,
        EntityFilter {
            include_hidden: true,
            ..Default::default()
        },
    )? {
        let attrs = crud::list_attribute_records(&case.conn, entity.id)?;
        let audit = crud::audit_trail(&case.conn, AuditTarget::Entity(entity.id))?;
        entities_json.push(serde_json::json!({
            "id": entity.id.0.to_string(),
            "entity_type": entity.entity_type.to_string(),
            "canonical_key": entity.canonical_key,
            "display_label": entity.display_label,
            "attributes": attrs_json(&attrs),
            "audit_events": audit_json(&audit),
        }));
    }

    let mut relationships_json = Vec::new();
    for rel in crud::list_relationships(&case.conn, true)? {
        let attrs = crud::list_relationship_attribute_records(&case.conn, rel.id)?;
        let audit = crud::audit_trail(&case.conn, AuditTarget::Relationship(rel.id))?;
        relationships_json.push(serde_json::json!({
            "id": rel.id.0.to_string(),
            "from_entity_id": rel.from.0.to_string(),
            "to_entity_id": rel.to.0.to_string(),
            "relationship_type": rel.relationship_type.to_string(),
            "created_at_unix_ms": rel.created_at_unix_ms,
            "attributes": attrs_json(&attrs),
            "audit_events": audit_json(&audit),
        }));
    }

    let report = serde_json::json!({
        "case_id": case.case_id.to_string(),
        "case_name": case.name,
        "generated_at_unix_ms": crate::now_unix_ms(),
        "entities": entities_json,
        "relationships": relationships_json,
    });
    fs::write(dest, serde_json::to_string_pretty(&report)?)?;
    Ok(())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Renders a stored unix-ms timestamp as a human-readable UTC datestamp
/// (RFC 3339, e.g. `2026-09-11T14:32:07Z`) for the HTML report — every
/// other consumer of `*_unix_ms` (the CLI, the GUI's own DTOs) is a
/// machine-facing value on purpose, but a report is read by a person.
/// Falls back to the raw millisecond value on any conversion error
/// (there isn't a realistic one for a timestamp this project itself
/// generated) rather than failing the whole export over a display nicety.
fn format_unix_ms_utc(unix_ms: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix_ms.div_euclid(1000))
        .ok()
        .and_then(|dt| {
            dt.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| unix_ms.to_string())
}

// A "case dossier" treatment (design review: an Artifact mockup against
// sample data, iterated once — backgrounds flattened to plain white after
// that review, since this is meant to be printed) rather than a bare
// unstyled document. Source Serif 4 (headings) + IBM Plex Sans (body) +
// IBM Plex Mono (ids/timestamps/data) — deliberately not the Inter-on-cream
// combination every generic AI-styled page reaches for. Colors echo the
// GUI's own accent hue (`entityStyle.ts`'s `--accent`) for product
// consistency, recalibrated for a white, printable background rather than
// the GUI's dark theme. `@media print` avoids splitting an entity card or
// table row across a page break.
const REPORT_CSS: &str = r#"
:root {
  --ink: #1c1a29;
  --paper: #ffffff;
  --line: #ded7f2;
  --accent: #5b46c9;
  --muted: #6c6480;
  --good: #2f7d5a;
  --good-soft: #e4f2ea;
  --warn: #a6532c;
  --warn-soft: #f6e9e0;
}
@media (prefers-color-scheme: dark) {
  :root {
    --ink: #eee9fb; --paper: #141220; --line: #322a4d; --accent: #a795f5;
    --muted: #a99fc4; --good: #6fcf9c; --good-soft: #1c2c24;
    --warn: #d98a5f; --warn-soft: #2e2119;
  }
}
* { box-sizing: border-box; }
html { color-scheme: light dark; }
body {
  margin: 0; background: var(--paper); color: var(--ink);
  font-family: "IBM Plex Sans", -apple-system, "Segoe UI", sans-serif;
  font-size: 15px; line-height: 1.6; -webkit-font-smoothing: antialiased;
}
.page { max-width: 880px; margin: 0 auto; padding: 3.5rem 2rem 4rem; }
.masthead { border-top: 3px solid var(--accent); padding-top: 1.4rem; margin-bottom: 2.6rem; }
.eyebrow {
  font-family: "IBM Plex Mono", monospace; font-size: 11px; font-weight: 500;
  letter-spacing: 0.14em; text-transform: uppercase; color: var(--accent); margin: 0 0 0.6rem;
}
h1.title {
  font-family: "Source Serif 4", Georgia, serif; font-weight: 600; font-size: 2.2rem;
  line-height: 1.15; margin: 0 0 0.75rem; text-wrap: balance; letter-spacing: -0.01em;
}
.meta-row {
  display: flex; flex-wrap: wrap; gap: 0.4rem 1.6rem;
  font-family: "IBM Plex Mono", monospace; font-size: 12.5px; color: var(--muted);
}
.meta-row b { color: var(--ink); font-weight: 500; }
.stats {
  display: grid; grid-template-columns: repeat(4, 1fr); gap: 1px;
  background: var(--line); border: 1px solid var(--line); border-radius: 4px;
  overflow: hidden; margin-bottom: 3rem;
}
.stat { background: var(--paper); padding: 1rem 1.1rem; }
.stat .n {
  font-family: "Source Serif 4", Georgia, serif; font-size: 1.9rem; font-weight: 600;
  font-variant-numeric: tabular-nums; line-height: 1; display: block; margin-bottom: 0.3rem;
}
.stat .l { font-size: 11.5px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--muted); }
section.block { margin-bottom: 3.2rem; }
.section-head {
  display: flex; align-items: baseline; gap: 0.7rem;
  border-bottom: 1px solid var(--line); padding-bottom: 0.5rem; margin-bottom: 1.4rem;
}
.section-head .num { font-family: "IBM Plex Mono", monospace; font-size: 13px; color: var(--accent); font-weight: 500; }
.section-head h2 { font-family: "Source Serif 4", Georgia, serif; font-weight: 600; font-size: 1.35rem; margin: 0; }
.section-head .count { margin-left: auto; font-family: "IBM Plex Mono", monospace; font-size: 12px; color: var(--muted); }
.empty { color: var(--muted); font-style: italic; font-size: 13.5px; }
.badge {
  display: inline-flex; align-items: center; justify-content: center; width: 28px; height: 22px;
  border-radius: 3px; font-family: "IBM Plex Mono", monospace; font-size: 10.5px; font-weight: 600;
  letter-spacing: 0.02em; flex: none;
}
.b-person { background: #ece7fa; color: #5b46c9; }
.b-username { background: #e3f2e8; color: #2f7d5a; }
.b-account { background: #eaf5ee; color: #3f8f66; }
.b-email { background: #faf0dc; color: #96721c; }
.b-vehicle { background: #fbf2e0; color: #a67c2c; }
.b-phone { background: #fbe8f0; color: #a6386f; }
.b-net { background: #e2eef9; color: #2a6ca6; }
.b-org { background: #ececf1; color: #57536b; }
.b-location { background: #f3ead9; color: #8a6a2f; }
.b-wallet { background: #fbf1d9; color: #a6791c; }
.b-other { background: #ececf1; color: #57536b; }
@media (prefers-color-scheme: dark) {
  .b-person { background: #2a2246; color: #c3b3fa; }
  .b-username { background: #1c2c24; color: #7fdba8; }
  .b-account { background: #172a20; color: #7fdba8; }
  .b-email { background: #362c17; color: #e0b95c; }
  .b-vehicle { background: #332912; color: #dcaa56; }
  .b-phone { background: #3a2030; color: #ec8fb8; }
  .b-net { background: #1c2c3a; color: #7ab6e8; }
  .b-org { background: #2a283a; color: #b7b3d0; }
  .b-location { background: #332a18; color: #d6b878; }
  .b-wallet { background: #3a2e12; color: #e6c164; }
  .b-other { background: #2a283a; color: #b7b3d0; }
}
.entity {
  border: 1px solid var(--line); border-left: 3px solid var(--accent); border-radius: 3px;
  padding: 1.1rem 1.3rem 1.2rem; margin-bottom: 1rem; background: var(--paper);
  break-inside: avoid;
}
.entity-head { display: flex; align-items: center; gap: 0.7rem; margin-bottom: 0.5rem; }
.entity-head .name { font-family: "Source Serif 4", Georgia, serif; font-weight: 600; font-size: 1.15rem; }
.entity-head .type { font-family: "IBM Plex Mono", monospace; font-size: 11.5px; color: var(--muted); }
.entity-ids { font-family: "IBM Plex Mono", monospace; font-size: 11.5px; color: var(--muted); margin: 0 0 0.9rem; }
table { border-collapse: collapse; width: 100%; font-size: 13px; }
th, td { text-align: left; padding: 0.45rem 0.6rem; border-bottom: 1px solid var(--line); }
th {
  font-size: 10.5px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted);
  font-weight: 500; border-bottom: 1px solid var(--ink);
}
tr:last-child td { border-bottom: none; }
.mono { font-family: "IBM Plex Mono", monospace; }
.num { font-variant-numeric: tabular-nums; }
.pill { display: inline-block; padding: 0.12rem 0.5rem; border-radius: 99px; font-size: 11px; font-weight: 500; }
.pill-current { background: var(--good-soft); color: var(--good); }
.pill-conflict { background: var(--warn-soft); color: var(--warn); }
.pill-superseded { background: var(--paper); color: var(--muted); border: 1px solid var(--line); }
.audit { margin-top: 0.9rem; padding-top: 0.8rem; border-top: 1px dashed var(--line); }
.audit .h { font-size: 10.5px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); margin-bottom: 0.5rem; }
.audit ul { list-style: none; margin: 0; padding: 0; }
.audit li { display: flex; gap: 0.7rem; font-size: 12.5px; padding: 0.25rem 0; }
.audit .when { font-family: "IBM Plex Mono", monospace; color: var(--muted); flex: none; width: 12rem; }
.rel-endpoint { display: flex; align-items: center; gap: 0.5rem; }
.rel-endpoint .txt .name { display: block; font-size: 13.5px; }
.rel-endpoint .txt .id { display: block; font-family: "IBM Plex Mono", monospace; font-size: 10.5px; color: var(--muted); }
.rel-arrow { color: var(--accent); font-size: 15px; text-align: center; }
.rel-type-cell { font-family: "IBM Plex Mono", monospace; font-size: 12px; color: var(--ink); }
footer {
  margin-top: 3.5rem; padding-top: 1.2rem; border-top: 1px solid var(--line);
  display: flex; justify-content: space-between;
  font-family: "IBM Plex Mono", monospace; font-size: 11px; color: var(--muted);
}
@media print {
  body { background: #fff; }
  .page { padding: 0.5in 0.4in; max-width: none; }
  .entity, tr { break-inside: avoid; }
  section.block { break-inside: avoid-page; }
}
"#;

/// `(badge CSS class, two-letter abbreviation)` for an entity type's
/// report badge — mirrors `eumeaus-gui/src/entityStyle.ts`'s
/// `TYPE_STYLES`/`styleForEntityType` (same hue groupings, e.g. Domain/
/// IpAddress/Url all read as "networking" and share one color) so a type
/// reads the same way in this report as it does in the GUI. Kept as its
/// own small mapping here rather than shared code: that file is
/// TypeScript, this is Rust, and the mapping is a handful of match arms,
/// not worth a cross-language shared-data scheme.
fn badge_style(entity_type: &EntityType) -> (&'static str, String) {
    use EntityType::*;
    match entity_type {
        Person => ("b-person", "PE".to_string()),
        Username => ("b-username", "UN".to_string()),
        OnlineAccount => ("b-account", "OA".to_string()),
        Email => ("b-email", "EM".to_string()),
        Vehicle => ("b-vehicle", "VE".to_string()),
        PhoneNumber => ("b-phone", "PH".to_string()),
        Domain => ("b-net", "DO".to_string()),
        IpAddress => ("b-net", "IP".to_string()),
        Url => ("b-net", "UR".to_string()),
        Organization => ("b-org", "OR".to_string()),
        Document => ("b-org", "DC".to_string()),
        Image => ("b-org", "IM".to_string()),
        Location => ("b-location", "LO".to_string()),
        CryptoWallet => ("b-wallet", "CW".to_string()),
        Custom(name) => (
            "b-other",
            name.chars().take(2).collect::<String>().to_uppercase(),
        ),
    }
}

/// Same directional/non-directional split as `eumeaus-gui/src/
/// entityStyle.ts`'s `isDirectionalRelationship` (the Graph screen's
/// arrowheads) — kept in sync by hand for the same reason `badge_style`
/// is: a handful of match arms, not worth sharing across languages. Purely
/// a display choice (a single arrow vs. a double-headed one in the
/// Relationships table); the underlying `from`/`to` data is unchanged
/// either way.
fn is_directional_relationship(rel_type: &RelationshipType) -> bool {
    matches!(
        rel_type,
        RelationshipType::HasAccount
            | RelationshipType::Owns
            | RelationshipType::LocatedAt
            | RelationshipType::MemberOf
            | RelationshipType::ResolvesTo
            | RelationshipType::Mentions
    )
}

/// One resolved relationship endpoint's report presentation: its badge
/// class/abbreviation, a human-readable name, and its raw id (kept
/// visible, de-emphasized, for provenance tracing — see
/// [`export_html`]'s `entity_displays` map).
#[derive(Clone)]
struct EndpointDisplay {
    badge_class: &'static str,
    abbr: String,
    name: String,
    id: EntityId,
}

/// Looks `id` up in `map` (built from the case's own entity list); falls
/// back to a plain "unknown type" badge showing just the raw id if it's
/// somehow missing (shouldn't happen — a merge re-points relationship
/// endpoints at the survivor rather than leaving one dangling — but a
/// report should never fail to generate over a display nicety).
fn resolve_endpoint(
    map: &std::collections::HashMap<EntityId, EndpointDisplay>,
    id: EntityId,
) -> EndpointDisplay {
    map.get(&id).cloned().unwrap_or_else(|| EndpointDisplay {
        badge_class: "b-other",
        abbr: "??".to_string(),
        name: id.to_string(),
        id,
    })
}

fn render_endpoint(e: &EndpointDisplay) -> String {
    format!(
        "<div class=\"rel-endpoint\"><span class=\"badge {}\">{}</span><span class=\"txt\"><span class=\"name\">{}</span><span class=\"id\">{}</span></span></div>",
        e.badge_class,
        html_escape(&e.abbr),
        html_escape(&e.name),
        e.id
    )
}

/// Same underlying data as [`export_report`], rendered as a self-contained
/// HTML document instead of JSON — human-readable, openable in any
/// browser, print-to-PDF-able from there (SPEC.md §8 open question 6),
/// with no external CSS/JS/images so it stays a single portable file. All
/// entity/plugin-supplied text is HTML-escaped.
fn export_html(case: &Case, dest: &Path) -> Result<(), EngineError> {
    let mut html = String::new();
    html.push_str("<!doctype html>\n<html><head><meta charset=\"utf-8\">\n");
    html.push_str(&format!(
        "<title>{} — Eumeaus case report</title>\n",
        html_escape(&case.name)
    ));
    html.push_str(
        "<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n\
         <link href=\"https://fonts.googleapis.com/css2?family=Source+Serif+4:wght@500;600;700&family=IBM+Plex+Sans:wght@400;500;600&family=IBM+Plex+Mono:wght@400;500&display=swap\" rel=\"stylesheet\">\n",
    );
    html.push_str(&format!("<style>{REPORT_CSS}</style>\n"));
    html.push_str("</head><body>\n<div class=\"page\">\n");

    let generated = format_unix_ms_utc(crate::now_unix_ms());
    html.push_str("<div class=\"masthead\">\n");
    html.push_str("<p class=\"eyebrow\">Eumeaus · Case Report</p>\n");
    html.push_str(&format!(
        "<h1 class=\"title\">{}</h1>\n",
        html_escape(&case.name)
    ));
    html.push_str(&format!(
        "<div class=\"meta-row\"><span>Case ID <b class=\"mono\">{}</b></span><span>Generated <b>{generated}</b></span></div>\n",
        case.case_id
    ));
    html.push_str("</div>\n");

    let stats = crud::case_stats(&case.conn)?;
    html.push_str("<div class=\"stats\">\n");
    for (n, label) in [
        (stats.entity_count, "Entities"),
        (stats.relationship_count, "Relationships"),
        (stats.fact_count, "Facts recorded"),
        (stats.conflicting_entity_count, "Entities in conflict"),
    ] {
        html.push_str(&format!(
            "<div class=\"stat\"><span class=\"n\">{n}</span><span class=\"l\">{label}</span></div>\n"
        ));
    }
    html.push_str("</div>\n");

    let entities = crud::list_entities(
        &case.conn,
        EntityFilter {
            include_hidden: true,
            ..Default::default()
        },
    )?;
    // Built before the loop below consumes `entities` — the Relationships
    // section (after it) needs to resolve each endpoint's id to something
    // an investigator can actually read, not a bare UUID, complete with
    // the same type badge its own entity card uses.
    let entity_displays: std::collections::HashMap<EntityId, EndpointDisplay> = entities
        .iter()
        .map(|e| {
            let (badge_class, abbr) = badge_style(&e.entity_type);
            (
                e.id,
                EndpointDisplay {
                    badge_class,
                    abbr,
                    name: e.display_label.clone(),
                    id: e.id,
                },
            )
        })
        .collect();

    html.push_str("<section class=\"block\">\n");
    html.push_str(&format!(
        "<div class=\"section-head\"><span class=\"num\">01</span><h2>Entities</h2><span class=\"count\">{} total</span></div>\n",
        entities.len()
    ));
    if entities.is_empty() {
        html.push_str("<p class=\"empty\">None.</p>\n");
    }
    for entity in entities {
        let attrs = crud::list_attribute_records(&case.conn, entity.id)?;
        let audit = crud::audit_trail(&case.conn, AuditTarget::Entity(entity.id))?;
        let (badge_class, abbr) = badge_style(&entity.entity_type);

        html.push_str("<div class=\"entity\">\n");
        html.push_str(&format!(
            "<div class=\"entity-head\"><span class=\"badge {badge_class}\">{}</span><span class=\"name\">{}</span><span class=\"type\">{}</span></div>\n",
            html_escape(&abbr),
            html_escape(&entity.display_label),
            html_escape(&entity.entity_type.to_string())
        ));
        html.push_str(&format!(
            "<p class=\"entity-ids\">ID <span class=\"mono\">{}</span> · Canonical key <span class=\"mono\">{}</span></p>\n",
            entity.id,
            html_escape(entity.canonical_key.as_deref().unwrap_or("-"))
        ));

        if attrs.is_empty() {
            html.push_str("<p class=\"empty\">No attributes.</p>\n");
        } else {
            html.push_str(
                "<table><tr><th>Key</th><th>Value</th><th>Source</th><th>Collected</th><th>Status</th><th>Fact ID</th></tr>\n",
            );
            for a in &attrs {
                let status_pill = match (a.is_current, a.conflicting) {
                    (true, true) => "<span class=\"pill pill-conflict\">conflict</span>",
                    (true, false) => "<span class=\"pill pill-current\">current</span>",
                    (false, _) => "<span class=\"pill pill-superseded\">superseded</span>",
                };
                html.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td class=\"mono\">{}</td><td class=\"mono num\">{}</td><td>{status_pill}</td><td class=\"mono\">{}</td></tr>\n",
                    html_escape(&a.key),
                    html_escape(&a.value),
                    html_escape(&a.source),
                    format_unix_ms_utc(a.collected_at_unix_ms),
                    a.fact_id,
                ));
            }
            html.push_str("</table>\n");
        }

        if !audit.is_empty() {
            html.push_str("<div class=\"audit\">\n<div class=\"h\">Audit trail</div>\n<ul>\n");
            for e in &audit {
                html.push_str(&format!(
                    "<li><span class=\"when mono\">{}</span><span>{} by {}: {}</span></li>\n",
                    format_unix_ms_utc(e.occurred_at_unix_ms),
                    html_escape(&e.event_type),
                    html_escape(&e.actor),
                    html_escape(&e.description)
                ));
            }
            html.push_str("</ul>\n</div>\n");
        }
        html.push_str("</div>\n");
    }
    html.push_str("</section>\n");

    let relationships = crud::list_relationships(&case.conn, true)?;
    html.push_str("<section class=\"block\">\n");
    html.push_str(&format!(
        "<div class=\"section-head\"><span class=\"num\">02</span><h2>Relationships</h2><span class=\"count\">{} total</span></div>\n",
        relationships.len()
    ));
    if relationships.is_empty() {
        html.push_str("<p class=\"empty\">None.</p>\n");
    } else {
        html.push_str("<table><tr><th>From</th><th></th><th>Relationship</th><th>To</th><th>Created</th></tr>\n");
        for r in &relationships {
            let from = resolve_endpoint(&entity_displays, r.from);
            let to = resolve_endpoint(&entity_displays, r.to);
            let arrow = if is_directional_relationship(&r.relationship_type) {
                "→"
            } else {
                "↔"
            };
            html.push_str(&format!(
                "<tr><td>{}</td><td class=\"rel-arrow\">{arrow}</td><td class=\"rel-type-cell\">{}</td><td>{}</td><td class=\"mono num\">{}</td></tr>\n",
                render_endpoint(&from),
                html_escape(&r.relationship_type.to_string()),
                render_endpoint(&to),
                format_unix_ms_utc(r.created_at_unix_ms)
            ));
        }
        html.push_str("</table>\n");
    }
    html.push_str("</section>\n");

    html.push_str(&format!(
        "<footer><span>Case ID <span class=\"mono\">{}</span></span><span>Generated {generated} · Eumeaus</span></footer>\n",
        case.case_id
    ));

    html.push_str("</div>\n</body></html>\n");
    fs::write(dest, html)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keeps the developer's real OS keychain clean across test runs; each
    // test uses a fresh random case_id so this never collides between
    // tests.
    fn cleanup(case: &Case) {
        let _ = keystore::delete_key(case.id());
    }

    #[test]
    fn create_then_open_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let created = Case::create(dir.path(), "roundtrip").unwrap();
        let case_id = created.id();
        assert_eq!(created.name(), "roundtrip");
        created.close().unwrap();

        let opened = Case::open(&dir.path().join("roundtrip.eum")).unwrap();
        assert_eq!(opened.id(), case_id);
        assert_eq!(opened.name(), "roundtrip");
        cleanup(&opened);
    }

    #[test]
    fn an_uploaded_image_survives_close_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut created = Case::create(dir.path(), "image-roundtrip").unwrap();

        let entity_id = created
            .add_entity(EntityType::Person, None, vec![], test_provenance())
            .unwrap();
        created
            .add_image_to_entity(
                entity_id,
                "image/jpeg".to_string(),
                vec![10, 20, 30, 40],
                test_provenance(),
            )
            .unwrap();
        let case_path = created.path().to_path_buf();
        created.close().unwrap();

        let reopened = Case::open(&case_path).unwrap();
        let images = reopened.list_entity_images(entity_id).unwrap();
        assert_eq!(images.len(), 1);
        let data = reopened.get_entity_image(images[0].id).unwrap();
        assert_eq!(data.mime_type, "image/jpeg");
        assert_eq!(data.data, vec![10, 20, 30, 40]);
        cleanup(&reopened);
    }

    /// Reproduces exactly what a real case file created under v0.1.0-
    /// v0.1.4 looks like: `init_case_file`'s own logic, minus
    /// `SCHEMA_ADDITIONS_SQL` — those releases' `schema.sql` never
    /// contained `entity_images`, so applying `SCHEMA_SQL` alone is the
    /// real old shape, not a guess at it.
    fn init_case_file_without_new_tables(
        case_path: &Path,
        case_id: Uuid,
        name: &str,
        hex_key: &str,
    ) -> Result<Case, EngineError> {
        let mut conn = Connection::open(case_path)?;
        apply_key(&conn, hex_key)?;
        set_exclusive_locking(&conn)?;

        let now = crate::now_unix_ms();
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA_SQL)?;
        {
            let mut insert_meta =
                tx.prepare("INSERT INTO case_meta (key, value) VALUES (?1, ?2)")?;
            insert_meta.execute(params!["case_id", case_id.to_string()])?;
            insert_meta.execute(params!["name", name])?;
            insert_meta.execute(params!["schema_version", "1"])?;
            insert_meta.execute(params!["created_at", now.to_string()])?;
        }
        tx.commit()?;

        fs::write(meta_path_for(case_path), case_id.to_string())?;

        Ok(Case {
            path: case_path.to_path_buf(),
            case_id,
            name: name.to_string(),
            conn,
        })
    }

    #[test]
    fn opening_a_pre_entity_images_case_backfills_the_table() {
        let dir = tempfile::tempdir().unwrap();
        let case_path = dir.path().join("old.eum");
        let case_id = Uuid::new_v4();
        let hex_key = keystore::create_key(case_id).unwrap();

        let old_case =
            init_case_file_without_new_tables(&case_path, case_id, "old", &hex_key).unwrap();
        // Confirm this test is honestly reproducing the old shape, not
        // accidentally passing because the table was there all along.
        let table_count: i64 = old_case
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'entity_images'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            table_count, 0,
            "old_case must not already have entity_images"
        );
        old_case.close().unwrap();

        // The real production code path a user reopening an old case takes.
        let mut reopened = Case::open(&case_path).unwrap();

        let entity_id = reopened
            .add_entity(EntityType::Person, None, vec![], test_provenance())
            .unwrap();
        reopened
            .add_image_to_entity(
                entity_id,
                "image/png".to_string(),
                vec![1, 2, 3],
                test_provenance(),
            )
            .unwrap();
        let images = reopened.list_entity_images(entity_id).unwrap();
        assert_eq!(
            images.len(),
            1,
            "entity_images must exist and be usable after reopening an old case"
        );

        cleanup(&reopened);
    }

    #[test]
    fn create_refuses_to_overwrite_existing_case() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "dup").unwrap();
        cleanup(&case);

        let err = Case::create(dir.path(), "dup").unwrap_err();
        assert!(matches!(err, EngineError::CaseAlreadyExists(_)));
    }

    #[test]
    fn open_missing_case_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = Case::open(&dir.path().join("nope.eum")).unwrap_err();
        assert!(matches!(err, EngineError::CaseNotFound(_)));
    }

    #[test]
    fn open_fails_fast_when_already_open() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "locked").unwrap();

        let err = Case::open(case.path()).unwrap_err();
        assert!(matches!(err, EngineError::CaseAlreadyOpen(_)));

        cleanup(&case);
    }

    #[test]
    fn list_finds_case_files_without_opening_them() {
        let dir = tempfile::tempdir().unwrap();
        let a = Case::create(dir.path(), "alpha").unwrap();
        let b = Case::create(dir.path(), "beta").unwrap();
        let (a_id, b_id) = (a.id(), b.id());
        a.close().unwrap();
        b.close().unwrap();

        let summaries = Case::list(dir.path()).unwrap();
        assert_eq!(summaries.len(), 2, "both .eum files should be listed");
        assert_eq!(summaries[0].name, "alpha");
        assert_eq!(summaries[0].id, a_id);
        assert_eq!(summaries[1].name, "beta");
        assert_eq!(summaries[1].id, b_id);

        keystore::delete_key(a_id).ok();
        keystore::delete_key(b_id).ok();
    }

    #[test]
    fn list_on_a_directory_with_no_cases_is_empty_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Case::list(dir.path()).unwrap().is_empty());
        assert!(Case::list(&dir.path().join("does-not-exist"))
            .unwrap()
            .is_empty());
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

    #[test]
    fn export_sqlite_produces_a_plaintext_readable_copy() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "export-sqlite").unwrap();
        case.add_entity(
            EntityType::Username,
            Some("carol".to_string()),
            vec![],
            test_provenance(),
        )
        .unwrap();

        let dest = dir.path().join("export.sqlite");
        case.export(&dest, ExportFormat::Sqlite).unwrap();

        // No PRAGMA key applied — this is what a plain sqlite3 open of the
        // exported file looks like, and it must succeed (unlike the same
        // check against the real case file — see
        // plain_sqlite_open_without_key_cannot_read_schema).
        let plain = Connection::open(&dest).unwrap();
        let count: i64 = plain
            .query_row("SELECT count(*) FROM entities", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        cleanup(&case);
    }

    #[test]
    fn export_refuses_to_overwrite_an_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "export-no-overwrite").unwrap();
        let dest = dir.path().join("already-there");
        fs::write(&dest, b"pre-existing content").unwrap();

        let err = case.export(&dest, ExportFormat::Sqlite).unwrap_err();
        assert!(matches!(err, EngineError::ExportDestinationExists(_)));

        cleanup(&case);
    }

    #[test]
    fn export_report_writes_json_with_entities_and_relationships() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "export-report").unwrap();
        let a = case
            .add_entity(EntityType::Person, None, vec![], test_provenance())
            .unwrap();
        let b = case
            .add_entity(EntityType::Organization, None, vec![], test_provenance())
            .unwrap();
        case.add_relationship(a, b, RelationshipType::MemberOf, vec![], test_provenance())
            .unwrap();

        let dest = dir.path().join("report.json");
        case.export(&dest, ExportFormat::Report).unwrap();

        let text = fs::read_to_string(&dest).unwrap();
        let report: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(report["entities"].as_array().unwrap().len(), 2);
        assert_eq!(report["relationships"].as_array().unwrap().len(), 1);

        cleanup(&case);
    }

    #[test]
    fn export_html_writes_a_self_contained_document_with_escaped_content() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "export-html").unwrap();
        case.add_entity(
            EntityType::Person,
            Some("<script>alert(1)</script>".to_string()),
            vec![Attribute {
                key: "note".to_string(),
                value: "value with <b>tags</b> & \"quotes\"".to_string(),
            }],
            test_provenance(),
        )
        .unwrap();

        let dest = dir.path().join("report.html");
        case.export(&dest, ExportFormat::Html).unwrap();

        let html = fs::read_to_string(&dest).unwrap();
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<h1 class=\"title\">export-html</h1>"));
        assert!(
            !html.contains("<script>alert(1)</script>"),
            "entity-supplied content must be HTML-escaped, not injected raw:\n{html}"
        );
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("value with &lt;b&gt;tags&lt;/b&gt; &amp; &quot;quotes&quot;"));

        cleanup(&case);
    }

    #[test]
    fn export_html_renders_timestamps_as_utc_datestamps_not_epoch_millis() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "export-html-timestamps").unwrap();
        // test_provenance()'s collected_at_unix_ms is a fixed 1000 (1s
        // past the epoch) — a deterministic, easy-to-assert-on value. An
        // attribute is required for that timestamp to actually render
        // (an attribute-less entity's "Collected" column never appears).
        case.add_entity(
            EntityType::Person,
            None,
            vec![Attribute {
                key: "note".to_string(),
                value: "x".to_string(),
            }],
            test_provenance(),
        )
        .unwrap();

        let dest = dir.path().join("report.html");
        case.export(&dest, ExportFormat::Html).unwrap();
        let html = fs::read_to_string(&dest).unwrap();

        assert!(
            html.contains("1970-01-01T00:00:01Z"),
            "collected_at_unix_ms=1000 should render as an RFC3339 UTC datestamp:\n{html}"
        );
        assert!(
            !html.contains(">1000<"),
            "the raw epoch-ms value should not appear as its own table cell:\n{html}"
        );

        cleanup(&case);
    }

    #[test]
    fn export_html_resolves_relationship_endpoints_to_entity_names_not_raw_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "export-html-relationships").unwrap();
        let alice = case
            .add_entity(
                EntityType::Person,
                Some("alice".to_string()),
                vec![],
                test_provenance(),
            )
            .unwrap();
        let bob = case
            .add_entity(
                EntityType::Person,
                Some("bob".to_string()),
                vec![],
                test_provenance(),
            )
            .unwrap();
        case.add_relationship(
            alice,
            bob,
            RelationshipType::AssociatedWith,
            vec![],
            test_provenance(),
        )
        .unwrap();

        let dest = dir.path().join("report.html");
        case.export(&dest, ExportFormat::Html).unwrap();
        let html = fs::read_to_string(&dest).unwrap();

        assert!(
            html.contains("class=\"name\">alice<") && html.contains("class=\"name\">bob<"),
            "relationship endpoints should show a readable entity name, not just an id:\n{html}"
        );
        assert!(
            html.contains("badge b-person"),
            "relationship endpoints should carry the same type badge as the entity card:\n{html}"
        );
        // The raw ids stay present (de-emphasized) for provenance tracing.
        assert!(html.contains(&alice.to_string()));
        assert!(html.contains(&bob.to_string()));

        cleanup(&case);
    }

    #[test]
    fn export_html_summary_strip_matches_case_stats() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "export-html-stats").unwrap();
        let a = case
            .add_entity(
                EntityType::Person,
                Some("dana".to_string()),
                vec![Attribute {
                    key: "role".to_string(),
                    value: "analyst".to_string(),
                }],
                test_provenance(),
            )
            .unwrap();
        let b = case
            .add_entity(EntityType::Organization, None, vec![], test_provenance())
            .unwrap();
        case.add_relationship(a, b, RelationshipType::MemberOf, vec![], test_provenance())
            .unwrap();

        let dest = dir.path().join("report.html");
        case.export(&dest, ExportFormat::Html).unwrap();
        let html = fs::read_to_string(&dest).unwrap();

        let stats = crud::case_stats(&case.conn).unwrap();
        assert_eq!(stats.entity_count, 2);
        assert_eq!(stats.relationship_count, 1);
        assert!(html.contains(&format!(
            "<span class=\"n\">{}</span><span class=\"l\">Entities</span>",
            stats.entity_count
        )));
        assert!(html.contains(&format!(
            "<span class=\"n\">{}</span><span class=\"l\">Relationships</span>",
            stats.relationship_count
        )));
        assert!(html.contains(&format!(
            "<span class=\"n\">{}</span><span class=\"l\">Facts recorded</span>",
            stats.fact_count
        )));

        cleanup(&case);
    }

    #[test]
    fn export_html_shows_a_conflict_pill_for_a_disputed_attribute() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "export-html-conflict-pill").unwrap();
        case.add_entity(
            EntityType::Username,
            Some("erin".to_string()),
            vec![Attribute {
                key: "color".to_string(),
                value: "blue".to_string(),
            }],
            test_provenance(),
        )
        .unwrap();
        let mut later = test_provenance();
        later.collected_at_unix_ms = 2000;
        case.add_entity(
            EntityType::Username,
            Some("erin".to_string()),
            vec![Attribute {
                key: "color".to_string(),
                value: "red".to_string(),
            }],
            later,
        )
        .unwrap();

        let dest = dir.path().join("report.html");
        case.export(&dest, ExportFormat::Html).unwrap();
        let html = fs::read_to_string(&dest).unwrap();

        assert!(
            html.contains("pill pill-conflict\">conflict"),
            "a key with two disagreeing values should render a conflict pill:\n{html}"
        );

        cleanup(&case);
    }

    #[test]
    fn export_html_badges_an_entity_by_its_type() {
        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "export-html-badge").unwrap();
        case.add_entity(EntityType::CryptoWallet, None, vec![], test_provenance())
            .unwrap();

        let dest = dir.path().join("report.html");
        case.export(&dest, ExportFormat::Html).unwrap();
        let html = fs::read_to_string(&dest).unwrap();

        assert!(
            html.contains("badge b-wallet\">CW"),
            "a CryptoWallet entity should get the wallet badge with a CW abbreviation:\n{html}"
        );

        cleanup(&case);
    }

    #[test]
    fn export_portable_refuses_an_empty_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "portable-empty-passphrase").unwrap();
        let dest = dir.path().join("export.eumx");

        let err = case
            .export(&dest, ExportFormat::Portable(String::new()))
            .unwrap_err();
        assert!(matches!(err, EngineError::EmptyPassphrase));
        assert!(!dest.exists(), "no partial file should be left behind");

        cleanup(&case);
    }

    #[test]
    fn export_portable_produces_a_sqlcipher_file_unreadable_without_the_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "portable-unreadable").unwrap();
        let dest = dir.path().join("export.eumx");

        case.export(
            &dest,
            ExportFormat::Portable("correct horse battery staple".to_string()),
        )
        .unwrap();

        // Same shape as plain_sqlite_open_without_key_cannot_read_schema:
        // no key applied at all.
        let unkeyed = Connection::open(&dest).unwrap();
        assert!(
            unkeyed
                .query_row("SELECT count(*) FROM sqlite_master", [], |row| row
                    .get::<_, i64>(0))
                .is_err(),
            "a portable export must not be readable without its passphrase"
        );

        // Wrong passphrase must fail the same way a wrong raw key does.
        let wrong_key = Connection::open(&dest).unwrap();
        wrong_key
            .execute_batch("PRAGMA key = 'not the passphrase'")
            .unwrap();
        assert!(
            wrong_key
                .query_row("SELECT count(*) FROM sqlite_master", [], |row| row
                    .get::<_, i64>(0))
                .is_err(),
            "a wrong passphrase must not decrypt a portable export"
        );

        // The right passphrase does work.
        let right_key = Connection::open(&dest).unwrap();
        right_key
            .execute_batch("PRAGMA key = 'correct horse battery staple'")
            .unwrap();
        right_key
            .query_row("SELECT count(*) FROM sqlite_master", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("the correct passphrase must decrypt a portable export");

        cleanup(&case);
    }

    #[test]
    fn import_round_trips_a_portable_export_into_a_normal_independent_case() {
        let dir = tempfile::tempdir().unwrap();
        let mut original = Case::create(&dir.path().join("original"), "orig").unwrap();
        original
            .add_entity(
                EntityType::Username,
                Some("carol".to_string()),
                vec![],
                test_provenance(),
            )
            .unwrap();
        let original_id = original.id();

        let portable = dir.path().join("handoff.eumx");
        original
            .export(&portable, ExportFormat::Portable("swordfish".to_string()))
            .unwrap();

        let imported_dir = dir.path().join("imported");
        let mut imported =
            Case::import(&portable, "swordfish", &imported_dir, "imported-case").unwrap();

        // A fresh identity, not the original's.
        assert_ne!(imported.id(), original_id);
        assert_eq!(imported.name(), "imported-case");

        // The data made the trip.
        let entities = imported
            .list_entities(EntityFilter {
                entity_type: Some(EntityType::Username),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].canonical_key.as_deref(), Some("carol"));

        // The imported case is fully independent: normal CRUD, its own
        // keychain entry, closable and reopenable like any other case.
        imported
            .add_entity(EntityType::Person, None, vec![], test_provenance())
            .unwrap();
        let imported_path = imported.path().to_path_buf();
        let imported_id = imported.id();
        imported.close().unwrap();
        let reopened = Case::open(&imported_path).unwrap();
        assert_eq!(reopened.id(), imported_id);

        keystore::delete_key(imported_id).ok();
        cleanup(&original);
    }

    #[test]
    fn import_rejects_the_wrong_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "import-wrong-passphrase").unwrap();
        let portable = dir.path().join("handoff.eumx");
        case.export(&portable, ExportFormat::Portable("right-one".to_string()))
            .unwrap();

        let err = Case::import(
            &portable,
            "wrong-one",
            &dir.path().join("imported"),
            "imported",
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::CaseCorrupt(_, _)));
        assert!(
            !dir.path().join("imported").join("imported.eum").exists(),
            "a failed import must not leave a partial case file behind"
        );

        cleanup(&case);
    }

    #[test]
    fn import_refuses_an_empty_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let err = Case::import(
            &dir.path().join("does-not-matter.eumx"),
            "",
            &dir.path().join("imported"),
            "imported",
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::EmptyPassphrase));
    }

    #[test]
    fn import_refuses_to_overwrite_an_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "import-no-overwrite").unwrap();
        let portable = dir.path().join("handoff.eumx");
        case.export(&portable, ExportFormat::Portable("pw".to_string()))
            .unwrap();

        let existing = Case::create(&dir.path().join("imported"), "already-here").unwrap();
        let existing_id = existing.id();
        existing.close().unwrap();

        let err = Case::import(
            &portable,
            "pw",
            &dir.path().join("imported"),
            "already-here",
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::CaseAlreadyExists(_)));

        keystore::delete_key(existing_id).ok();
        cleanup(&case);
    }

    #[test]
    fn tampered_case_file_is_detected_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "tampered").unwrap();
        let path = case.path().to_path_buf();
        let case_id = case.id();
        case.close().unwrap();

        // Flip the first page's bytes: SQLCipher's per-page HMAC must
        // reject this rather than silently returning garbage rows.
        let mut bytes = fs::read(&path).unwrap();
        for byte in bytes.iter_mut().take(64) {
            *byte ^= 0xFF;
        }
        fs::write(&path, bytes).unwrap();

        let err = Case::open(&path).unwrap_err();
        assert!(matches!(err, EngineError::CaseCorrupt(_, _)));

        let _ = keystore::delete_key(case_id);
    }

    #[test]
    fn plain_sqlite_open_without_key_cannot_read_schema() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "encrypted").unwrap();
        let path = case.path().to_path_buf();
        cleanup(&case);
        case.close().unwrap();

        // No `PRAGMA key` applied here — this is what a plain `sqlite3`
        // open of the case file looks like.
        let conn = Connection::open(&path).unwrap();
        let result = conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        });
        assert!(
            result.is_err(),
            "case file must be unreadable without the key"
        );
    }
}
