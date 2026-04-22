#![allow(dead_code)] // Prototype scaffolding — stubs aren't wired up yet.

//! Scripting boundary for prism.
//!
//! The executor is transport-agnostic: every transport produces
//! [`ScriptRequest`] values into a shared bounded `tokio::mpsc` queue,
//! and the executor, polled between frames, runs the source and
//! replies on the request's [`tokio::sync::oneshot`] channel. Adding
//! a new way to submit scripts (browser UI, named pipe, stdin) means
//! implementing [`ScriptTransport`] — the executor, the Lua engine,
//! and the undo/redo plumbing never change.
//!
//! Runtime: everything runs on the app-wide `#[tokio::main]` runtime,
//! including the executor loop spawned by [`ScriptExecutor::new`].
//! The loop `await`s the next request (zero CPU when idle), runs it,
//! and yields back to the scheduler between requests so a stream of
//! scripts can't starve other tokio tasks.

use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, yield_now};
use tokio_util::sync::CancellationToken;

pub mod tcp;

/// Capacity of the transport → executor queue. Small on purpose so
/// flooding clients feel backpressure on their own sending tasks
/// instead of piling up memory in the server.
const REQUEST_QUEUE_DEPTH: usize = 4;

/// Work item sent from a transport to the executor. `reply` is a
/// single-shot channel; the executor runs `source`, then sends one
/// [`ScriptResult`]. If the client has gone away the receiver is
/// dropped and `reply.send` returns `Err` — scripts still run to
/// completion, the reply is just discarded.
#[derive(Debug)]
pub struct ScriptRequest {
    pub source: String,
    pub reply: oneshot::Sender<ScriptResult>,
}

#[derive(Debug, Clone)]
pub struct ScriptResult {
    pub stdout: String,
    pub error: Option<String>,
}

/// Owns a transport's background task. Dropping it cancels the token
/// (cooperative stop) and aborts the task (hard stop) so sockets and
/// discovery files get cleaned up without blocking shutdown of the
/// whole app. For graceful shutdown before drop, call
/// [`TransportHandle::shutdown`] from async context.
#[derive(Debug)]
pub struct TransportHandle {
    pub name: &'static str,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl TransportHandle {
    pub fn new(name: &'static str, cancel: CancellationToken, task: JoinHandle<()>) -> Self {
        Self {
            name,
            cancel,
            task: Some(task),
        }
    }

    /// Cancel the transport and await its task — graceful shutdown.
    /// Prefer this over plain drop when you're in async context.
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for TransportHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// How a transport plugs into the executor: it gets a bounded `Sender`
/// for pushing requests and a [`CancellationToken`] for cooperative
/// shutdown, and returns a handle that owns its background task.
pub trait ScriptTransport: Send {
    fn start(
        self: Box<Self>,
        tx: mpsc::Sender<ScriptRequest>,
        cancel: CancellationToken,
    ) -> TransportHandle;
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// Owns the background executor task plus every transport it feeds.
/// Dropping it cancels everything and aborts the tasks.
#[derive(Debug)]
pub struct ScriptExecutor {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
    _transports: Vec<TransportHandle>,
}

impl ScriptExecutor {
    /// Create the executor, spawn every transport's listener task,
    /// and spawn the executor task itself. Must be called from a
    /// tokio runtime context.
    pub fn new<I>(transports: I) -> Self
    where
        I: IntoIterator<Item = Box<dyn ScriptTransport>>,
    {
        let (tx, rx) = mpsc::channel(REQUEST_QUEUE_DEPTH);
        let handles = transports
            .into_iter()
            .map(|t| {
                let cancel = CancellationToken::new();
                t.start(tx.clone(), cancel)
            })
            .collect();

        let cancel = CancellationToken::new();
        let cancel_task = cancel.clone();
        let task = tokio::spawn(async move { run_executor(rx, cancel_task).await });

        Self {
            cancel,
            task: Some(task),
            _transports: handles,
        }
    }

    /// Cancel and await the executor task — graceful shutdown.
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for ScriptExecutor {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn run_executor(mut rx: mpsc::Receiver<ScriptRequest>, cancel: CancellationToken) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            r = rx.recv() => {
                let Some(req) = r else { break };
                let result = run_lua_stub(&req.source);
                let _ = req.reply.send(result);
            }
        }
        yield_now().await;
    }
}

/// Placeholder for the real `mlua` call. Returns the source unchanged
/// as stdout so transports can be tested end-to-end before Lua lands.
fn run_lua_stub(source: &str) -> ScriptResult {
    ScriptResult {
        stdout: format!("echo: {source}"),
        error: None,
    }
}
