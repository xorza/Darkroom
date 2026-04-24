//! TCP transport. Binds `127.0.0.1:<port>`, accepts connections, reads
//! one script per connection and writes one reply back.
//!
//! Without auth, the wire format is `[u32-be len][utf8 source]`.
//! With auth (token = `Some`), each connection is
//! `[16 raw bytes: UUID as u128-be][u32-be src_len][src_bytes]` — the
//! token is compared constant-time; mismatches close the connection
//! without touching the executor.
//!
//! Cancellation is cooperative via [`CancellationToken`]: the accept
//! loop `select!`s between the next accept and the cancel future.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{ScriptRequest, ScriptTransport, TcpScriptConfig, TransportHandle};

/// Hard cap on a single frame so a malicious `u32::MAX` doesn't OOM
/// the server. 1 MiB is plenty for user scripts.
const MAX_FRAME_BYTES: u32 = 1 << 20;

/// TCP script transport. Constructed via [`TcpTransport::bind`] so the
/// caller can inspect [`local_addr`] (useful when `port = 0`) before the
/// accept loop starts.
#[derive(Debug)]
pub struct TcpTransport {
    listener: std::net::TcpListener,
    token: Option<Uuid>,
}

impl TcpTransport {
    /// Bind a loopback listener. `port = 0` asks the OS for a free port —
    /// read it back with [`local_addr`].
    pub fn bind(port: u16, token: Option<Uuid>) -> std::io::Result<Self> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", port))?;
        // Required to convert into a `tokio::net::TcpListener` inside the task.
        listener.set_nonblocking(true)?;
        Ok(Self { listener, token })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

/// Side-effect-free summary of a successful [`start`] call. The caller
/// decides how to surface it (stdout banner for CLI, status bar for GUI,
/// log line for tests).
#[derive(Debug, Clone)]
pub struct TcpStartReport {
    pub addr: SocketAddr,
    pub token: Option<Uuid>,
    /// `Some(path)` if the discovery file was requested and written;
    /// `None` if not requested or write failed.
    pub token_file_written: Option<PathBuf>,
    /// `Some(msg)` if the discovery file was requested but the write
    /// failed. Independent of `token_file_written` — never both `Some`.
    pub token_file_error: Option<String>,
}

/// Boot a TCP transport from config: binds the socket and (if requested)
/// writes the discovery file. Returns the transport together with a
/// pure report — no stdout I/O happens inside, so this is unit-testable.
pub fn start(cfg: &TcpScriptConfig) -> std::io::Result<(TcpTransport, TcpStartReport)> {
    let transport = TcpTransport::bind(cfg.port, cfg.token)?;
    let addr = transport.local_addr()?;

    let (token_file_written, token_file_error) = match &cfg.token_file {
        Some(path) => match write_token_file(path, addr.port(), cfg.token) {
            Ok(()) => (Some(path.clone()), None),
            Err(e) => (None, Some(e.to_string())),
        },
        None => (None, None),
    };

    Ok((
        transport,
        TcpStartReport {
            addr,
            token: cfg.token,
            token_file_written,
            token_file_error,
        },
    ))
}

/// Pure rendering of the discovery-file body. Exposed so tests can
/// byte-diff the output without touching the filesystem.
pub fn render_token_file(port: u16, token: Option<Uuid>) -> String {
    let payload = serde_json::json!({
        "port": port,
        "token": token.map(|t| t.to_string()),
    });
    serde_json::to_string_pretty(&payload).expect("JSON payload is well-formed")
}

fn write_token_file(path: &Path, port: u16, token: Option<Uuid>) -> std::io::Result<()> {
    atomic_write(path, render_token_file(port, token).as_bytes())
}

/// Write `body` to `path` atomically via same-directory temp + rename.
fn atomic_write(path: &Path, body: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = path.to_path_buf();
    let mut name = tmp
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    tmp.set_file_name(name);

    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl ScriptTransport for TcpTransport {
    fn start(
        self: Box<Self>,
        tx: mpsc::Sender<ScriptRequest>,
        cancel: CancellationToken,
    ) -> TransportHandle {
        let Self { listener, token } = *self;
        let cancel_task = cancel.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = run_listener(listener, token, tx, cancel_task).await {
                tracing::error!(error = %e, "tcp script transport listener exited");
            }
        });

        TransportHandle::new(cancel, task)
    }
}

async fn run_listener(
    listener: std::net::TcpListener,
    token: Option<Uuid>,
    tx: mpsc::Sender<ScriptRequest>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let listener = TcpListener::from_std(listener)?;
    let bound = listener.local_addr()?;
    tracing::info!(addr = %bound, auth = token.is_some(), "tcp script transport listening");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            accept = listener.accept() => {
                let (stream, peer) = match accept {
                    Ok(pair) => pair,
                    Err(e) => {
                        // One bad handshake (RST, EMFILE, …) must not kill
                        // the whole listener. Log and keep serving.
                        tracing::warn!(error = %e, "tcp accept failed");
                        continue;
                    }
                };

                let tx = tx.clone();
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, peer, token, tx, cancel).await {
                        tracing::debug!(error = %e, peer = %peer, "tcp script conn closed with error");
                    }
                });
            }
        }
    }
    Ok(())
}

async fn handle_conn(
    mut stream: TcpStream,
    peer: SocketAddr,
    expected_token: Option<Uuid>,
    tx: mpsc::Sender<ScriptRequest>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    if let Some(expected) = expected_token {
        let got = stream.read_u128().await?;
        // XOR-then-zero-check compiles branchless on x86_64/aarch64,
        // so comparison time doesn't leak which byte mismatched.
        if got ^ expected.as_u128() != 0 {
            tracing::warn!(peer = %peer, "tcp script: token rejected, closing connection");
            // Dropping the stream closes the socket and sends FIN; no
            // pending writes to flush on the reject path.
            drop(stream);
            return Ok(());
        }
    }

    let source = read_string_frame(&mut stream).await?;
    let (reply_tx, reply_rx) = oneshot::channel();

    // Bounded send — if the executor is saturated, this awaits and
    // backpressure propagates all the way to the TCP client.
    if tx
        .send(ScriptRequest {
            source,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return Ok(()); // executor dropped, app is shutting down
    }

    let reply = tokio::select! {
        _ = cancel.cancelled() => return Ok(()),
        r = reply_rx => match r {
            Ok(r) => r,
            Err(_) => return Ok(()), // executor dropped the sender
        },
    };

    let payload = match reply.error {
        None => reply.stdout,
        Some(err) => format!("ERROR: {err}\n{}", reply.stdout),
    };
    write_frame(&mut stream, payload.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn read_string_frame(stream: &mut TcpStream) -> std::io::Result<String> {
    let len = stream.read_u32().await?;
    if len > MAX_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame too large: {len} > {MAX_FRAME_BYTES}"),
        ));
    }
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).await?;
    String::from_utf8(buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

async fn write_frame(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    let len =
        u32::try_from(bytes.len()).map_err(|_| std::io::Error::other("reply frame exceeds u32"))?;
    stream.write_u32(len).await?;
    stream.write_all(bytes).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{ScriptAction, ScriptExecutor};

    async fn spawn_executor_with_transport(
        transport: TcpTransport,
    ) -> (
        SocketAddr,
        ScriptExecutor,
        mpsc::UnboundedReceiver<ScriptAction>,
    ) {
        let addr = transport.local_addr().unwrap();
        let (action_tx, action_rx) = mpsc::unbounded_channel::<ScriptAction>();
        let executor =
            ScriptExecutor::new([Box::new(transport) as Box<dyn ScriptTransport>], action_tx);
        (addr, executor, action_rx)
    }

    async fn send_frame(stream: &mut TcpStream, bytes: &[u8]) {
        let len = u32::try_from(bytes.len()).unwrap();
        stream.write_u32(len).await.unwrap();
        stream.write_all(bytes).await.unwrap();
    }

    async fn read_reply(stream: &mut TcpStream) -> String {
        let len = stream.read_u32().await.unwrap() as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await.unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[tokio::test]
    async fn no_auth_accepts_script_and_routes_print_action() {
        let t = TcpTransport::bind(0, None).unwrap();
        let (addr, _executor, mut action_rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        send_frame(&mut s, b"print(\"hi\")").await;
        let reply = read_reply(&mut s).await;
        assert!(!reply.starts_with("ERROR:"), "reply was: {reply}");

        // Rhai `print("hi")` must route through the executor's on_print
        // hook and push ScriptAction::Print("hi") to the action channel.
        let action = tokio::time::timeout(std::time::Duration::from_secs(2), action_rx.recv())
            .await
            .expect("timed out waiting for ScriptAction::Print")
            .expect("action channel closed");
        match action {
            ScriptAction::Print(msg) => assert_eq!(msg, "hi"),
        }
    }

    #[tokio::test]
    async fn auth_accepts_correct_token() {
        let token = Uuid::new_v4();
        let t = TcpTransport::bind(0, Some(token)).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_u128(token.as_u128()).await.unwrap();
        send_frame(&mut s, b"let x = 1;").await;
        let reply = read_reply(&mut s).await;
        assert!(!reply.starts_with("ERROR:"), "got: {reply}");
    }

    #[tokio::test]
    async fn auth_rejects_wrong_token() {
        let correct = Uuid::new_v4();
        let wrong = Uuid::new_v4();
        let t = TcpTransport::bind(0, Some(correct)).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_u128(wrong.as_u128()).await.unwrap();
        send_frame(&mut s, b"let x = 1;").await;

        // Server closes without replying. The exact ErrorKind varies
        // (UnexpectedEof vs ConnectionReset depending on timing/OS), so
        // assert only that the read fails.
        assert!(s.read_u32().await.is_err());
    }

    #[test]
    fn render_token_file_with_token() {
        let token = Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
        let s = render_token_file(45678, Some(token));
        // Serde's pretty printer uses exact indent/newline layout; this
        // assertion pins the on-disk shape so clients can parse stably.
        assert_eq!(
            s,
            "{\n  \"port\": 45678,\n  \"token\": \"12345678-9abc-def0-1122-334455667788\"\n}"
        );
    }

    #[test]
    fn render_token_file_without_token() {
        let s = render_token_file(1234, None);
        assert_eq!(s, "{\n  \"port\": 1234,\n  \"token\": null\n}");
    }

    #[tokio::test]
    async fn start_reports_bound_addr_and_token() {
        let token = Uuid::new_v4();
        let cfg = TcpScriptConfig {
            port: 0,
            token: Some(token),
            token_file: None,
        };
        let (transport, report) = start(&cfg).unwrap();
        // Bound addr matches the transport's own local_addr.
        assert_eq!(report.addr, transport.local_addr().unwrap());
        assert_eq!(report.addr.port(), transport.local_addr().unwrap().port());
        assert_eq!(report.token, Some(token));
        assert!(report.token_file_written.is_none());
        assert!(report.token_file_error.is_none());
    }

    #[tokio::test]
    async fn start_writes_discovery_file_atomically() {
        use std::fs;

        // Per-test directory under the system temp root; Uuid keeps
        // parallel test runs from colliding. Clean up at the end.
        let dir = std::env::temp_dir().join(format!("prism-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("discovery.json");

        let token = Uuid::from_u128(42);
        let cfg = TcpScriptConfig {
            port: 0,
            token: Some(token),
            token_file: Some(path.clone()),
        };
        let (_transport, report) = start(&cfg).unwrap();

        assert_eq!(report.token_file_written.as_deref(), Some(path.as_path()));
        assert!(report.token_file_error.is_none());

        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(&format!("\"port\": {}", report.addr.port())));
        assert!(body.contains(&token.to_string()));

        // Temp file must not linger beside the real one.
        let tmp = path.with_file_name("discovery.json.tmp");
        assert!(!tmp.exists(), "tmp file should have been renamed");

        let _ = fs::remove_dir_all(&dir);
    }
}
