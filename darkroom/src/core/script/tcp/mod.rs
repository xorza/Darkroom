//! TCP transport. Binds `127.0.0.1:<port>`, accepts connections, reads
//! scripts from persistent streams and writes one reply per script.
//!
//! Connection lifecycle:
//!   1. (auth only) client sends 16 bytes of UUID auth token.
//!   2. loop: client sends one request frame, server returns one reply
//!      frame. Connection stays open until the client closes it.
//!
//! Request frame (per script):
//!     `[16 bytes: session-id as u128-be][u32-be body_len][body]`
//! A zero session id (`Uuid::nil()`) asks the executor to create a new
//! session. Any other id resumes the matching session's Rhai scope (or
//! errors if the id isn't known).
//!
//! Reply frame (per script):
//!     `[u32-be body_len][body]`
//! JSON body is the serialized [`crate::core::script::ScriptResult`] — fields:
//! `session`, `print`, `result`, `error`. See `ScriptResult`'s doc
//! comment for semantics.
//!
//! `body` (both directions) is LZ4-block-compressed with the original
//! length prepended as a u32-LE — i.e. exactly the layout produced by
//! [`lz4_flex::block::compress_prepend_size`] /
//! [`lz4_flex::block::decompress_size_prepended`]. Compression is
//! mandatory; there is no flag bit. `body_len` bounds the compressed
//! bytes; the embedded original size is validated against
//! `MAX_FRAME_BYTES` before allocation to bound decompression.
//!
//! Auth failure is handled before the loop: on a bad token we drop the
//! stream without sending anything.
//!
//! Cancellation is cooperative via [`CancellationToken`]: the accept
//! loop `select!`s between the next accept and the cancel future.

use std::io::{Error, ErrorKind};
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::core::script::{CancellableTask, ScriptRequest, session};

/// Hard cap on a single frame so a malicious `u32::MAX` doesn't OOM
/// the server. 1 MiB is plenty for user scripts. Applied to both the
/// on-wire (compressed) length and the lz4-embedded original size.
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
    /// of the next request frame. Matches [`session::SESSION_IDLE_TIMEOUT`]
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
            idle: session::SESSION_IDLE_TIMEOUT,
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
    listener: StdTcpListener,
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
        let listener = StdTcpListener::bind(addr)?;
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

/// TCP listener config: bind address, optional auth token, and optional
/// discovery file. Built from CLI flags in `main.rs`; wrapped by
/// [`crate::core::script::ScriptConfig`] and consumed by [`start`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TcpScriptConfig {
    /// Socket address to bind. Port `0` lets the OS pick a free port;
    /// a non-loopback IP widens exposure beyond the local machine and
    /// will emit a warning at startup.
    pub bind: SocketAddr,
    /// Required token clients must present. `None` means `--script-no-auth`
    /// was passed; the listener accepts any client without a handshake.
    /// On the wire the token is 16 raw bytes (the UUID's u128 big-endian
    /// repr). Treat as a secret.
    pub token: Option<Uuid>,
    /// Optional JSON discovery file (`{"port": N, "token": "..."}`) written
    /// atomically at startup.
    pub token_file: Option<PathBuf>,
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

impl TcpTransport {
    /// Spawn the accept loop on the ambient runtime; the returned handle
    /// owns the task (dropping it cancels + aborts the listener). Pairs
    /// requests onto `tx` and stops cooperatively on `cancel`.
    pub fn start(
        self,
        tx: mpsc::Sender<ScriptRequest>,
        cancel: CancellationToken,
    ) -> CancellableTask {
        let Self {
            listener,
            token,
            timeouts,
        } = self;
        let cancel_task = cancel.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = run_listener(listener, token, timeouts, tx, cancel_task).await {
                tracing::error!(error = %e, "tcp script transport listener exited");
            }
        });

        CancellableTask::new(cancel, task)
    }
}

async fn run_listener(
    listener: StdTcpListener,
    token: Option<Uuid>,
    timeouts: TcpTimeouts,
    tx: mpsc::Sender<ScriptRequest>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let listener = TcpListener::from_std(listener)?;

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

/// Wrap an I/O future in a `tokio::time::timeout` and map timer elapse
/// to `ErrorKind::TimedOut` with the given message. Flattens the
/// `Result<Result<_, _>, Elapsed>` onion to a single `io::Result<T>`
/// so call sites are one `?` each.
async fn with_timeout<T, F>(dur: Duration, msg: &'static str, fut: F) -> std::io::Result<T>
where
    F: std::future::Future<Output = std::io::Result<T>>,
{
    timeout(dur, fut)
        .await
        .map_err(|_| Error::new(ErrorKind::TimedOut, msg))?
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
        let got = with_timeout(timeouts.auth, "auth read timed out", stream.read_u128()).await?;
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
        let raw_id = match timeout(timeouts.idle, stream.read_u128()).await {
            Err(_) => {
                tracing::debug!(peer = %peer, "tcp script: idle between-frame timeout");
                return Ok(());
            }
            Ok(Err(e)) if e.kind() == ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(e)) => return Err(e),
            Ok(Ok(v)) => v,
        };
        // All-zero u128 is the wire convention for "create a new session"
        // (see `Uuid::nil`). Any other value is a resume request.
        let session_id = (raw_id != 0).then(|| Uuid::from_u128(raw_id));

        // Once the first byte has landed, the rest of the frame should
        // arrive promptly. Short timeout.
        let source = with_timeout(
            timeouts.frame,
            "frame body read timed out",
            read_string_frame(&mut stream),
        )
        .await?;

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
        with_timeout(
            timeouts.write,
            "reply write timed out",
            write_frame(&mut stream, body.as_bytes()),
        )
        .await?;
    }
}

async fn read_string_frame(stream: &mut TcpStream) -> std::io::Result<String> {
    let buf = read_compressed_frame(stream).await?;
    String::from_utf8(buf).map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))
}

/// Read `[u32 compressed_len][lz4-with-prepended-size payload]` and
/// return the decompressed bytes. Bounds both the compressed frame
/// length and the embedded original length against `MAX_FRAME_BYTES`.
async fn read_compressed_frame(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let len = stream.read_u32().await?;
    if len > MAX_FRAME_BYTES {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("frame too large: {len} > {MAX_FRAME_BYTES}"),
        ));
    }
    if len < 4 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "compressed frame missing size prefix",
        ));
    }
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).await?;
    // lz4_flex::compress_prepend_size emits original size as u32-LE in
    // the first 4 bytes. Cap the decompressed allocation up front.
    let original = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if original > MAX_FRAME_BYTES {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("decompressed size too large: {original} > {MAX_FRAME_BYTES}"),
        ));
    }
    lz4_flex::block::decompress_size_prepended(&buf)
        .map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))
}

async fn write_frame(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    let compressed = lz4_flex::block::compress_prepend_size(bytes);
    let len =
        u32::try_from(compressed.len()).map_err(|_| Error::other("reply frame exceeds u32"))?;
    stream.write_u32(len).await?;
    stream.write_all(&compressed).await?;
    Ok(())
}

#[cfg(test)]
mod tests;
