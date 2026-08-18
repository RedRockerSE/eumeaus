//! Wire contract between `eumeaus-engine` and plugin subprocesses.
//!
//! The canonical contract is `plugin.proto` at the crate root. Codegen
//! (tonic-build/prost-build) is wired up in milestone M3; until then this
//! module hand-mirrors the message shapes so downstream crates have
//! something concrete to compile against.

pub mod stub {
    use std::collections::HashMap;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ConfidenceStatus {
        Found,
        NotFound,
        Uncertain,
        Error,
    }

    #[derive(Debug, Clone)]
    pub struct Provenance {
        pub source_url: String,
        pub retrieval_method: String,
        pub raw_response_sha256: String,
        pub collected_at_unix_ms: i64,
        pub plugin_name: String,
        pub plugin_version: String,
    }

    #[derive(Debug, Clone)]
    pub struct EntityFinding {
        pub entity_type: String,
        pub canonical_key: String,
        pub display_label: String,
        pub attributes: HashMap<String, String>,
    }

    #[derive(Debug, Clone)]
    pub struct RelationshipFinding {
        pub from_canonical_key: String,
        pub to_canonical_key: String,
        pub relationship_type: String,
    }

    #[derive(Debug, Clone)]
    pub struct CheckResult {
        pub status: ConfidenceStatus,
        pub entities: Vec<EntityFinding>,
        pub relationships: Vec<RelationshipFinding>,
        pub provenance: Option<Provenance>,
        pub error_message: Option<String>,
    }
}
