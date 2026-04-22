//! TCP transport. Binds `127.0.0.1:<port>`, accepts connections, reads
//! one script per connection (length-prefixed, see `FRAMING`), ships
//! it as a [`ScriptRequest`], writes the reply back.
//!
//! Cancellation is cooperative via [`CancellationToken`]: the accept
//! loop `select!`s between the next accept and the cancel future.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::{ScriptRequest, ScriptTransport, TransportHandle};

/// `[u32-be len][utf8 source]` on each connection, same shape for the
/// reply. Version byte + max-length check belong here in a real impl.
pub const FRAMING: &str = "u32-be-len-prefixed";

/// Hard cap on a single frame so a malicious `u32::MAX` doesn't OOM
/// the server. 1 MiB is plenty for user scripts.
const MAX_FRAME_BYTES: u32 = 1 << 20;

#[derive(Debug)]
pub struct TcpTransport {
    /// `0` → OS picks a free port, written to a discovery file so the
    /// CLI can find us.
    pub port: u16,
}

impl ScriptTransport for TcpTransport {
    fn start(
        self: Box<Self>,
        tx: mpsc::Sender<ScriptRequest>,
        cancel: CancellationToken,
    ) -> TransportHandle {
        let port = self.port;
        let cancel_task = cancel.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = run_listener(port, tx, cancel_task).await {
                tracing::error!(error = %e, "tcp script transport listener exited");
            }
        });

        TransportHandle::new("tcp", cancel, task)
    }
}

async fn run_listener(
    port: u16,
    tx: mpsc::Sender<ScriptRequest>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let bound = listener.local_addr()?;
    tracing::info!(addr = %bound, "tcp script transport listening");
    // TODO: write bound.port() to a discovery file under $XDG_RUNTIME_DIR.

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            accept = listener.accept() => {
                let (stream, _peer) = match accept {
                    Ok(pair) => pair,
                    Err(e) => {
                        // One bad handshake (RST, EMFILE, …) must not kill
                        // the whole listener. Log and keep serving.
                        tracing::warn!(error = %e, "tcp accept failed");
                        continue;
                    }
                };

                tokio::spawn({
                        let tx = tx.clone();
                        let cancel = cancel.clone();

                        async move {
                            if let Err(e) = handle_conn(stream, tx, cancel).await {
                                tracing::debug!(error = %e, "tcp script conn closed with error");
                            }
                        }
                });
            }
        }
    }
    Ok(())
}

async fn handle_conn(
    mut stream: TcpStream,
    tx: mpsc::Sender<ScriptRequest>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let source = read_frame(&mut stream).await?;
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

async fn read_frame(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
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
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(bytes).await?;
    Ok(())
}
