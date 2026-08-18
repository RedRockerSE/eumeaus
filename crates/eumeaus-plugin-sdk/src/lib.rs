//! `eumeaus-plugin-sdk` — first-party ergonomic helper for plugin authors:
//! implements the boilerplate side of the plugin protocol (handshake, gRPC
//! server bootstrap) so a plugin author writes only the collection logic.
//!
//! Plugins are not required to use this SDK or be written in Rust — the
//! protocol is language-agnostic (SPEC.md §2.4) — but this is the
//! first-party ergonomic path.

use eumeaus_plugin_protocol::plugin_runtime_server::{
    PluginRuntime as ProtoPluginRuntime, PluginRuntimeServer,
};
use eumeaus_plugin_protocol::{CheckRequest, CheckResult, DescribeRequest, DescribeResponse};

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error(
        "EUMEAUS_PLUGIN_DIR is not set — plugins are expected to be spawned by \
         eumeaus-plugin-host, not run directly"
    )]
    MissingPluginDir,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("transport error: {0}")]
    Transport(#[from] tonic::transport::Error),
}

/// Implemented by a plugin's collection logic; [`serve`] handles everything
/// else (handshake, gRPC server, streaming the `Vec` back one item at a
/// time). `check` is async, not sync — a real plugin's collection logic is
/// almost always I/O (an HTTP request per site, for example), and the SDK
/// runs it inside the same tokio runtime the gRPC server uses. A sync
/// `check()` calling a *blocking* HTTP client would panic (reqwest's
/// blocking client refuses to run nested inside an existing runtime); a
/// plugin that's genuinely CPU-only can just `.await` nothing and return
/// immediately.
#[async_trait::async_trait]
pub trait PluginRuntime: Send + Sync + 'static {
    /// `(plugin_name, plugin_version)`.
    fn describe(&self) -> (String, String);
    async fn check(&self, request: &CheckRequest) -> Vec<CheckResult>;
}

struct Adapter<R>(R);

#[async_trait::async_trait]
impl<R: PluginRuntime> ProtoPluginRuntime for Adapter<R> {
    async fn describe(
        &self,
        _request: tonic::Request<DescribeRequest>,
    ) -> Result<tonic::Response<DescribeResponse>, tonic::Status> {
        let (plugin_name, plugin_version) = self.0.describe();
        Ok(tonic::Response::new(DescribeResponse {
            plugin_name,
            plugin_version,
        }))
    }

    type CheckStream = tokio_stream::Iter<std::vec::IntoIter<Result<CheckResult, tonic::Status>>>;

    async fn check(
        &self,
        request: tonic::Request<CheckRequest>,
    ) -> Result<tonic::Response<Self::CheckStream>, tonic::Status> {
        let results: Vec<Result<CheckResult, tonic::Status>> = self
            .0
            .check(request.get_ref())
            .await
            .into_iter()
            .map(Ok)
            .collect();
        Ok(tonic::Response::new(tokio_stream::iter(results)))
    }
}

/// Starts the plugin's gRPC server on a Unix domain socket inside the
/// directory the host provided via `EUMEAUS_PLUGIN_DIR`, then writes the
/// go-plugin-style handshake line to stdout (SPEC.md §2.2):
/// `EUMEAUS-PLUGIN|1|unix|<socket-path>|grpc`. Runs until the process is
/// killed — this is meant to be the plugin binary's entire `main`.
pub async fn serve<R: PluginRuntime>(runtime: R) -> Result<(), SdkError> {
    let plugin_dir = std::env::var("EUMEAUS_PLUGIN_DIR").map_err(|_| SdkError::MissingPluginDir)?;
    let socket_path = std::path::Path::new(&plugin_dir).join("plugin.sock");
    let _ = std::fs::remove_file(&socket_path);

    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let incoming = tokio_stream::wrappers::UnixListenerStream::new(listener);

    println!("EUMEAUS-PLUGIN|1|unix|{}|grpc", socket_path.display());
    {
        use std::io::Write;
        std::io::stdout().flush()?;
    }

    tonic::transport::Server::builder()
        .add_service(PluginRuntimeServer::new(Adapter(runtime)))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}
