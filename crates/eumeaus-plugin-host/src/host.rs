//! Subprocess lifecycle: spawn a plugin binary, perform the go-plugin-style
//! handshake over its stdout, connect a gRPC client over the Unix domain
//! socket it reports, and enforce per-invocation timeouts.
//!
//! Two env vars are set on every spawned plugin: `EUMEAUS_PLUGIN_DIR` (a
//! fresh per-invocation scratch dir, currently used for the handshake
//! socket) and `EUMEAUS_PLUGIN_MANIFEST_DIR` (the stable directory holding
//! the plugin's own `plugin.toml`, for finding sibling config/data files —
//! e.g. eumeaus-username-search-plugin's `sites.toml`). Neither is part of
//! the gRPC wire contract (`plugin.proto`); both are host-provided
//! filesystem conveniences a plugin may ignore entirely.

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
/// (SPEC.md §2.2). `WIRE` is always `grpc`; `NETWORK` is `unix` on Unix or
/// `namedpipe` on Windows — [`eumeaus_plugin_sdk::serve`] on the plugin
/// side picks the one matching the platform it's actually running on, and
/// [`EXPECTED_NETWORK`] here does the same, so a handshake line claiming
/// the wrong platform's transport is rejected as invalid rather than
/// silently mismatched.
const HANDSHAKE_MAGIC: &str = "EUMEAUS-PLUGIN";
const HANDSHAKE_CORE_VERSION: &str = "1";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(unix)]
const EXPECTED_NETWORK: &str = "unix";
#[cfg(windows)]
const EXPECTED_NETWORK: &str = "namedpipe";

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

        // Canonicalized so it's stable regardless of the host's own cwd at
        // spawn time — unlike EUMEAUS_PLUGIN_DIR (a fresh, per-invocation
        // work dir), this points at the plugin's actual installation
        // directory, letting it find sibling files next to its own
        // plugin.toml (e.g. eumeaus-username-search-plugin's sites.toml).
        // Falls back to the raw path if canonicalization fails (e.g. the
        // directory was removed since discovery) rather than failing the
        // whole spawn over a convenience env var.
        let manifest_dir = manifest
            .manifest_dir
            .canonicalize()
            .unwrap_or_else(|_| manifest.manifest_dir.clone());

        let mut child = Command::new(manifest.entrypoint_path())
            .env("EUMEAUS_PLUGIN_DIR", work_dir.path())
            .env("EUMEAUS_PLUGIN_MANIFEST_DIR", &manifest_dir)
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

        let address = match parse_handshake(&handshake_line) {
            Some(address) => address,
            None => {
                let _ = child.kill().await;
                return Err(PluginError::InvalidHandshake(
                    manifest.plugin.name.clone(),
                    handshake_line,
                ));
            }
        };

        let client = match connect_transport(&address).await {
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
        || network != EXPECTED_NETWORK
        || wire != "grpc"
    {
        return None;
    }
    Some(PathBuf::from(address))
}

/// Connects to the plugin's gRPC server at `address` — a filesystem path
/// (Unix domain socket) on Unix, or a `\\.\pipe\...` name (Windows named
/// pipe) on Windows; [`parse_handshake`] already validated it matches
/// [`EXPECTED_NETWORK`] for whichever platform this actually is.
#[cfg(unix)]
async fn connect_transport(address: &Path) -> Result<Channel, tonic::transport::Error> {
    let address = address.to_path_buf();
    // The URI is a required but ignored placeholder: our connector always
    // dials `address` regardless of what tonic passes it.
    Endpoint::try_from("http://[::]:0")
        .expect("static placeholder URI is always valid")
        .connect_with_connector(service_fn(move |_: Uri| {
            let address = address.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(address).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await
}

#[cfg(windows)]
async fn connect_transport(address: &Path) -> Result<Channel, tonic::transport::Error> {
    let pipe_name = address.to_string_lossy().into_owned();
    Endpoint::try_from("http://[::]:0")
        .expect("static placeholder URI is always valid")
        .connect_with_connector(service_fn(move |_: Uri| {
            let pipe_name = pipe_name.clone();
            async move {
                let stream = connect_named_pipe_client(&pipe_name).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await
}

/// Windows named pipe client connections are synchronous-attempt, not
/// async-await-until-ready like a Unix socket connect: `ClientOptions::open`
/// either succeeds immediately or fails with `ERROR_PIPE_BUSY` if a server
/// exists but hasn't posted its next accept yet (a brief window
/// `eumeaus-plugin-sdk`'s own accept loop always reopens quickly) — retry
/// briefly, per tokio's own documented pattern for this exact API, bounded
/// so a genuinely stuck server doesn't hang forever.
#[cfg(windows)]
async fn connect_named_pipe_client(
    pipe_name: &str,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use tokio::net::windows::named_pipe::ClientOptions;

    const ERROR_PIPE_BUSY: i32 = 231; // Win32 ERROR_PIPE_BUSY
    const MAX_ATTEMPTS: u32 = 50; // ~1s total at 20ms between attempts

    let mut last_err = None;
    for _ in 0..MAX_ATTEMPTS {
        match ClientOptions::new().open(pipe_name) {
            Ok(client) => return Ok(client),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.expect("loop only exits via return or after storing an error"))
}
