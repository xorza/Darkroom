//! Scripting boundary for prism.
//!
//! The executor is transport-agnostic: every transport produces
//! [`ScriptRequest`] values into a shared bounded `tokio::mpsc`
//! queue, and the executor runs each one inside a single
//! `rhai::Engine` pinned to the executor task. Each request gets
//! back a single [`ScriptResult`] on its [`tokio::sync::oneshot`]
//! reply channel.
//!
//! Scripts talk back to [`crate::session::Session`] via a separate
//! `ScriptAction` channel: functions registered on the engine (like
//! `prism::print`) push actions; `Session` drains them each frame
//! and applies them. This mirrors the Worker→Session shape and
//! keeps the executor task off the main thread.
//!
//! Rhai is sandbox-by-default (no filesystem, process, or network
//! access unless the host opts in), so there's no nil-globals /
//! whitelist-`_ENV` ceremony. We just layer a few resource caps on
//! top: max operations, max string/array/map size — see
//! [`build_engine`].
//!
//! Runtime: everything runs on the app-wide `#[tokio::main]`
//! runtime. The executor loop `await`s the next request (zero CPU
//! when idle), runs it, then yields back to the scheduler.

use rhai::{Engine, Scope};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, yield_now};
use tokio_util::sync::CancellationToken;

pub mod tcp;

/// Capacity of the transport → executor queue. Small on purpose so
/// flooding clients feel backpressure on their own sending tasks
/// instead of piling up memory in the server.
const REQUEST_QUEUE_DEPTH: usize = 4;

/// Upper bound on the number of Rhai operations a single chunk can
/// perform. Rhai counts operations at every AST node, so this is
/// a rough proxy for CPU time. 10M lets legitimate scripts run for
/// several seconds on modern hardware; infinite loops trip long
/// before that.
const MAX_OPERATIONS: u64 = 10_000_000;

/// Largest string / array / object-map a script can construct. Picks
/// a point where honest usage fits comfortably but DoS attacks
/// (`"a".repeat(2 ^ 30)`) trip immediately.
const MAX_STRING_SIZE: usize = 1 << 20; // 1 MiB
const MAX_ARRAY_LEN: usize = 100_000;
const MAX_MAP_LEN: usize = 100_000;

/// Concurrent live variables in a script's scope. Rhai has no
/// byte-level memory cap, so this is the closest proxy: bounds how
/// many values can coexist. 256 is ample for any legitimate script.
const MAX_VARIABLES: usize = 256;

/// Cap on Rhai's interned-string pool. Protects against
/// distinct-string-flood DoS.
const MAX_STRINGS_INTERNED: usize = 1024;

/// Deepest function recursion a script may perform.
const MAX_CALL_LEVELS: usize = 64;

/// Expression / function nesting depth caps passed to
/// `set_max_expr_depths(expr, fn_expr)`. Guards the parser and
/// evaluator against deeply-nested-AST DoS.
const MAX_EXPR_DEPTH: usize = 64;
const MAX_FN_EXPR_DEPTH: usize = 32;

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

/// Side-effect requests from scripts into [`crate::session::Session`].
/// Each registered script function pushes a variant here; Session drains
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
/// whole app.
#[derive(Debug)]
pub struct TransportHandle {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl TransportHandle {
    pub fn new(cancel: CancellationToken, task: JoinHandle<()>) -> Self {
        Self {
            cancel,
            task: Some(task),
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
    /// receiver for side-effect requests emitted by script functions.
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
    let engine = build_engine(&action_tx);
    let mut scope = Scope::new();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            r = rx.recv() => {
                let Some(req) = r else { break };
                let result = run_script(&engine, &mut scope, &req.source);
                let _ = req.reply.send(result);
            }
        }
        yield_now().await;
    }
}

fn build_engine(action_tx: &mpsc::UnboundedSender<ScriptAction>) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_string_size(MAX_STRING_SIZE);
    engine.set_max_array_size(MAX_ARRAY_LEN);
    engine.set_max_map_size(MAX_MAP_LEN);
    engine.set_max_variables(MAX_VARIABLES);
    engine.set_max_strings_interned(MAX_STRINGS_INTERNED);
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_expr_depths(MAX_EXPR_DEPTH, MAX_FN_EXPR_DEPTH);

    let print_tx = action_tx.clone();
    engine.on_print(move |msg| {
        let _ = print_tx.send(ScriptAction::Print(msg.to_string()));
    });

    engine.on_debug(|msg, src, pos| {
        tracing::debug!(
            target: "prism::script",
            src = src.unwrap_or("<script>"),
            line = pos.line().unwrap_or(0),
            col = pos.position().unwrap_or(0),
            "{msg}"
        );
    });

    engine
}

fn run_script(engine: &Engine, scope: &mut Scope, source: &str) -> ScriptResult {
    match engine.run_with_scope(scope, source) {
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
