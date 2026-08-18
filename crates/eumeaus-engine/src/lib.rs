//! `eumeaus-engine` — case lifecycle, the entity/relationship/provenance
//! data model, entity resolution, and scan orchestration.
//!
//! STUB CRATE (milestone M0). Every `Case` method returns
//! [`EngineError::NotImplemented`]; real behavior lands in M1 (case
//! lifecycle/persistence) and M2 (entity/relationship CRUD).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("not yet implemented: {0}")]
    NotImplemented(&'static str),
    #[error("case already open: {0}")]
    CaseAlreadyOpen(PathBuf),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type CaseError = EngineError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelationshipId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FactId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScanId(pub Uuid);

/// Starter taxonomy per SPEC.md §4.3 (open question: confirm before real
/// implementation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityType {
    Person,
    Username,
    Email,
    PhoneNumber,
    Domain,
    IpAddress,
    OnlineAccount,
    Organization,
    Location,
    Document,
    Image,
    Vehicle,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationshipType {
    HasAccount,
    Owns,
    AssociatedWith,
    LocatedAt,
    MemberOf,
    ResolvesTo,
    Mentions,
    RelatedTo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attribute {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub source: String,
    pub source_url: Option<String>,
    pub retrieval_method: Option<String>,
    pub raw_response_sha256: Option<String>,
    pub collected_at_unix_ms: i64,
}

#[derive(Debug, Clone)]
pub struct Actor {
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct EntityFilter {
    pub entity_type: Option<EntityType>,
}

#[derive(Debug, Clone)]
pub struct Entity {
    pub id: EntityId,
    pub entity_type: EntityType,
    pub canonical_key: Option<String>,
    pub display_label: String,
}

pub enum AuditTarget {
    Entity(EntityId),
    Relationship(RelationshipId),
    Scan(ScanId),
}

#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub id: Uuid,
    pub description: String,
    pub actor: String,
    pub occurred_at_unix_ms: i64,
}

pub enum ExportFormat {
    Sqlite,
    Report,
}

pub struct PluginRef {
    pub name: String,
}

pub struct TargetEntity {
    pub id: EntityId,
}

#[derive(Default)]
pub struct ScanConfig {
    pub worker_pool: Option<u32>,
    pub rate_limit_per_sec: Option<u32>,
    pub proxy: Option<String>,
}

pub enum ScanStatus {
    Pending,
    Running,
    Completed,
    PartiallyFailed,
    Aborted,
}

/// Opaque handle over an open, decrypted case DB connection + file lock.
///
/// STUB: does not yet open a real SQLCipher-encrypted database or acquire a
/// file lock. See SPEC.md §4.1 and milestone M1.
pub struct Case {
    path: PathBuf,
}

impl Case {
    pub fn create(_path: &Path, _name: &str) -> Result<Case, CaseError> {
        Err(EngineError::NotImplemented("Case::create"))
    }

    pub fn open(_path: &Path) -> Result<Case, CaseError> {
        Err(EngineError::NotImplemented("Case::open"))
    }

    pub fn close(self) -> Result<(), CaseError> {
        Err(EngineError::NotImplemented("Case::close"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn export(&self, _dest: &Path, _format: ExportFormat) -> Result<(), CaseError> {
        Err(EngineError::NotImplemented("Case::export"))
    }

    pub fn add_entity(
        &mut self,
        _entity_type: EntityType,
        _key: Option<String>,
        _attrs: Vec<Attribute>,
        _provenance: Provenance,
    ) -> Result<EntityId, EngineError> {
        Err(EngineError::NotImplemented("Case::add_entity"))
    }

    pub fn merge_entities(
        &mut self,
        _a: EntityId,
        _b: EntityId,
        _actor: Actor,
    ) -> Result<EntityId, EngineError> {
        Err(EngineError::NotImplemented("Case::merge_entities"))
    }

    pub fn split_entity(
        &mut self,
        _id: EntityId,
        _fact_ids: Vec<FactId>,
        _actor: Actor,
    ) -> Result<EntityId, EngineError> {
        Err(EngineError::NotImplemented("Case::split_entity"))
    }

    pub fn add_relationship(
        &mut self,
        _from: EntityId,
        _to: EntityId,
        _rel_type: RelationshipType,
        _attrs: Vec<Attribute>,
        _provenance: Provenance,
    ) -> Result<RelationshipId, EngineError> {
        Err(EngineError::NotImplemented("Case::add_relationship"))
    }

    pub fn list_entities(&self, _filter: EntityFilter) -> Result<Vec<Entity>, EngineError> {
        Err(EngineError::NotImplemented("Case::list_entities"))
    }

    pub fn audit_trail(&self, _target: AuditTarget) -> Result<Vec<AuditEvent>, EngineError> {
        Err(EngineError::NotImplemented("Case::audit_trail"))
    }

    pub fn start_scan(
        &mut self,
        _plugin: PluginRef,
        _target: TargetEntity,
        _config: ScanConfig,
    ) -> Result<ScanId, EngineError> {
        Err(EngineError::NotImplemented("Case::start_scan"))
    }

    pub fn resume_scan(&mut self, _scan_id: ScanId) -> Result<(), EngineError> {
        Err(EngineError::NotImplemented("Case::resume_scan"))
    }

    pub fn scan_status(&self, _scan_id: ScanId) -> Result<ScanStatus, EngineError> {
        Err(EngineError::NotImplemented("Case::scan_status"))
    }
}
