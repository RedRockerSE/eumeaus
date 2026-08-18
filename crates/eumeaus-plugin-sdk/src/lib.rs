//! `eumeaus-plugin-sdk` — first-party ergonomic helper for plugin authors:
//! implements the boilerplate side of the plugin protocol (handshake, gRPC
//! server bootstrap, manifest embedding) so a plugin author writes only the
//! collection logic.
//!
//! STUB CRATE (milestone M0). Real handshake/server bootstrap lands in M3,
//! consumed first by the username-search PoC plugin in M5.

use eumeaus_plugin_protocol::stub::CheckResult;

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("not yet implemented: {0}")]
    NotImplemented(&'static str),
}

/// Implemented by a plugin's collection logic; wired to the gRPC server
/// bootstrap by [`serve`].
pub trait PluginRuntime {
    fn describe(&self) -> (String, String);
    fn check(&self, input_value: &str) -> Vec<CheckResult>;
}

/// Starts the plugin's gRPC server on a local socket and performs the
/// handshake with the engine (writes connection info to stdout).
pub fn serve<R: PluginRuntime>(_runtime: R) -> Result<(), SdkError> {
    Err(SdkError::NotImplemented("serve"))
}
