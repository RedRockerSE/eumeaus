//! `eumeaus-plugin-sdk` — first-party ergonomic helper for plugin authors:
//! implements the boilerplate side of the plugin protocol (handshake, gRPC
//! server bootstrap) so a plugin author writes only the collection logic.
//!
//! Plugins are not required to use this SDK or be written in Rust — the
//! protocol is language-agnostic (SPEC.md §2.4) — but this is the
//! first-party ergonomic path.
//!
//! [`serve`] hosts the gRPC server over a Unix domain socket on Unix, or a
//! Windows named pipe on Windows (SPEC.md §2.2) — `eumeaus-plugin-host`'s
//! `host.rs` connects to whichever one this process's handshake line
//! advertises. The named-pipe path (`serve_named_pipe`) is the more
//! involved of the two: unlike `UnixListenerStream`, tokio has no
//! ready-made `Stream` wrapper around
//! `ServerOptions::create`/`NamedPipeServer::connect`'s accept loop, and a
//! Windows named pipe server must always keep one *unconnected* pipe
//! instance alive or a client's connect can spuriously fail — so a
//! background task owns that loop directly (tokio's own documented
//! pattern) and feeds each connected pipe to the tonic server through an
//! mpsc channel instead.

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

// Method signatures are dictated by the generated ProtoPluginRuntime trait
// (tonic::Status is inherently >128 bytes) — same rationale as
// eumeaus-plugin-protocol's own crate-level allow.
#[allow(clippy::result_large_err)]
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

/// Writes the go-plugin-style handshake line to stdout (SPEC.md §2.2):
/// `EUMEAUS-PLUGIN|1|<network>|<address>|grpc`, then flushes — the host is
/// reading this line-by-line from our stdout and won't see it otherwise.
fn write_handshake(network: &str, address: &str) -> std::io::Result<()> {
    println!("EUMEAUS-PLUGIN|1|{network}|{address}|grpc");
    use std::io::Write;
    std::io::stdout().flush()
}

/// Starts the plugin's gRPC server (Unix domain socket, or Windows named
/// pipe) inside/named after the directory the host provided via
/// `EUMEAUS_PLUGIN_DIR`, then writes the handshake line. Runs until the
/// process is killed — this is meant to be the plugin binary's entire
/// `main`.
pub async fn serve<R: PluginRuntime>(runtime: R) -> Result<(), SdkError> {
    let plugin_dir = std::env::var("EUMEAUS_PLUGIN_DIR").map_err(|_| SdkError::MissingPluginDir)?;
    let adapter = Adapter(runtime);

    #[cfg(unix)]
    {
        serve_unix(&plugin_dir, adapter).await
    }
    #[cfg(windows)]
    {
        serve_named_pipe(&plugin_dir, adapter).await
    }
}

#[cfg(unix)]
async fn serve_unix<R: PluginRuntime>(
    plugin_dir: &str,
    adapter: Adapter<R>,
) -> Result<(), SdkError> {
    let socket_path = std::path::Path::new(plugin_dir).join("plugin.sock");
    let _ = std::fs::remove_file(&socket_path);

    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let incoming = tokio_stream::wrappers::UnixListenerStream::new(listener);

    write_handshake("unix", &socket_path.display().to_string())?;

    tonic::transport::Server::builder()
        .add_service(PluginRuntimeServer::new(adapter))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}

/// Wraps a connected [`tokio::net::windows::named_pipe::NamedPipeServer`]
/// only to implement `tonic::transport::server::Connected` on it — tonic
/// doesn't provide that impl itself (unlike `TcpStream`/`UnixStream`), and
/// the orphan rule means it can't be added directly to the foreign type.
/// `AsyncRead`/`AsyncWrite` are simple pass-through delegation; the inner
/// type is already `Unpin` (a plain `PollEvented`, no self-references), so
/// this newtype is too.
#[cfg(windows)]
struct NamedPipeConn(tokio::net::windows::named_pipe::NamedPipeServer);

#[cfg(windows)]
impl tonic::transport::server::Connected for NamedPipeConn {
    /// No peer-address/credential concept for a named pipe worth exposing
    /// (unlike `UdsConnectInfo`/`TcpConnectInfo`) — same as tonic's own
    /// `impl Connected for DuplexStream`.
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {}
}

#[cfg(windows)]
impl tokio::io::AsyncRead for NamedPipeConn {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

#[cfg(windows)]
impl tokio::io::AsyncWrite for NamedPipeConn {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
    }
}

#[cfg(windows)]
async fn serve_named_pipe<R: PluginRuntime>(
    plugin_dir: &str,
    adapter: Adapter<R>,
) -> Result<(), SdkError> {
    use tokio::net::windows::named_pipe::ServerOptions;

    // Named pipes live in their own namespace (\\.\pipe\...), not the
    // filesystem — reusing EUMEAUS_PLUGIN_DIR's own unique tempdir name
    // (eumeaus-plugin-host creates it via `tempfile` with a random
    // suffix) gives a unique pipe name without adding a UUID dependency
    // just for this.
    let dir_name = std::path::Path::new(plugin_dir)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "eumeaus-plugin".to_string());
    let pipe_name = format!(r"\\.\pipe\{dir_name}");

    // The first server instance must exist *before* the handshake line is
    // written, or a client connecting immediately after reading it can
    // fail to find a listener yet — `first_pipe_instance(true)` also
    // catches a genuine name collision loudly instead of silently
    // colliding with an unrelated pipe.
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)?;
    write_handshake("namedpipe", &pipe_name)?;

    // Bridges the named-pipe accept loop into a Stream `serve_with_incoming`
    // can consume — see the module doc for why there's no ready-made
    // wrapper like `UnixListenerStream` for this.
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tokio::spawn(async move {
        loop {
            if server.connect().await.is_err() {
                break;
            }
            let connected = NamedPipeConn(server);
            // Constructed *before* handing the connected instance off, so
            // a next client always finds a listener ready — same ordering
            // tokio's own named_pipe module doc requires.
            server = match ServerOptions::new().create(&pipe_name) {
                Ok(next) => next,
                Err(_) => break,
            };
            if tx.send(Ok::<_, std::io::Error>(connected)).await.is_err() {
                break;
            }
        }
    });
    let incoming = tokio_stream::wrappers::ReceiverStream::new(rx);

    tonic::transport::Server::builder()
        .add_service(PluginRuntimeServer::new(adapter))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}
