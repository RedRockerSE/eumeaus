//! `eumeaus-engine` — case lifecycle, the entity/relationship/provenance
//! data model, entity resolution, and scan orchestration.
//!
//! Case lifecycle (M1) and entity/relationship CRUD + merge/split (M2) are
//! implemented over a SQLCipher-encrypted SQLite file. Scan orchestration
//! is still stubs returning [`EngineError::NotImplemented`] (M3+).

mod case;
mod crud;
mod keystore;
pub mod plugins;
mod scan;
pub mod trust;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use case::{Case, ExportFormat};
pub use eumeaus_plugin_host::credentials;
pub use eumeaus_plugin_host::TrustPolicy;

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
    #[error("entity not found: {0}")]
    EntityNotFound(EntityId),
    #[error("relationship not found: {0}")]
    RelationshipNotFound(RelationshipId),
    #[error("fact {0} not found on entity {1}")]
    FactNotFound(FactId, EntityId),
    #[error("cannot merge an entity with itself: {0}")]
    CannotMergeSelf(EntityId),
    #[error("scan not found: {0}")]
    ScanNotFound(ScanId),
    #[error("no discovered plugin in {0} is compatible with entity type {1}")]
    NoCompatiblePlugins(PathBuf, String),
    #[error("plugin {0} was requested but is not a discovered, compatible plugin")]
    UnknownPlugin(String),
    #[error("plugin {0:?} is already installed (remove its directory under --plugins-dir first)")]
    PluginAlreadyInstalled(String),
    #[error("stored scan config is invalid: {0}")]
    InvalidScanConfig(String),
    #[error("plugin host error: {0}")]
    PluginHost(#[from] eumeaus_plugin_host::PluginError),
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("export destination already exists: {0}")]
    ExportDestinationExists(PathBuf),
    #[error("a passphrase is required for a portable export/import (SPEC.md §8 open question 1)")]
    EmptyPassphrase,
}

pub type CaseError = EngineError;

pub(crate) fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as i64
}

macro_rules! uuid_id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub Uuid);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

uuid_id_type!(EntityId);
uuid_id_type!(RelationshipId);
uuid_id_type!(FactId);
uuid_id_type!(ScanId);

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
    /// Added to the starter taxonomy (SPEC.md §4.3/§8 open question 3, now
    /// resolved) alongside `Url` — common OSINT investigation targets that
    /// don't map cleanly onto any existing type (a wallet isn't an
    /// `OnlineAccount`; a bare URL of interest isn't necessarily a profile
    /// page).
    CryptoWallet,
    Url,
    Custom(String),
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Person => write!(f, "Person"),
            Self::Username => write!(f, "Username"),
            Self::Email => write!(f, "Email"),
            Self::PhoneNumber => write!(f, "PhoneNumber"),
            Self::Domain => write!(f, "Domain"),
            Self::IpAddress => write!(f, "IPAddress"),
            Self::OnlineAccount => write!(f, "OnlineAccount"),
            Self::Organization => write!(f, "Organization"),
            Self::Location => write!(f, "Location"),
            Self::Document => write!(f, "Document"),
            Self::Image => write!(f, "Image"),
            Self::Vehicle => write!(f, "Vehicle"),
            Self::CryptoWallet => write!(f, "CryptoWallet"),
            Self::Url => write!(f, "Url"),
            Self::Custom(s) => write!(f, "{s}"),
        }
    }
}

impl std::str::FromStr for EntityType {
    type Err = std::convert::Infallible;

    /// Never fails: an unrecognized string is [`EntityType::Custom`], the
    /// taxonomy's escape hatch (SPEC.md §4.3).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "Person" => Self::Person,
            "Username" => Self::Username,
            "Email" => Self::Email,
            "PhoneNumber" => Self::PhoneNumber,
            "Domain" => Self::Domain,
            "IPAddress" => Self::IpAddress,
            "OnlineAccount" => Self::OnlineAccount,
            "Organization" => Self::Organization,
            "Location" => Self::Location,
            "Document" => Self::Document,
            "Image" => Self::Image,
            "Vehicle" => Self::Vehicle,
            "CryptoWallet" => Self::CryptoWallet,
            "Url" => Self::Url,
            other => Self::Custom(other.to_string()),
        })
    }
}

/// SPEC.md §4.3 lists no `Custom` escape hatch for relationships (only the
/// generic `RelatedTo`), but silently coercing an unrecognized string to
/// `RelatedTo` would lose what the caller actually said into an
/// append-only fact — so this carries the same escape hatch as
/// [`EntityType`].
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
    Custom(String),
}

impl std::fmt::Display for RelationshipType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HasAccount => write!(f, "HasAccount"),
            Self::Owns => write!(f, "Owns"),
            Self::AssociatedWith => write!(f, "AssociatedWith"),
            Self::LocatedAt => write!(f, "LocatedAt"),
            Self::MemberOf => write!(f, "MemberOf"),
            Self::ResolvesTo => write!(f, "ResolvesTo"),
            Self::Mentions => write!(f, "Mentions"),
            Self::RelatedTo => write!(f, "RelatedTo"),
            Self::Custom(s) => write!(f, "{s}"),
        }
    }
}

impl std::str::FromStr for RelationshipType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "HasAccount" => Self::HasAccount,
            "Owns" => Self::Owns,
            "AssociatedWith" => Self::AssociatedWith,
            "LocatedAt" => Self::LocatedAt,
            "MemberOf" => Self::MemberOf,
            "ResolvesTo" => Self::ResolvesTo,
            "Mentions" => Self::Mentions,
            "RelatedTo" => Self::RelatedTo,
            other => Self::Custom(other.to_string()),
        })
    }
}

/// Confidence of a single collected finding (SPEC.md §3.1/§3.2). Manually
/// entered facts are always [`Self::Found`] — a human directly asserting
/// data isn't "checking" anything.
///
/// SPEC.md §3.1 illustrates `Error(String)`; the `facts` table (§4.2) has
/// nowhere to persist that message, so this drops the payload until M4
/// wires up scan ingestion and (if needed) a schema column for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfidenceStatus {
    Found,
    NotFound,
    Uncertain,
    Error,
}

impl std::fmt::Display for ConfidenceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Found => write!(f, "FOUND"),
            Self::NotFound => write!(f, "NOT_FOUND"),
            Self::Uncertain => write!(f, "UNCERTAIN"),
            Self::Error => write!(f, "ERROR"),
        }
    }
}

impl ConfidenceStatus {
    /// Maps `eumeaus_plugin_protocol::ConfidenceStatus`'s raw `i32` (as
    /// stored on `CheckResult.status`) without a direct type dependency —
    /// engine only needs the four discriminant values, not the generated
    /// protocol enum itself. Values 0 (unspecified) and anything
    /// unrecognized fold into `Error`, matching "a plugin that won't say
    /// what it found is indistinguishable from one that failed."
    pub(crate) fn from_protocol_i32(value: i32) -> Self {
        match value {
            1 => Self::Found,
            2 => Self::NotFound,
            3 => Self::Uncertain,
            _ => Self::Error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attribute {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    /// Plugin name, or `"user"` for manual entry (SPEC.md §4.2 `facts.source`).
    pub source: String,
    /// Plugin semver, or app version for manual entry (`facts.source_version`).
    pub source_version: String,
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
    pub event_type: String,
    pub description: String,
    pub actor: String,
    pub occurred_at_unix_ms: i64,
}

/// One key/value fact on an entity, with enough provenance to judge it.
/// SPEC.md §4.4: the "current" value per key is the most recent by
/// `collected_at`; `conflicting` flags when other facts disagree on the
/// same key so a viewer never mistakes "current" for "only".
#[derive(Debug, Clone)]
pub struct AttributeRecord {
    /// The fact this value came from — `entity split --facts` (SPEC.md
    /// §3.4) needs these ids and has no other way to learn them.
    pub fact_id: FactId,
    pub key: String,
    pub value: String,
    pub source: String,
    pub collected_at_unix_ms: i64,
    pub is_current: bool,
    pub conflicting: bool,
}

#[derive(Debug, Clone)]
pub struct Relationship {
    pub id: RelationshipId,
    pub from: EntityId,
    pub to: EntityId,
    pub relationship_type: RelationshipType,
    pub created_at_unix_ms: i64,
}

/// One `.eum` file found by [`Case::list`] — deliberately shallow (no
/// decryption, no keychain lookup): the name comes straight from the
/// filename (`Case::create` always writes `<name>.eum`) and the id from
/// the plaintext `.eum.meta` sidecar, so listing cases in a directory never
/// requires opening any of them.
#[derive(Debug, Clone)]
pub struct CaseSummary {
    pub path: PathBuf,
    pub name: String,
    pub id: Uuid,
}

#[derive(Debug, Clone)]
pub struct ScanSummary {
    pub id: ScanId,
    pub target_entity_id: EntityId,
    pub status: ScanStatus,
    pub started_at_unix_ms: Option<i64>,
    pub completed_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct PluginRef {
    pub name: String,
}

#[derive(Debug, Clone, Copy)]
pub struct TargetEntity {
    pub id: EntityId,
}

#[derive(Debug, Clone, Default)]
pub struct ScanConfig {
    pub worker_pool: Option<u32>,
    pub rate_limit_per_sec: Option<u32>,
    pub proxy: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStatus {
    Pending,
    Running,
    Completed,
    PartiallyFailed,
    Aborted,
}

impl std::fmt::Display for ScanStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "PENDING"),
            Self::Running => write!(f, "RUNNING"),
            Self::Completed => write!(f, "COMPLETED"),
            Self::PartiallyFailed => write!(f, "PARTIALLY_FAILED"),
            Self::Aborted => write!(f, "ABORTED"),
        }
    }
}

impl std::str::FromStr for ScanStatus {
    type Err = EngineError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "PENDING" => Self::Pending,
            "RUNNING" => Self::Running,
            "COMPLETED" => Self::Completed,
            "PARTIALLY_FAILED" => Self::PartiallyFailed,
            "ABORTED" => Self::Aborted,
            other => {
                return Err(EngineError::InvalidScanConfig(format!(
                    "unrecognized scan status {other:?}"
                )))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn entity_type_round_trips_the_full_taxonomy_through_display_and_from_str() {
        let named = [
            EntityType::Person,
            EntityType::Username,
            EntityType::Email,
            EntityType::PhoneNumber,
            EntityType::Domain,
            EntityType::IpAddress,
            EntityType::OnlineAccount,
            EntityType::Organization,
            EntityType::Location,
            EntityType::Document,
            EntityType::Image,
            EntityType::Vehicle,
            EntityType::CryptoWallet,
            EntityType::Url,
        ];
        for entity_type in named {
            let text = entity_type.to_string();
            assert_eq!(
                EntityType::from_str(&text).unwrap(),
                entity_type,
                "{text} did not round-trip"
            );
        }
    }

    #[test]
    fn entity_type_from_str_never_fails_an_unrecognized_string_falls_back_to_custom() {
        assert_eq!(
            EntityType::from_str("SocialMediaPost").unwrap(),
            EntityType::Custom("SocialMediaPost".to_string())
        );
    }
}
