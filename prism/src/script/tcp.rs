//! TCP transport. Binds `127.0.0.1:<port>`, accepts connections, reads
//! scripts from persistent streams and writes one reply per script.
//!
//! Connection lifecycle:
//!   1. (auth only) client sends 16 bytes of UUID auth token.
//!   2. loop: client sends one request frame, server returns one reply
//!      frame. Connection stays open until the client closes it.
//!
//! Request frame (per script):
//!     `[16 bytes: session-id as u128-be][u32-be src_len][src_bytes]`
//! A zero session id (`Uuid::nil()`) asks the executor to create a new
//! session. Any other id resumes the matching session's Rhai scope (or
//! errors if the id isn't known).
//!
//! Reply frame (per script):
//!     `[u32-be len][JSON body]`
//! JSON body is the serialized [`super::ScriptResult`] — fields:
//! `session`, `print`, `result`, `error`. See `ScriptResult`'s doc
//! comment for semantics.
//!
//! Auth failure is handled before the loop: on a bad token we drop the
//! stream without sending anything.
//!
//! Cancellation is cooperative via [`CancellationToken`]: the accept
//! loop `select!`s between the next accept and the cancel future.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{ScriptRequest, ScriptTransport, TcpScriptConfig, TransportHandle};

/// Hard cap on a single frame so a malicious `u32::MAX` doesn't OOM
/// the server. 1 MiB is plenty for user scripts.
const MAX_FRAME_BYTES: u32 = 1 << 20;

/// Upper bound on concurrent TCP client connections. Extras are refused
/// at accept time (socket closed immediately, no auth exchange). Picked
/// low because each connection is a long-lived channel that can resume
/// a session; bursty short connections shouldn't need more.
const MAX_CONNECTIONS: usize = 4;

/// Read/write deadlines that guard against misbehaving or hung clients.
/// Every read point and the reply write get wrapped in a `timeout`; an
/// elapsed timer surfaces as `io::ErrorKind::TimedOut`. Production
/// callers pass [`TcpTimeouts::default`] to [`TcpTransport::bind`];
/// tests may pass short values to drive timeout paths in real time.
#[derive(Debug, Clone, Copy)]
pub struct TcpTimeouts {
    /// Window for the initial auth token frame (16 bytes). Keep short —
    /// a legitimate client has the token ready on connect.
    pub auth: Duration,
    /// Window between the end of one reply and the start (first byte)
    /// of the next request frame. Matches `SESSION_IDLE_TIMEOUT` range
    /// because a client who hasn't sent anything for this long is
    /// probably better off reconnecting on a new socket.
    pub idle: Duration,
    /// Window for the remaining bytes of an in-flight request once its
    /// first byte has been received. Short — the client has committed.
    pub frame: Duration,
    /// Window for writing the JSON reply. Caps the damage a slow/dead
    /// reader can do by filling the kernel TX buffer.
    pub write: Duration,
}

impl Default for TcpTimeouts {
    fn default() -> Self {
        Self {
            auth: Duration::from_secs(10),
            idle: Duration::from_secs(600),
            frame: Duration::from_secs(30),
            write: Duration::from_secs(30),
        }
    }
}

/// TCP script transport. Constructed via [`TcpTransport::bind`] so the
/// caller can inspect [`local_addr`] (useful when `port = 0`) before the
/// accept loop starts.
#[derive(Debug)]
pub struct TcpTransport {
    listener: std::net::TcpListener,
    token: Option<Uuid>,
    timeouts: TcpTimeouts,
}

impl TcpTransport {
    /// Bind the listener to `addr`. Port `0` asks the OS for a free
    /// port — read it back with [`Self::local_addr`]. Production
    /// callers pass `TcpTimeouts::default()`; tests may pass short
    /// values to exercise timeout paths in real time.
    pub fn bind(
        addr: SocketAddr,
        token: Option<Uuid>,
        timeouts: TcpTimeouts,
    ) -> std::io::Result<Self> {
        let listener = std::net::TcpListener::bind(addr)?;
        // Required to convert into a `tokio::net::TcpListener` inside the task.
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            token,
            timeouts,
        })
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
    let transport = TcpTransport::bind(cfg.bind, cfg.token, TcpTimeouts::default())?;
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
        let Self {
            listener,
            token,
            timeouts,
        } = *self;
        let cancel_task = cancel.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = run_listener(listener, token, timeouts, tx, cancel_task).await {
                tracing::error!(error = %e, "tcp script transport listener exited");
            }
        });

        TransportHandle::new(cancel, task)
    }
}

async fn run_listener(
    listener: std::net::TcpListener,
    token: Option<Uuid>,
    timeouts: TcpTimeouts,
    tx: mpsc::Sender<ScriptRequest>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let listener = TcpListener::from_std(listener)?;
    let bound = listener.local_addr()?;
    tracing::info!(addr = %bound, auth = token.is_some(), "tcp script transport listening");

    // One permit per concurrent connection. Cap keeps FD/memory use
    // bounded if a client spams connects; extras are closed at accept
    // time without going through auth or the executor.
    let conn_limit = Arc::new(Semaphore::new(MAX_CONNECTIONS));

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

                // Try to take a connection slot without blocking the accept
                // loop. If we can't, close the new socket immediately —
                // client sees EOF on its first read.
                let permit = match Arc::clone(&conn_limit).try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        tracing::warn!(
                            peer = %peer,
                            max = MAX_CONNECTIONS,
                            "tcp script: connection limit reached, dropping new connection"
                        );
                        drop(stream);
                        continue;
                    }
                };

                let tx = tx.clone();
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    // Permit held for the lifetime of the task; released
                    // when `handle_conn` returns and `_permit` drops.
                    let _permit = permit;
                    if let Err(e) = handle_conn(stream, peer, token, timeouts, tx, cancel).await {
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
    timeouts: TcpTimeouts,
    tx: mpsc::Sender<ScriptRequest>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    // Auth once per connection with a tight read deadline — a legitimate
    // client has the token in hand on connect.
    if let Some(expected) = expected_token {
        let got = timeout(timeouts.auth, stream.read_u128())
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "auth read timed out")
            })??;
        if got ^ expected.as_u128() != 0 {
            tracing::warn!(peer = %peer, "tcp script: token rejected, closing connection");
            // Drop sends FIN; no pending writes to flush on the reject path.
            drop(stream);
            return Ok(());
        }
    }

    // Persistent connection: read [16B session-id][u32 src-len][src] per
    // request, dispatch, reply, loop. Exits cleanly on client close (EOF).
    loop {
        // Session-id is the first byte of a new request frame. Long
        // timeout — the client may legitimately idle between frames.
        let session_id = match timeout(timeouts.idle, stream.read_u128()).await {
            Err(_) => {
                tracing::debug!(peer = %peer, "tcp script: idle between-frame timeout");
                return Ok(());
            }
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(e)) => return Err(e),
            Ok(Ok(0)) => None,
            Ok(Ok(id)) => Some(Uuid::from_u128(id)),
        };

        // Once the first byte has landed, the rest of the frame should
        // arrive promptly. Short timeout.
        let source = timeout(timeouts.frame, read_string_frame(&mut stream))
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "frame body read timed out")
            })??;

        let (reply_tx, reply_rx) = oneshot::channel();
        // Bounded send — backpressure propagates to the TCP client.
        if tx
            .send(ScriptRequest {
                origin: peer.to_string(),
                session_id,
                source,
                reply: reply_tx,
            })
            .await
            .is_err()
        {
            return Ok(()); // executor dropped; app shutting down
        }

        // No timeout on the executor reply: Rhai's MAX_OPERATIONS /
        // MAX_EXPR_DEPTH caps bound worst-case script runtime, and a
        // dropped sender returns Err immediately.
        let reply = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            r = reply_rx => match r {
                Ok(r) => r,
                Err(_) => return Ok(()), // executor dropped the sender
            },
        };

        // `ScriptResult` derives Serialize so its field names define the
        // wire shape. See its doc comment for the JSON layout.
        let body = serde_json::to_string(&reply).expect("script reply is serializable");
        timeout(timeouts.write, write_frame(&mut stream, body.as_bytes()))
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "reply write timed out")
            })??;
    }
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

    /// Send one script request frame: 16B session-id (nil = new) + u32
    /// length + source bytes. Use this instead of raw writes so tests
    /// stay in sync with the wire format.
    async fn send_request(stream: &mut TcpStream, session_id: Option<Uuid>, source: &[u8]) {
        let id = session_id.map(|u| u.as_u128()).unwrap_or(0);
        stream.write_u128(id).await.unwrap();
        let len = u32::try_from(source.len()).unwrap();
        stream.write_u32(len).await.unwrap();
        stream.write_all(source).await.unwrap();
    }

    /// Legacy frame-writer, kept for the auth-only tests where the
    /// connection is expected to close before we reach a script frame.
    async fn send_raw_len_bytes(stream: &mut TcpStream, bytes: &[u8]) {
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
        let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
        let (addr, _executor, mut action_rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        let local = s.local_addr().unwrap();
        send_request(&mut s, None, b"print(\"hi\")").await;

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
        let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        send_request(&mut s, None, b"40 + 2").await;
        let reply = parse_reply(&read_reply(&mut s).await);
        assert_eq!(reply.print, "");
        assert_eq!(reply.result.as_deref(), Some("42\n"));
        assert_eq!(reply.error, None);
        assert!(reply.session.is_some());
    }

    #[tokio::test]
    async fn script_result_object_map() {
        let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        send_request(&mut s, None, b"#{ a: 1, b: 2 }").await;
        let reply = parse_reply(&read_reply(&mut s).await);
        let r = reply.result.expect("result present");
        // serde_rhai orders map keys alphabetically via BTreeMap iteration.
        assert!(r.starts_with("#{\n"), "got: {r}");
        assert!(r.contains("a: 1,"));
        assert!(r.contains("b: 2,"));
    }

    #[tokio::test]
    async fn session_resume_preserves_scope_across_frames() {
        let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;
        let mut s = TcpStream::connect(addr).await.unwrap();

        // Frame 1: create session, bind x=42.
        send_request(&mut s, None, b"let x = 42;").await;
        let reply1 = parse_reply(&read_reply(&mut s).await);
        let session = reply1.session.expect("session id returned");

        // Frame 2 on the same connection: resume, read x.
        send_request(&mut s, Some(session), b"x").await;
        let reply2 = parse_reply(&read_reply(&mut s).await);
        assert_eq!(reply2.session, Some(session));
        assert_eq!(reply2.result.as_deref(), Some("42\n"));
    }

    #[tokio::test]
    async fn unknown_session_id_returns_error_and_echoes_id() {
        let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;
        let mut s = TcpStream::connect(addr).await.unwrap();

        let ghost = Uuid::new_v4();
        send_request(&mut s, Some(ghost), b"1").await;
        let reply = parse_reply(&read_reply(&mut s).await);
        assert_eq!(reply.session, Some(ghost));
        assert!(reply.result.is_none());
        let err = reply.error.expect("error set");
        assert!(err.contains("unknown session"), "got: {err}");
    }

    #[tokio::test]
    async fn script_error_populates_error_field() {
        let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        // Parse error — Rhai rejects bare `let` without an identifier.
        send_request(&mut s, None, b"let = 1").await;
        let reply = parse_reply(&read_reply(&mut s).await);
        assert_eq!(reply.print, "");
        assert!(reply.result.is_none(), "result should be null on error");
        assert!(reply.error.is_some(), "error should be populated");
    }

    #[tokio::test]
    async fn auth_accepts_correct_token() {
        let token = Uuid::new_v4();
        let t =
            TcpTransport::bind(loopback_ephemeral(), Some(token), TcpTimeouts::default()).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_u128(token.as_u128()).await.unwrap();
        send_request(&mut s, None, b"let x = 1;").await;
        let reply = parse_reply(&read_reply(&mut s).await);
        assert!(reply.error.is_none(), "got error: {:?}", reply.error);
    }

    #[tokio::test]
    async fn auth_rejects_wrong_token() {
        let correct = Uuid::new_v4();
        let wrong = Uuid::new_v4();
        let t = TcpTransport::bind(loopback_ephemeral(), Some(correct), TcpTimeouts::default())
            .unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_u128(wrong.as_u128()).await.unwrap();
        // After the bad token, the server has already closed its side —
        // we send a legitimate-shaped request payload anyway to confirm
        // it doesn't get processed, and then observe the EOF on read.
        let _ = send_raw_len_bytes(&mut s, b"let x = 1;").await;

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

    // ----- Timeout tests -----
    //
    // Use short real-time values so the tests themselves finish in
    // hundreds of milliseconds without mocking the clock.

    fn short_timeouts() -> TcpTimeouts {
        TcpTimeouts {
            auth: Duration::from_millis(150),
            idle: Duration::from_millis(150),
            frame: Duration::from_millis(150),
            write: Duration::from_millis(150),
        }
    }

    /// Quiet client that never sends a frame is closed by the idle-between-
    /// frames timeout.
    #[tokio::test]
    async fn idle_connection_closes_after_timeout() {
        let t = TcpTransport::bind(loopback_ephemeral(), None, short_timeouts()).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        // Send nothing. Within ~150ms the server should give up and close.
        let start = std::time::Instant::now();
        let mut buf = [0u8; 4];
        let n = s.read(&mut buf).await.unwrap_or(0);
        assert_eq!(n, 0, "expected EOF, got {n} bytes");
        assert!(
            start.elapsed() >= Duration::from_millis(100),
            "closed too fast: {:?}",
            start.elapsed()
        );
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "closed too slow: {:?}",
            start.elapsed()
        );
    }

    /// With auth on, a client that connects but never sends the token is
    /// closed by the auth-read timeout.
    #[tokio::test]
    async fn auth_timeout_closes_silent_client() {
        let token = Uuid::new_v4();
        let t = TcpTransport::bind(loopback_ephemeral(), Some(token), short_timeouts()).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        // No token sent. Server must close without replying.
        let mut buf = [0u8; 4];
        let n = s.read(&mut buf).await.unwrap_or(0);
        assert_eq!(n, 0);
    }

    /// A client that sends just the session-id prefix and then stalls is
    /// closed by the mid-frame timeout (shorter than the idle window).
    #[tokio::test]
    async fn partial_frame_closes_after_frame_timeout() {
        let t = TcpTransport::bind(loopback_ephemeral(), None, short_timeouts()).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        let mut s = TcpStream::connect(addr).await.unwrap();
        // Send 16B nil session id, then stall (no length-prefix, no src).
        s.write_u128(0).await.unwrap();
        let mut buf = [0u8; 4];
        let n = s.read(&mut buf).await.unwrap_or(0);
        assert_eq!(n, 0);
    }

    /// Beyond MAX_CONNECTIONS concurrent clients, extras are closed at
    /// accept time without going through auth or the executor.
    #[tokio::test]
    async fn excess_connections_are_refused() {
        // Default timeouts so the holders don't get reaped mid-test.
        let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
        let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

        // Hold MAX_CONNECTIONS sockets open. For each, send+receive a
        // no-op script: the reply proves the server has already
        // accepted, authed, and taken the permit. No wall-clock sleep.
        let mut holders = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            let mut s = TcpStream::connect(addr).await.unwrap();
            send_request(&mut s, None, b"1").await;
            let reply = parse_reply(&read_reply(&mut s).await);
            assert_eq!(reply.error, None);
            holders.push(s);
        }

        // Over-the-limit connect: accept() succeeds (OS-level 3-way
        // handshake), but the server drops the stream immediately, so
        // our read sees EOF.
        let mut extra = TcpStream::connect(addr).await.unwrap();
        let mut buf = [0u8; 4];
        let n = extra.read(&mut buf).await.unwrap_or(0);
        assert_eq!(n, 0, "over-limit connection should be closed immediately");

        // Drop one holder → its permit frees. We need the server's spawned
        // task to observe the close and release the permit, so probe with
        // a retry loop instead of a fixed sleep.
        drop(holders.pop());
        let mut admitted = None;
        for _ in 0..50 {
            let mut s = TcpStream::connect(addr).await.unwrap();
            send_request(&mut s, None, b"1").await;
            let mut hdr = [0u8; 4];
            if s.peek(&mut hdr).await.unwrap_or(0) == 4 {
                admitted = Some(s);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut admitted = admitted.expect("permit should free after holder closes");
        let reply = parse_reply(&read_reply(&mut admitted).await);
        assert_eq!(reply.error, None);
    }
}
