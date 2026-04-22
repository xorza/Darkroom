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
//! Runtime: everything async runs on the app-wide `#[tokio::main]`
//! runtime. The executor itself is synchronous — `tick` is called
//! from the egui render loop, uses `try_recv`, and never awaits.

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub mod tcp;

/// Capacity of the transport → executor queue. Bounded so a flooding
/// client applies backpressure to its own sending task rather than
/// growing the executor's memory without limit.
const REQUEST_QUEUE_DEPTH: usize = 32;

/// Max number of scripts the executor runs per frame. Caps the UI
/// stall a single burst of queued requests can cause.
const MAX_REQUESTS_PER_FRAME: usize = 4;

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

/// Owns the request receiver and (eventually) the Lua state. Polled
/// once per frame from `MainUi::render` — no blocking, no awaiting.
#[derive(Debug)]
pub struct ScriptExecutor {
    rx: mpsc::Receiver<ScriptRequest>,
    _transports: Vec<TransportHandle>,
}

impl ScriptExecutor {
    pub fn new(transports: Vec<Box<dyn ScriptTransport>>) -> Self {
        let (tx, rx) = mpsc::channel(REQUEST_QUEUE_DEPTH);
        let handles = transports
            .into_iter()
            .map(|t| {
                let cancel = CancellationToken::new();
                t.start(tx.clone(), cancel)
            })
            .collect();
        Self {
            rx,
            _transports: handles,
        }
    }

    /// Drain up to [`MAX_REQUESTS_PER_FRAME`] requests and run them.
    /// Called once per frame from the egui render loop.
    pub fn tick(&mut self) {
        for _ in 0..MAX_REQUESTS_PER_FRAME {
            let Ok(req) = self.rx.try_recv() else { break };
            let result = run_lua_stub(&req.source);
            let _ = req.reply.send(result);
        }
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
