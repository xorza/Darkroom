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

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rhai::{Engine, Scope};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, yield_now};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub mod tcp;

/// Runtime configuration for the scripting surface. Built from CLI flags in
/// `main.rs`. `tcp.is_none()` means the TCP listener is off (no transport
/// registered at all).
#[derive(Debug, Clone, Default)]
pub struct ScriptConfig {
    pub tcp: Option<TcpScriptConfig>,
}

#[derive(Debug, Clone)]
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

/// Materialize every transport described by `ScriptConfig`. Each enabled
/// transport binds eagerly so the caller learns OS-assigned ports before
/// any accept loop starts, and can print discovery info synchronously.
/// A per-transport bind failure is logged and treated as "transport
/// disabled"; the app still starts.
pub fn build_transports(cfg: &ScriptConfig) -> Vec<Box<dyn ScriptTransport>> {
    let mut out: Vec<Box<dyn ScriptTransport>> = Vec::new();
    if let Some(tcp_cfg) = &cfg.tcp {
        match tcp::start(tcp_cfg) {
            Ok((transport, report)) => {
                announce_tcp(&report);
                out.push(Box::new(transport));
            }
            Err(e) => {
                tracing::error!(error = %e, "tcp script transport disabled (bind failed)");
            }
        }
    }
    out
}

fn announce_tcp(report: &tcp::TcpStartReport) {
    println!("script-tcp: listening on {}", report.addr);
    match report.token {
        Some(token) => println!("script-tcp: token {token}"),
        None => println!("script-tcp: auth disabled"),
    }
    match &report.token_file {
        Some(Ok(path)) => tracing::info!(path = %path.display(), "wrote script token file"),
        Some(Err(err)) => tracing::warn!(error = %err, "failed to write script token file"),
        None => {}
    }
}

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

/// Work item sent from a transport to the executor. `origin` labels
/// where the request came from (e.g. a TCP peer `"127.0.0.1:54321"`)
/// so Session can attribute script output back to its sender.
/// `reply` is a single-shot channel; the executor runs `source`, then
/// sends one [`ScriptResult`]. If the client has gone away the
/// receiver is dropped and `reply.send` returns `Err` — scripts still
/// run to completion, the reply is just discarded.
#[derive(Debug)]
pub struct ScriptRequest {
    pub origin: String,
    pub source: String,
    pub reply: oneshot::Sender<ScriptResult>,
}

#[derive(Debug, Clone)]
pub struct ScriptResult {
    /// Everything the script sent through Rhai's `print` during its run,
    /// with a `\n` after each call. Empty when no prints occurred.
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
    /// A `print(msg)` call from a script. `origin` identifies the sender
    /// (e.g. a remote peer addr) so Session can label the status line.
    Print { origin: String, msg: String },
}

/// Shared state touched by the `on_print` hook and the executor loop.
/// Holds the current in-flight request's origin plus its accumulating
/// stdout buffer. Because the executor runs one script at a time on a
/// single tokio task, a plain `Mutex` suffices — the hook locks it
/// briefly inside each `print(...)` call.
#[derive(Debug, Default)]
struct RequestState {
    origin: String,
    stdout: String,
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
    let state: Arc<Mutex<RequestState>> = Arc::new(Mutex::new(RequestState::default()));
    let engine = build_engine(state.clone(), action_tx);
    let mut scope = Scope::new();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            r = rx.recv() => {
                let Some(req) = r else { break };
                // Set origin for the `on_print` hook, run, then drain
                // accumulated stdout back into the reply.
                {
                    let mut s = state.lock().unwrap();
                    s.origin = req.origin.clone();
                    s.stdout.clear();
                }
                let error = run_script(&engine, &mut scope, &req.source).err();
                let stdout = std::mem::take(&mut state.lock().unwrap().stdout);
                let _ = req.reply.send(ScriptResult { stdout, error });
            }
        }
        yield_now().await;
    }
}

fn build_engine(
    state: Arc<Mutex<RequestState>>,
    action_tx: mpsc::UnboundedSender<ScriptAction>,
) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_string_size(MAX_STRING_SIZE);
    engine.set_max_array_size(MAX_ARRAY_LEN);
    engine.set_max_map_size(MAX_MAP_LEN);
    engine.set_max_variables(MAX_VARIABLES);
    engine.set_max_strings_interned(MAX_STRINGS_INTERNED);
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_expr_depths(MAX_EXPR_DEPTH, MAX_FN_EXPR_DEPTH);

    // Dual-sink `print`: append to the caller's reply buffer AND notify
    // Session for the local status bar. Hook fires synchronously during
    // the script run, so the per-request state is guaranteed to still
    // describe the active request.
    engine.on_print(move |msg| {
        let origin = {
            let mut s = state.lock().unwrap();
            s.stdout.push_str(msg);
            s.stdout.push('\n');
            s.origin.clone()
        };
        let _ = action_tx.send(ScriptAction::Print {
            origin,
            msg: msg.to_string(),
        });
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

fn run_script(engine: &Engine, scope: &mut Scope, source: &str) -> Result<(), String> {
    engine
        .run_with_scope(scope, source)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_transports_empty_when_no_tcp_config() {
        // Guards against accidentally re-enabling an always-on listener.
        let cfg = ScriptConfig::default();
        assert!(cfg.tcp.is_none());
        let transports = build_transports(&cfg);
        assert!(transports.is_empty());
    }
}
