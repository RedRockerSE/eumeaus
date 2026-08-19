//! `eumeaus-plugin-host` — plugin discovery, manifest validation, signature
//! verification, and subprocess lifecycle management: spawn, gRPC
//! handshake, health/timeout monitoring, teardown (SPEC.md §2.2).
//!
//! Modeled on HashiCorp's `go-plugin`: the engine spawns a plugin binary,
//! the plugin starts a local gRPC server on a Unix domain socket and writes
//! the connection info to stdout as a handshake line; the host then
//! connects as a gRPC client. See [`host`] for the handshake format.
//!
//! No Windows named-pipe transport yet (SPEC.md §2.2 mentions one) — this
//! machine has no Windows target to develop or test that against, so it's
//! left as a documented gap rather than guessed at.

/// `credential set/list/remove` (SPEC.md §3.4) plus [`credentials::resolve_credentials`],
/// which a scan uses to inject a plugin's requested credentials into its
/// `CheckRequest` — see the module doc for why credentials never touch
/// the case file, subprocess argv, or environment variables.
pub mod credentials;
mod host;
mod manifest;
mod signature;
/// `trust add/list/remove` (SPEC.md §8 open question 2): a local, plain-file
/// store of named Ed25519 public keys the investigator has explicitly
/// chosen to trust — not the OS keychain `credentials` uses, since these
/// aren't secrets. See the module doc for why v1 has no baked-in
/// "official" key.
pub mod trust_store;

pub use eumeaus_plugin_protocol::{
    CheckRequest, CheckResult, ConfidenceStatus, EntityFinding, Provenance, RateLimitConfig,
    RelationshipFinding,
};
pub use host::{PluginHandle, PluginHost};
pub use manifest::{
    check_compatibility, discover, load_file, CompatibilitySection, ContractSection,
    ExecutionSection, PermissionsSection, PluginManifest, PluginSection,
};
pub use signature::{sign, verify, TrustPolicy};

use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("not yet implemented: {0}")]
    NotImplemented(&'static str),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid manifest at {0}: {1}")]
    InvalidManifest(PathBuf, String),
    #[error("plugin {0} is unsigned; refusing to load (pass --allow-unsigned for local dev)")]
    Unsigned(String),
    #[error("plugin {0}'s signature does not verify")]
    InvalidSignature(String),
    #[error("signature error: {0}")]
    Signature(String),
    #[error("plugin {0} is incompatible with this host: {1}")]
    Incompatible(String, String),
    #[error("plugin {0} handshake timed out after {1:?}")]
    HandshakeTimeout(String, Duration),
    #[error("plugin {0} sent an invalid handshake line: {1:?}")]
    InvalidHandshake(String, String),
    #[error("plugin {0} exited before completing the handshake")]
    ProcessExited(String),
    #[error("plugin {0} timed out after {1:?}")]
    Timeout(String, Duration),
    #[error("transport error: {0}")]
    Transport(#[from] tonic::transport::Error),
    #[error("grpc error: {0}")]
    Grpc(#[from] Box<tonic::Status>),
    #[error("credential store error: {0}")]
    Credential(String),
    #[error(
        "plugin {1} requires credential {0:?}, which is not set (use `eumeaus credential set {0}`)"
    )]
    MissingCredential(String, String),
    #[error("trust store error: {0}")]
    TrustStore(String),
}
