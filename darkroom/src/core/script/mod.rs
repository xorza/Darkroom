//! Scripting boundary for darkroom.
//!
//! The executor is transport-agnostic: every transport produces
//! [`ScriptRequest`] values into a shared bounded `tokio::mpsc`
//! queue, and the executor runs each one inside a single
//! `rhai::Engine` pinned to the executor task. Each request gets
//! back a single [`ScriptResult`] on its [`tokio::sync::oneshot`]
//! reply channel.
//!
//! Scripts talk back to the editor via a separate [`ScriptMessage`]
//! channel: functions registered on the engine (`print`, `apply`,
//! `run`, …) push messages; [`crate::gui::app::App`] drains them at the top
//! of every frame and applies them through the same intent/undo path
//! the GUI uses. This keeps the executor task off the UI thread.
//!
//! Rhai is sandbox-by-default (no filesystem, process, or network
//! access unless the host opts in), so there's no nil-globals /
//! whitelist-`_ENV` ceremony. We just layer a few resource caps on
//! top: max operations, max string/array/map size — see
//! [`engine::build_engine`].
//!
//! Runtime: a [`ScriptHost`] owns a dedicated tokio runtime (mirroring
//! `WorkerBridge`); the executor loop `await`s the next request (zero
//! CPU when idle), runs it, then yields back to the scheduler.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rhai::{Dynamic, Engine};

use crate::core::background_runtime::BackgroundRuntime;
use crate::core::edit::intent::types::Intent;
use crate::core::wake::Wake;
use scenarium::library::Library;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod engine;
mod session;
pub mod tcp;
#[cfg(test)]
mod tests;

pub use session::{SessionError, SessionRef, SessionStore};
use tcp::TcpScriptConfig;

/// Runtime configuration for the scripting surface. Built from CLI flags in
/// `main.rs`. `tcp.is_none()` means the TCP listener is off (no transport
/// registered at all).
#[derive(Debug, Clone, Default)]
pub struct ScriptConfig {
    pub tcp: Option<TcpScriptConfig>,
}

/// Log a successful bind: the listening line, the auth-disabled warning,
/// and the discovery-file write outcome. Called by [`ScriptHost::start`].
fn announce_tcp(report: &tcp::TcpStartReport) {
    tracing::info!(
        addr = %report.addr,
        auth = report.token.is_some(),
        "script-tcp: listening",
    );
    if report.token.is_none() {
        tracing::warn!("script-tcp: auth disabled");
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

/// Work item sent from a transport to the executor. `origin` labels
/// where the request came from (e.g. a TCP peer `"127.0.0.1:54321"`)
/// for tracing / attribution.
/// `session_id = None` asks the executor to create a fresh session;
/// `Some(id)` resumes an existing one (errors if unknown).
/// `reply` is a single-shot channel; the executor runs `source`, then
/// sends one [`ScriptResult`]. If the client has gone away the
/// receiver is dropped and `reply.send` returns `Err` — scripts still
/// run to completion, the reply is just discarded.
#[derive(Debug)]
pub struct ScriptRequest {
    pub origin: String,
    pub session_id: Option<Uuid>,
    pub source: String,
    pub reply: oneshot::Sender<ScriptResult>,
}

/// One-to-one with the JSON body the TCP transport writes back. Derive-
/// serialized so the wire shape lives here, not split between the struct
/// def and a hand-built JSON payload. `Deserialize` is for test clients
/// (and any future in-process consumer) to parse the reply symmetrically.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScriptResult {
    /// Session the script ran in. Echoed on success (whether resumed or
    /// freshly created). `None` only when the request failed before a
    /// session was resolvable (unknown id, store full).
    pub session: Option<Uuid>,
    /// Everything the script sent through Rhai's `print` during its run,
    /// with a `\n` after each call. Empty when no prints occurred.
    pub print: String,
    /// Rhai-source serialization of the script's final expression value.
    /// Always populated on success (a statement-terminated script yields
    /// `"()\n"`). `None` only when `error` is `Some`.
    pub result: Option<String>,
    pub error: Option<String>,
}

/// Inbound signals from the script executor to [`crate::gui::app::App`]. Each
/// registered script function pushes a variant here; `App` drains the
/// queue at the top of every frame (woken by the host's `Notify` after
/// each successful send). Kept separate from the request/reply channel so
/// the executor can complete a script without round-tripping through the
/// editor for every side effect.
#[derive(Debug)]
pub enum ScriptMessage {
    /// A `print(msg)` call from a script. The TCP client also receives
    /// this text back in `ScriptResult.print`; the local echo (stderr) is
    /// `App`'s job. Per-peer attribution is the transport's (it traces the
    /// peer addr at the tracing layer).
    Print { msg: String },
    /// A batch of graph mutations issued by a script. Built on the
    /// executor side (which has its `Library` snapshot and any other
    /// inputs the actions need) and applied verbatim by the editor through the same
    /// intent/undo path the GUI uses — one batch is one undo entry. Keeps
    /// the script→graph boundary symmetric with the GUI→graph boundary,
    /// with no editor-side per-variant glue. Empty vecs are no-ops.
    Apply(Vec<Intent>),
    /// `run()` — evaluate the graph once. `App` routes it to
    /// [`crate::gui::app::App::run_graph`]; the editor runs on demand,
    /// with no autorun toggle.
    RunOnce,
    /// `shutdown()` — ask the host to quit, via
    /// [`aperture::HostHandle::quit`].
    Shutdown,
}

/// `mpsc::UnboundedSender<ScriptMessage>` + a host-side ping. Every
/// script side-effect needs to wake the host loop so it drains the
/// inbound queue — without this, a script that fires actions on a quiet
/// GUI canvas (no mouse, no keyboard) sees them pile up unnoticed until
/// the next user input. Mirrors the worker path, which pings the same
/// host after every result.
#[derive(Clone)]
struct InboundSender {
    tx: mpsc::UnboundedSender<ScriptMessage>,
    notify: Wake,
}

impl std::fmt::Debug for InboundSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboundSender").finish_non_exhaustive()
    }
}

impl InboundSender {
    fn send(&self, inbound: ScriptMessage) {
        // Channel send only fails if the receiver is dropped, which
        // happens during shutdown — Session is going away, no need to
        // ping it for a redraw it'll never serve.
        if self.tx.send(inbound).is_ok() {
            (self.notify)();
        }
    }
}

/// Buffer accumulating the in-flight script's `print(...)` output. The
/// executor runs one script at a time on a single tokio task, so there's
/// no real contention — the `Mutex` exists because Rhai's `on_print` hook
/// requires a `'static + Sync` callback (sync-mode Rhai). Drained by
/// `run_script` after each request and shipped as `ScriptResult.print`.
type StdoutBuffer = Arc<Mutex<String>>;

/// A background tokio task paired with its cooperative cancel token.
/// Dropping it cancels the token (soft stop) then aborts the task (hard
/// stop), so sockets and discovery files get cleaned up without blocking
/// shutdown of the whole app. Both the transport listener and the executor
/// loop are exactly this — "a task you cancel on drop" — so they share it.
#[derive(Debug)]
pub struct CancellableTask {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl CancellableTask {
    pub fn new(cancel: CancellationToken, task: JoinHandle<()>) -> Self {
        Self {
            cancel,
            task: Some(task),
        }
    }
}

impl Drop for CancellableTask {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Owns the background executor task plus the TCP transport feeding it.
/// Both are [`CancellableTask`]s, so dropping this cancels + aborts each
/// (executor first, then transport) with no hand-written `Drop`.
#[derive(Debug)]
pub struct ScriptExecutor {
    _executor: CancellableTask,
    _transport: CancellableTask,
}

impl ScriptExecutor {
    /// Spawn the TCP transport's listener task and the executor task that
    /// drains it. Must be called from a tokio runtime context. `action_tx`
    /// is the host-side sender for the side effects script functions emit.
    pub fn new(
        transport: tcp::TcpTransport,
        action_tx: mpsc::UnboundedSender<ScriptMessage>,
        library: Arc<Library>,
        notify: Wake,
    ) -> Self {
        let inbound = InboundSender {
            tx: action_tx,
            notify,
        };
        let (tx, rx) = mpsc::channel(REQUEST_QUEUE_DEPTH);
        let transport = transport.start(tx, CancellationToken::new());

        let cancel = CancellationToken::new();
        let cancel_task = cancel.clone();
        let task = tokio::spawn(async move {
            run_executor(rx, cancel_task, inbound, library).await;
        });

        Self {
            _executor: CancellableTask::new(cancel, task),
            _transport: transport,
        }
    }
}

async fn run_executor(
    mut rx: mpsc::Receiver<ScriptRequest>,
    cancel: CancellationToken,
    inbound: InboundSender,
    library: Arc<Library>,
) {
    let state: StdoutBuffer = Arc::new(Mutex::new(String::new()));
    let engine = engine::build_engine(state.clone(), inbound, library);
    let mut sessions = SessionStore::default();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            r = rx.recv() => {
                let Some(req) = r else { break };
                // Isolate per-request panics: a panicking script (or a
                // poisoned buffer) must not kill the executor task, which
                // would silently stop the whole server while the listener
                // keeps accepting connections.
                let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_script(&engine, &mut sessions, &state, &req)
                }))
                .unwrap_or_else(|_| ScriptResult {
                    session: req.session_id,
                    print: String::new(),
                    result: None,
                    error: Some("script execution panicked".to_string()),
                });
                let _ = req.reply.send(reply);
            }
        }
    }
}

/// Run a single request end-to-end: resolve its session, evaluate the
/// script, drain the accumulated stdout, and return the reply.
fn run_script(
    engine: &Engine,
    sessions: &mut SessionStore,
    state: &StdoutBuffer,
    req: &ScriptRequest,
) -> ScriptResult {
    // Opportunistic sweep: every incoming request is a chance to drop
    // sessions that have been idle past the timeout.
    let now = Instant::now();
    sessions.reap(now);

    let session_ref = match sessions.get_or_create(req.session_id, &req.origin, now) {
        Ok(r) => r,
        Err(e) => {
            // Echo the requested id on Unknown so clients can detect
            // "my session was reaped" and request a fresh one.
            let echoed = match &e {
                SessionError::Unknown(id) => Some(*id),
                SessionError::Full { .. } => None,
            };
            return ScriptResult {
                session: echoed,
                print: String::new(),
                result: None,
                error: Some(e.to_string()),
            };
        }
    };
    let SessionRef {
        id: session_id,
        scope,
    } = session_ref;

    let (result, error) = match engine.eval_with_scope::<Dynamic>(scope, &req.source) {
        Ok(dynamic) => match common::serde_rhai::to_string(&dynamic) {
            Ok(s) => (Some(s), None),
            Err(e) => (
                None,
                Some(format!("failed to serialize script result: {e}")),
            ),
        },
        Err(e) => (None, Some(e.to_string())),
    };

    // Drain the hook's accumulator. `mem::take` leaves the buffer empty
    // for the next request, so no explicit clear is needed.
    let print = std::mem::take(&mut *state.lock().unwrap());

    ScriptResult {
        session: Some(session_id),
        print,
        result,
        error,
    }
}

/// Owns the scripting runtime: a dedicated tokio runtime (mirroring
/// `WorkerBridge`), the [`ScriptExecutor`] feeding off it, and the
/// receiving end of the executor→host [`ScriptMessage`] channel the
/// frontend drains each frame. Built only when `--script-tcp` bound the
/// listener; dropping it cancels every task and shuts the runtime down.
pub struct ScriptHost {
    // `executor` and `runtime` are RAII holders, never read after
    // construction: dropping `executor` cancels + aborts the executor and
    // transport tasks, then `runtime` (declared last, so dropped last)
    // shuts down the threads they ran on. Held only for that drop order.
    #[allow(dead_code)]
    executor: ScriptExecutor,
    inbound_rx: mpsc::UnboundedReceiver<ScriptMessage>,
    #[allow(dead_code)]
    runtime: BackgroundRuntime,
}

impl std::fmt::Debug for ScriptHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptHost").finish_non_exhaustive()
    }
}

impl ScriptHost {
    /// Bind the TCP listener (when `cfg.tcp` is set), spin up a runtime,
    /// and start the executor on it. Returns `None` when scripting is off
    /// or the bind failed (logged here) — the normal case, not an error.
    /// `library` is the shared swappable library cell; the executor `load`s
    /// it per call, so promote/publish growth from the GUI is reflected in
    /// running scripts on their next access.
    pub fn start(cfg: &ScriptConfig, library: Arc<Library>, wake: Wake) -> Option<Self> {
        let tcp_cfg = cfg.tcp.as_ref()?;
        let (transport, report) = match tcp::start(tcp_cfg) {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!(error = %e, "script-tcp: bind failed; scripting disabled");
                return None;
            }
        };
        announce_tcp(&report);
        let runtime = match BackgroundRuntime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!(error = %e, "failed to build script runtime; scripting disabled");
                return None;
            }
        };
        let (tx, inbound_rx) = mpsc::unbounded_channel();
        // `ScriptExecutor::new` spawns the executor + transport listener
        // tasks, so it needs an ambient runtime.
        let executor = runtime.enter(|| ScriptExecutor::new(transport, tx, library, wake));
        Some(Self {
            executor,
            inbound_rx,
            runtime,
        })
    }

    /// Non-blocking drain of everything scripts have pushed since the last
    /// frame. `App` applies each message on the UI thread.
    pub fn drain(&mut self) -> Vec<ScriptMessage> {
        let mut out = Vec::new();
        while let Ok(inbound) = self.inbound_rx.try_recv() {
            out.push(inbound);
        }
        out
    }
}

/// The default bind when `--script-tcp` is on with no `--script-bind`.
pub const DEFAULT_BIND: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 34567);
