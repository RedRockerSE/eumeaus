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
    crud, keystore, Actor, Attribute, AttributeRecord, AuditEvent, AuditTarget, EngineError,
    Entity, EntityFilter, EntityId, EntityType, FactId, PluginRef, Provenance, RelationshipId,
    RelationshipType, ScanConfig, ScanId, ScanStatus, TargetEntity,
};

const SCHEMA_SQL: &str = include_str!("schema.sql");
const SCHEMA_VERSION: &str = "1";

pub enum ExportFormat {
    Sqlite,
    Report,
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

    pub fn export(&self, _dest: &Path, _format: ExportFormat) -> Result<(), EngineError> {
        Err(EngineError::NotImplemented("Case::export"))
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

    pub fn resume_scan(&mut self, scan_id: ScanId) -> Result<(), EngineError> {
        crate::scan::resume(&mut self.conn, scan_id.0)
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
