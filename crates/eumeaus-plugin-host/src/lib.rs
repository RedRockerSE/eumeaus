//! `eumeaus-plugin-host` — plugin discovery, manifest validation, signature
//! verification, and subprocess lifecycle management (spawn, gRPC handshake,
//! health/timeout monitoring, teardown).
//!
//! STUB CRATE (milestone M0). Real subprocess/gRPC wiring lands in M3.

use std::path::{Path, PathBuf};

use eumeaus_plugin_protocol::stub::CheckResult;

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("not yet implemented: {0}")]
    NotImplemented(&'static str),
}

pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub entrypoint: PathBuf,
}

pub struct PluginHandle {
    pub name: String,
}

pub enum TrustPolicy {
    RequireSignature,
    AllowUnsigned,
}

pub struct CheckRequest {
    pub scan_id: String,
    pub input_entity_type: String,
    pub input_value: String,
}

pub type CheckResultStream = Vec<CheckResult>;

/// Manages plugin subprocess lifecycles.
pub struct PluginHost;

impl PluginHost {
    pub fn discover(_plugins_dir: &Path) -> Result<Vec<PluginManifest>, PluginError> {
        Err(PluginError::NotImplemented("PluginHost::discover"))
    }

    pub fn load(
        &mut self,
        _manifest: &PluginManifest,
        _trust_policy: TrustPolicy,
    ) -> Result<PluginHandle, PluginError> {
        Err(PluginError::NotImplemented("PluginHost::load"))
    }

    pub fn invoke(
        &self,
        _handle: &PluginHandle,
        _request: CheckRequest,
    ) -> Result<CheckResultStream, PluginError> {
        Err(PluginError::NotImplemented("PluginHost::invoke"))
    }

    pub fn shutdown(&mut self, _handle: PluginHandle) -> Result<(), PluginError> {
        Err(PluginError::NotImplemented("PluginHost::shutdown"))
    }
}
