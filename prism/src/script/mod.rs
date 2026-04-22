#![allow(dead_code)] // unused items on the transport API surface while the client CLI is still internal.

//! Scripting boundary for prism.
//!
//! The executor is transport-agnostic: every transport produces
//! [`ScriptRequest`] values into a shared bounded `tokio::mpsc` queue,
//! and the executor runs each one inside a single `mlua::Lua` VM
//! pinned to the executor task. Each request gets back a single
//! [`ScriptResult`] on its [`tokio::sync::oneshot`] reply channel.
//!
//! Scripts talk back to [`crate::session::Session`] via a separate
//! `ScriptAction` channel: functions registered into Lua (like
//! `prism.print`) push actions; `Session` drains them each frame and
//! applies them. This mirrors the Worker→Session shape and keeps the
//! executor task off the main thread.
//!
//! Runtime: everything runs on the app-wide `#[tokio::main]` runtime.
//! The executor loop `await`s the next request (zero CPU when idle),
//! runs it, then yields back to the scheduler.

use mlua::Lua;
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

/// Side-effect requests from Lua scripts into [`crate::session::Session`].
/// Each registered Lua function pushes a variant here; Session drains
/// the queue every frame and applies them. Kept separate from the
/// request/reply channel so the executor can complete a script without
/// round-tripping through Session for every side effect.
#[derive(Debug)]
pub enum ScriptAction {
    /// `prism.print(msg)` — append `msg` as a status-log line.
    Print(String),
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
    /// tokio runtime context. `action_tx` is the Session-side
    /// receiver for side-effect requests emitted by Lua functions.
    pub fn new<I>(transports: I, action_tx: mpsc::UnboundedSender<ScriptAction>) -> Self
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
        let task = tokio::spawn(async move { run_executor(rx, cancel_task, action_tx).await });

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

async fn run_executor(
    mut rx: mpsc::Receiver<ScriptRequest>,
    cancel: CancellationToken,
    action_tx: mpsc::UnboundedSender<ScriptAction>,
) {
    let lua = match build_lua(action_tx) {
        Ok(lua) => lua,
        Err(err) => {
            tracing::error!(error = %err, "failed to init Lua VM — script executor exiting");
            return;
        }
    };

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            r = rx.recv() => {
                let Some(req) = r else { break };
                let result = run_lua(&lua, &req.source);
                let _ = req.reply.send(result);
            }
        }
        yield_now().await;
    }
}

/// Build a `mlua::Lua` and register the `prism` API table. Uses
/// `Lua::new()` (not Luau's sandbox — that needs a different feature
/// flag) so the base stdlib is available; we expose only the
/// functions we intend as the public surface via `prism.*`.
fn build_lua(action_tx: mpsc::UnboundedSender<ScriptAction>) -> mlua::Result<Lua> {
    let lua = Lua::new();
    let prism = lua.create_table()?;

    let tx = action_tx.clone();
    let print_fn = lua.create_function(move |_, msg: String| {
        let _ = tx.send(ScriptAction::Print(msg));
        Ok(())
    })?;
    prism.set("print", print_fn)?;

    lua.globals().set("prism", prism)?;
    Ok(lua)
}

/// Run one chunk of source. `exec()` discards expression values; a
/// future richer API can swap to `eval()` and pack the return values
/// into [`ScriptResult::stdout`].
fn run_lua(lua: &Lua, source: &str) -> ScriptResult {
    match lua.load(source).set_name("=<script>").exec() {
        Ok(()) => ScriptResult {
            stdout: String::new(),
            error: None,
        },
        Err(err) => ScriptResult {
            stdout: String::new(),
            error: Some(err.to_string()),
        },
    }
}
