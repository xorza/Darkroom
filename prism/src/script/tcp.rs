//! TCP transport. Binds `127.0.0.1:<port>`, accepts connections, reads
//! one script per connection and writes one reply back.
//!
//! Request framing — without auth:
//!     `[u32-be len][utf8 source]`
//! With auth (`token = Some`):
//!     `[16 bytes: UUID as u128-be][u32-be src_len][src_bytes]`
//! The token is compared constant-time; mismatches close the connection
//! without touching the executor.
//!
//! Reply framing:
//!     `[u32-be len][JSON body]`
//! JSON body shape:
//!     `{"print": <string>, "result": <string|null>, "error": <string|null>}`
//! `print` holds the script's captured `print(...)` output (may be empty).
//! `result` holds the Rhai-source serialization of the final expression
//! on success; `null` on error. `error` holds the error message on
//! failure; `null` on success. Exactly one of `result`/`error` is
//! non-null per successful reply.
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
    /// Bind the listener to `addr`. Port `0` asks the OS for a free port —
    /// read it back with [`local_addr`].
    pub fn bind(addr: SocketAddr, token: Option<Uuid>) -> std::io::Result<Self> {
        let listener = std::net::TcpListener::bind(addr)?;
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
    /// `None` = no discovery file requested.
    /// `Some(Ok(path))` = file written to `path`.
    /// `Some(Err(msg))` = requested but the write failed.
    pub token_file: Option<Result<PathBuf, String>>,
}

/// Boot a TCP transport from config: binds the socket and (if requested)
/// writes the discovery file. Returns the transport together with a
/// pure report — no stdout I/O happens inside, so this is unit-testable.
pub fn start(cfg: &TcpScriptConfig) -> std::io::Result<(TcpTransport, TcpStartReport)> {
    let transport = TcpTransport::bind(cfg.bind, cfg.token)?;
    let addr = transport.local_addr()?;

    if !addr.ip().is_loopback() {
        tracing::warn!(
            addr = %addr,
            "tcp script: non-loopback bind — listener reachable beyond this machine"
        );
    }

    let token_file = cfg.token_file.as_ref().map(|path| {
        write_token_file(path, addr.port(), cfg.token)
            .map(|()| path.clone())
            .map_err(|e| e.to_string())
    });

    Ok((
        transport,
        TcpStartReport {
            addr,
            token: cfg.token,
            token_file,
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
            origin: peer.to_string(),
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

    // `ScriptResult` derives Serialize so its field names define the wire
    // shape. See its doc comment for the JSON layout.
    let body = serde_json::to_string(&reply).expect("script reply is serializable");
    write_frame(&mut stream, body.as_bytes()).await?;
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
    use crate::script::{ScriptAction, ScriptExecutor, ScriptResult};
    use std::net::Ipv4Addr;

    /// Port 0 on the loopback interface — the OS picks a free port.
    fn loopback_ephemeral() -> SocketAddr {
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)
    }

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

    fn parse_reply(raw: &str) -> ScriptResult {
        serde_json::from_str(raw).expect("reply is a ScriptResult JSON body")
    }

    #[tokio::test]
    async fn no_auth_accepts_script_and_dual_sinks_print() {
        let t = TcpTransport::bind(loopback_ephemeral(), None).unwrap();
        let (addr, _executor, mut action_rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        let local = s.local_addr().unwrap();
        send_frame(&mut s, b"print(\"hi\")").await;

        let reply = parse_reply(&read_reply(&mut s).await);
        assert_eq!(reply.print, "hi\n");
        // `print(...)` is a statement, so the final expression is Unit.
        assert_eq!(reply.result.as_deref(), Some("()\n"));
        assert_eq!(reply.error, None);

        // Status sink: Session gets a tagged Print action.
        let action = tokio::time::timeout(std::time::Duration::from_secs(2), action_rx.recv())
            .await
            .expect("timed out waiting for ScriptAction::Print")
            .expect("action channel closed");
        match action {
            ScriptAction::Print { origin, msg } => {
                assert_eq!(msg, "hi");
                assert_eq!(origin, local.to_string());
            }
        }
    }

    #[tokio::test]
    async fn script_result_is_last_expression() {
        let t = TcpTransport::bind(loopback_ephemeral(), None).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        send_frame(&mut s, b"40 + 2").await;
        let reply = parse_reply(&read_reply(&mut s).await);
        assert_eq!(reply.print, "");
        assert_eq!(reply.result.as_deref(), Some("42\n"));
        assert_eq!(reply.error, None);
    }

    #[tokio::test]
    async fn script_result_object_map() {
        let t = TcpTransport::bind(loopback_ephemeral(), None).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        send_frame(&mut s, b"#{ a: 1, b: 2 }").await;
        let reply = parse_reply(&read_reply(&mut s).await);
        let r = reply.result.expect("result present");
        // serde_rhai orders map keys alphabetically via BTreeMap iteration.
        assert!(r.starts_with("#{\n"), "got: {r}");
        assert!(r.contains("a: 1,"));
        assert!(r.contains("b: 2,"));
    }

    #[tokio::test]
    async fn script_error_populates_error_field() {
        let t = TcpTransport::bind(loopback_ephemeral(), None).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        // Parse error — Rhai rejects bare `let` without an identifier.
        send_frame(&mut s, b"let = 1").await;
        let reply = parse_reply(&read_reply(&mut s).await);
        assert_eq!(reply.print, "");
        assert!(reply.result.is_none(), "result should be null on error");
        assert!(reply.error.is_some(), "error should be populated");
    }

    #[tokio::test]
    async fn auth_accepts_correct_token() {
        let token = Uuid::new_v4();
        let t = TcpTransport::bind(loopback_ephemeral(), Some(token)).unwrap();
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
        let t = TcpTransport::bind(loopback_ephemeral(), Some(correct)).unwrap();
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
            bind: loopback_ephemeral(),
            token: Some(token),
            token_file: None,
        };
        let (transport, report) = start(&cfg).unwrap();
        // Bound addr matches the transport's own local_addr.
        assert_eq!(report.addr, transport.local_addr().unwrap());
        assert_eq!(report.addr.port(), transport.local_addr().unwrap().port());
        assert_eq!(report.token, Some(token));
        assert!(report.token_file.is_none());
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
            bind: loopback_ephemeral(),
            token: Some(token),
            token_file: Some(path.clone()),
        };
        let (_transport, report) = start(&cfg).unwrap();

        assert_eq!(
            report.token_file.as_ref().unwrap().as_deref(),
            Ok(path.as_path())
        );

        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(&format!("\"port\": {}", report.addr.port())));
        assert!(body.contains(&token.to_string()));

        // Temp file must not linger beside the real one.
        let tmp = path.with_file_name("discovery.json.tmp");
        assert!(!tmp.exists(), "tmp file should have been renamed");

        let _ = fs::remove_dir_all(&dir);
    }
}
