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

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, ErrorCode};
use uuid::Uuid;

use crate::{
    crud, keystore, Actor, Attribute, AttributeRecord, AuditEvent, AuditTarget, CaseSummary,
    EngineError, Entity, EntityFilter, EntityId, EntityType, FactId, PluginRef, Provenance,
    RelationshipId, RelationshipType, ScanConfig, ScanId, ScanStatus, ScanSummary, TargetEntity,
};

const SCHEMA_SQL: &str = include_str!("schema.sql");
const SCHEMA_VERSION: &str = "1";

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

/// Opaque handle over an open, decrypted case DB connection + exclusive
/// file lock. Dropping it (or calling [`Case::close`]) releases both.
pub struct Case {
    path: PathBuf,
    case_id: Uuid,
    name: String,
    conn: Connection,
    _lock: File,
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
        let lock = lock_exclusive(case_path)?;

        let mut conn = Connection::open(case_path)?;
        apply_key(&conn, hex_key)?;

        let now = crate::now_unix_ms();
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA_SQL)?;
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
            _lock: lock,
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

        let lock = lock_exclusive(path)?;

        let case_id = read_case_id(path)?;
        let hex_key = keystore::load_key(case_id)?;

        let conn = Connection::open(path)?;
        apply_key(&conn, &hex_key)?;
        verify_decryption(&conn, path)?;
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
            _lock: lock,
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
        let lock = lock_exclusive(dest_path)?;
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
            _lock: lock,
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
        actor: Actor,
    ) -> Result<EntityId, EngineError> {
        crud::split_entity(&mut self.conn, id, fact_ids, actor)
    }

    pub fn redact_fact(
        &mut self,
        fact_id: FactId,
        actor: Actor,
        reason: &str,
    ) -> Result<(), EngineError> {
        crud::redact_fact(&mut self.conn, fact_id, actor, reason)
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

fn lock_exclusive(path: &Path) -> Result<File, EngineError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock().map_err(|err| match err {
        std::fs::TryLockError::WouldBlock => EngineError::CaseAlreadyOpen(path.to_path_buf()),
        std::fs::TryLockError::Error(io_err) => EngineError::Io(io_err),
    })?;
    Ok(file)
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
/// query.
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
/// can print, gathered case-wide into one file.
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
    for entity in crud::list_entities(&case.conn, EntityFilter::default())? {
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
    for rel in crud::list_relationships(&case.conn)? {
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

const REPORT_CSS: &str = "\
body { font-family: sans-serif; max-width: 900px; margin: 2rem auto; padding: 0 1rem; color: #1a1a1a; }\
h1, h2 { border-bottom: 1px solid #ccc; padding-bottom: 0.3rem; }\
.entity { border: 1px solid #ddd; border-radius: 6px; padding: 0.75rem 1rem; margin: 1rem 0; }\
.entity .type { color: #666; font-weight: normal; }\
table { border-collapse: collapse; width: 100%; margin: 0.5rem 0; }\
th, td { border: 1px solid #ddd; padding: 0.3rem 0.5rem; text-align: left; font-size: 0.9rem; }\
th { background: #f5f5f5; }\
code { background: #f5f5f5; padding: 0.1rem 0.3rem; border-radius: 3px; }\
";

/// Same underlying data as [`export_report`], rendered as a self-contained
/// HTML document instead of JSON — human-readable, openable in any
/// browser, print-to-PDF-able from there (SPEC.md §8 open question 6),
/// with no external CSS/JS/images so it stays a single portable file. All
/// entity/plugin-supplied text is HTML-escaped.
fn export_html(case: &Case, dest: &Path) -> Result<(), EngineError> {
    let mut html = String::new();
    html.push_str("<!doctype html>\n<html><head><meta charset=\"utf-8\">\n");
    html.push_str(&format!(
        "<title>Eumeaus case report — {}</title>\n<style>{REPORT_CSS}</style>\n",
        html_escape(&case.name)
    ));
    html.push_str("</head><body>\n");
    html.push_str(&format!("<h1>Case: {}</h1>\n", html_escape(&case.name)));
    html.push_str(&format!(
        "<p>Case ID: <code>{}</code><br>Generated: {}</p>\n",
        case.case_id,
        crate::now_unix_ms()
    ));

    html.push_str("<h2>Entities</h2>\n");
    let entities = crud::list_entities(&case.conn, EntityFilter::default())?;
    if entities.is_empty() {
        html.push_str("<p><em>None.</em></p>\n");
    }
    for entity in entities {
        let attrs = crud::list_attribute_records(&case.conn, entity.id)?;
        let audit = crud::audit_trail(&case.conn, AuditTarget::Entity(entity.id))?;
        html.push_str("<div class=\"entity\">\n");
        html.push_str(&format!(
            "<h3>{} <span class=\"type\">({})</span></h3>\n",
            html_escape(&entity.display_label),
            html_escape(&entity.entity_type.to_string())
        ));
        html.push_str(&format!(
            "<p>ID: <code>{}</code><br>Canonical key: <code>{}</code></p>\n",
            entity.id,
            html_escape(entity.canonical_key.as_deref().unwrap_or("-"))
        ));
        if attrs.is_empty() {
            html.push_str("<p><em>No attributes.</em></p>\n");
        } else {
            html.push_str(
                "<table><tr><th>Key</th><th>Value</th><th>Source</th><th>Collected</th><th>Current</th><th>Fact ID</th></tr>\n",
            );
            for a in &attrs {
                let current = match (a.is_current, a.conflicting) {
                    (true, true) => "yes (conflict)",
                    (true, false) => "yes",
                    (false, _) => "no",
                };
                html.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{current}</td><td><code>{}</code></td></tr>\n",
                    html_escape(&a.key),
                    html_escape(&a.value),
                    html_escape(&a.source),
                    a.collected_at_unix_ms,
                    a.fact_id,
                ));
            }
            html.push_str("</table>\n");
        }
        if !audit.is_empty() {
            html.push_str("<h4>Audit events</h4>\n<ul>\n");
            for e in &audit {
                html.push_str(&format!(
                    "<li>{} — {} by {}: {}</li>\n",
                    e.occurred_at_unix_ms,
                    html_escape(&e.event_type),
                    html_escape(&e.actor),
                    html_escape(&e.description)
                ));
            }
            html.push_str("</ul>\n");
        }
        html.push_str("</div>\n");
    }

    html.push_str("<h2>Relationships</h2>\n");
    let relationships = crud::list_relationships(&case.conn)?;
    if relationships.is_empty() {
        html.push_str("<p><em>None.</em></p>\n");
    } else {
        html.push_str("<table><tr><th>From</th><th>Type</th><th>To</th><th>Created</th></tr>\n");
        for r in &relationships {
            html.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{}</td><td><code>{}</code></td><td>{}</td></tr>\n",
                r.from,
                html_escape(&r.relationship_type.to_string()),
                r.to,
                r.created_at_unix_ms
            ));
        }
        html.push_str("</table>\n");
    }

    html.push_str("</body></html>\n");
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
        assert!(html.contains("<h1>Case: export-html</h1>"));
        assert!(
            !html.contains("<script>alert(1)</script>"),
            "entity-supplied content must be HTML-escaped, not injected raw:\n{html}"
        );
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("value with &lt;b&gt;tags&lt;/b&gt; &amp; &quot;quotes&quot;"));

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
