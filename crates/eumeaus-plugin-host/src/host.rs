//! Subprocess lifecycle: spawn a plugin binary, perform the go-plugin-style
//! handshake over its stdout, connect a gRPC client over the Unix domain
//! socket it reports, and enforce per-invocation timeouts.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use hyper_util::rt::TokioIo;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use eumeaus_plugin_protocol::plugin_runtime_client::PluginRuntimeClient;
use eumeaus_plugin_protocol::{CheckRequest, CheckResult};

use crate::manifest::{self, PluginManifest};
use crate::signature;
use crate::{PluginError, TrustPolicy};

/// Handshake magic string and core protocol version, written by the
/// plugin's first stdout line as `MAGIC|CORE_VERSION|NETWORK|ADDRESS|WIRE`
/// (SPEC.md §2.2). `NETWORK` is always `unix` and `WIRE` always `grpc` for
/// now — see the module doc for why there's no Windows named-pipe support
/// yet.
const HANDSHAKE_MAGIC: &str = "EUMEAUS-PLUGIN";
const HANDSHAKE_CORE_VERSION: &str = "1";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// A running plugin subprocess plus its gRPC client. Dropping this kills
/// the process (`kill_on_drop`) even if [`PluginHost::shutdown`] is never
/// called — a hung plugin can never outlive the handle that names it.
pub struct PluginHandle {
    name: String,
    child: Child,
    client: PluginRuntimeClient<Channel>,
    default_timeout: Duration,
    _work_dir: tempfile::TempDir,
}

impl std::fmt::Debug for PluginHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginHandle")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Manages plugin subprocess lifecycles.
#[derive(Debug, Default)]
pub struct PluginHost;

impl PluginHost {
    pub fn discover(plugins_dir: &Path) -> Result<Vec<PluginManifest>, PluginError> {
        manifest::discover(plugins_dir)
    }

    /// Validates the manifest (engine/protocol compatibility, then
    /// signature per `trust_policy`), spawns the plugin binary, and blocks
    /// until its handshake arrives or [`HANDSHAKE_TIMEOUT`] elapses.
    pub async fn load(
        &mut self,
        manifest: &PluginManifest,
        trust_policy: TrustPolicy,
    ) -> Result<PluginHandle, PluginError> {
        manifest::check_compatibility(manifest)?;
        signature::verify(manifest, &trust_policy)?;

        let work_dir = tempfile::Builder::new()
            .prefix("eumeaus-plugin-")
            .tempdir()?;

        let mut child = Command::new(manifest.entrypoint_path())
            .env("EUMEAUS_PLUGIN_DIR", work_dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;

        let stdout = child.stdout.take().expect("stdout was piped at spawn");
        let mut lines = BufReader::new(stdout).lines();

        let handshake_line = match timeout(HANDSHAKE_TIMEOUT, lines.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => {
                let _ = child.kill().await;
                return Err(PluginError::ProcessExited(manifest.plugin.name.clone()));
            }
            Ok(Err(e)) => {
                let _ = child.kill().await;
                return Err(PluginError::Io(e));
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err(PluginError::HandshakeTimeout(
                    manifest.plugin.name.clone(),
                    HANDSHAKE_TIMEOUT,
                ));
            }
        };

        let socket_path = match parse_handshake(&handshake_line) {
            Some(path) => path,
            None => {
                let _ = child.kill().await;
                return Err(PluginError::InvalidHandshake(
                    manifest.plugin.name.clone(),
                    handshake_line,
                ));
            }
        };

        let client = match connect_uds(&socket_path).await {
            Ok(channel) => PluginRuntimeClient::new(channel),
            Err(e) => {
                let _ = child.kill().await;
                return Err(PluginError::InvalidHandshake(
                    manifest.plugin.name.clone(),
                    e.to_string(),
                ));
            }
        };

        Ok(PluginHandle {
            name: manifest.plugin.name.clone(),
            child,
            client,
            default_timeout: Duration::from_millis(manifest.execution.default_timeout_ms),
            _work_dir: work_dir,
        })
    }

    /// Invokes `Check`, draining its result stream, bounded by the
    /// request's `rate_limit.timeout_ms` if set and nonzero, else the
    /// manifest's `default_timeout_ms`. On timeout the plugin process is
    /// left running — [`PluginHandle`]'s `kill_on_drop` and
    /// [`PluginHost::shutdown`] own teardown, so a caller retrying or
    /// inspecting the handle after a timeout still can.
    pub async fn invoke(
        &self,
        handle: &PluginHandle,
        request: CheckRequest,
    ) -> Result<Vec<CheckResult>, PluginError> {
        let effective_timeout = request
            .rate_limit
            .as_ref()
            .map(|r| r.timeout_ms)
            .filter(|&ms| ms > 0)
            .map(|ms| Duration::from_millis(ms as u64))
            .unwrap_or(handle.default_timeout);

        let mut client = handle.client.clone();
        let call = async move {
            let mut stream = client.check(request).await.map_err(Box::new)?.into_inner();
            let mut results = Vec::new();
            while let Some(item) = stream.message().await.map_err(Box::new)? {
                results.push(item);
            }
            Ok::<_, PluginError>(results)
        };

        match timeout(effective_timeout, call).await {
            Ok(result) => result,
            Err(_) => Err(PluginError::Timeout(handle.name.clone(), effective_timeout)),
        }
    }

    pub async fn shutdown(&mut self, mut handle: PluginHandle) -> Result<(), PluginError> {
        let _ = handle.child.kill().await;
        Ok(())
    }
}

fn parse_handshake(line: &str) -> Option<PathBuf> {
    let mut parts = line.splitn(5, '|');
    let magic = parts.next()?;
    let core_version = parts.next()?;
    let network = parts.next()?;
    let address = parts.next()?;
    let wire = parts.next()?;

    if magic != HANDSHAKE_MAGIC
        || core_version != HANDSHAKE_CORE_VERSION
        || network != "unix"
        || wire != "grpc"
    {
        return None;
    }
    Some(PathBuf::from(address))
}

async fn connect_uds(socket_path: &Path) -> Result<Channel, tonic::transport::Error> {
    let socket_path = socket_path.to_path_buf();
    // The URI is a required but ignored placeholder: our connector always
    // dials `socket_path` regardless of what tonic passes it.
    Endpoint::try_from("http://[::]:0")
        .expect("static placeholder URI is always valid")
        .connect_with_connector(service_fn(move |_: Uri| {
            let socket_path = socket_path.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(socket_path).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await
}
