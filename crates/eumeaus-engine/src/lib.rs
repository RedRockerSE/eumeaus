//! `eumeaus-engine` — case lifecycle, the entity/relationship/provenance
//! data model, entity resolution, and scan orchestration.
//!
//! Case lifecycle (create/open/close) is implemented over a
//! SQLCipher-encrypted SQLite file (M1); entity/relationship CRUD, merge/
//! split, and scan orchestration are still stubs returning
//! [`EngineError::NotImplemented`] (M2+).

mod case;
mod keystore;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use case::{Case, ExportFormat};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("not yet implemented: {0}")]
    NotImplemented(&'static str),
    #[error("case already open: {0}")]
    CaseAlreadyOpen(PathBuf),
    #[error("case not found: {0}")]
    CaseNotFound(PathBuf),
    #[error("case already exists: {0}")]
    CaseAlreadyExists(PathBuf),
    #[error("case file {0} is corrupt or tampered: {1}")]
    CaseCorrupt(PathBuf, String),
    #[error("OS keychain error: {0}")]
    Keychain(String),
    #[error("no encryption key found in the OS keychain for case {0} (created on a different machine or keychain entry removed?)")]
    KeyNotFound(Uuid),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
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
