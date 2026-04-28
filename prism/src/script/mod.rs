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
use std::time::Instant;

use rhai::{Array, Dynamic, Engine};
use scenarium::prelude::FuncLib;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, yield_now};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod session;
pub mod tcp;

pub use session::{SessionError, SessionRef, SessionStore};

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

/// Successfully bound transport plus its caller-renderable report.
/// Returned from [`build_transports`] so the caller can decide how to
/// surface discovery info (stdout banner for CLI, status bar for GUI,
/// silent in tests) before handing the transport to [`ScriptExecutor`].
#[derive(Debug)]
pub struct StartedTransport {
    pub transport: Box<dyn ScriptTransport>,
    pub report: TransportReport,
}

/// Identifies a transport in startup outcomes. Carried on both the
/// success side (via [`TransportReport`]) and the failure side (via
/// [`TransportBindError`]) so an empty config and a failed bind are
/// always distinguishable by *which* transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Tcp,
}

/// Per-transport startup report. One variant per supported transport.
#[derive(Debug, Clone)]
pub enum TransportReport {
    Tcp(tcp::TcpStartReport),
}

/// Bind failure tagged with the transport that failed, so callers can
/// log meaningfully without inferring kind from position in the result
/// vec.
#[derive(Debug)]
pub struct TransportBindError {
    pub kind: TransportKind,
    pub error: std::io::Error,
}

/// Materialize every transport described by `ScriptConfig`. Each enabled
/// transport binds eagerly so the caller learns OS-assigned ports before
/// any accept loop starts. Returns one entry per enabled transport: `Ok`
/// with the bound transport + report, or `Err` with the failing
/// transport's kind and bind error. Empty result means the config
/// enabled no transports — that's the normal case for tests and the
/// GUI without `--script-tcp`, not a degraded state. Pure with respect
/// to stdout — surfacing is the caller's job (see [`announce`]).
pub fn build_transports(cfg: &ScriptConfig) -> Vec<Result<StartedTransport, TransportBindError>> {
    let mut out = Vec::new();
    if let Some(tcp_cfg) = &cfg.tcp {
        out.push(
            tcp::start(tcp_cfg)
                .map(|(transport, report)| StartedTransport {
                    transport: Box::new(transport),
                    report: TransportReport::Tcp(report),
                })
                .map_err(|error| TransportBindError {
                    kind: TransportKind::Tcp,
                    error,
                }),
        );
    }
    out
}

/// Print a transport's discovery banner to stdout (and log token-file
/// writes via tracing). Split out from [`build_transports`] so binding
/// stays pure and unit-testable.
pub fn announce(report: &TransportReport) {
    match report {
        TransportReport::Tcp(r) => announce_tcp(r),
    }
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
pub trait ScriptTransport: Send + std::fmt::Debug {
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
    pub fn new<I>(
        transports: I,
        action_tx: mpsc::UnboundedSender<ScriptAction>,
        func_lib: Arc<FuncLib>,
    ) -> Self
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
        let task =
            tokio::spawn(async move { run_executor(rx, cancel_task, action_tx, func_lib).await });

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
    func_lib: Arc<FuncLib>,
) {
    let state: Arc<Mutex<RequestState>> = Arc::new(Mutex::new(RequestState::default()));
    let engine = build_engine(state.clone(), action_tx, func_lib);
    let mut sessions = SessionStore::default();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            r = rx.recv() => {
                let Some(req) = r else { break };
                let reply = run_script(&engine, &mut sessions, &state, &req);
                let _ = req.reply.send(reply);
            }
        }
        yield_now().await;
    }
}

fn build_engine(
    state: Arc<Mutex<RequestState>>,
    action_tx: mpsc::UnboundedSender<ScriptAction>,
    func_lib: Arc<FuncLib>,
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

    // `list_funcs()` → array of strings: every name in the live FuncLib.
    // Insertion order; matches the order funcs were merged at startup.
    engine.register_fn("list_funcs", move || -> Array {
        func_lib
            .funcs
            .iter()
            .map(|f| Dynamic::from(f.name.clone()))
            .collect()
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

/// Run a single request end-to-end: resolve its session, publish the
/// origin into shared state so the `on_print` hook can tag it, evaluate
/// the script, drain the accumulated stdout, and return the reply.
fn run_script(
    engine: &Engine,
    sessions: &mut SessionStore,
    state: &Arc<Mutex<RequestState>>,
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

    state.lock().unwrap().origin = req.origin.clone();

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
    let print = std::mem::take(&mut state.lock().unwrap().stdout);

    ScriptResult {
        session: Some(session_id),
        print,
        result,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn list_funcs_returns_loaded_func_names_in_insertion_order() {
        use scenarium::function::{Func, FuncId};

        let mut lib = FuncLib::default();
        lib.add(Func {
            id: FuncId::unique(),
            name: "alpha".to_string(),
            ..Default::default()
        });
        lib.add(Func {
            id: FuncId::unique(),
            name: "beta".to_string(),
            ..Default::default()
        });

        let state = Arc::new(Mutex::new(RequestState::default()));
        let (tx, _rx) = mpsc::unbounded_channel::<ScriptAction>();
        let engine = build_engine(state, tx, Arc::new(lib));

        let result: Array = engine.eval("list_funcs()").unwrap();
        let names: Vec<String> = result
            .into_iter()
            .map(|d| d.into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn list_funcs_is_empty_when_func_lib_is_empty() {
        let state = Arc::new(Mutex::new(RequestState::default()));
        let (tx, _rx) = mpsc::unbounded_channel::<ScriptAction>();
        let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

        let result: Array = engine.eval("list_funcs()").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn build_transports_empty_when_no_tcp_config() {
        // Guards against accidentally re-enabling an always-on listener.
        let cfg = ScriptConfig::default();
        assert!(cfg.tcp.is_none());
        let results = build_transports(&cfg);
        assert!(results.is_empty());
    }

    #[test]
    fn build_transports_returns_started_tcp_with_report() {
        let token = Uuid::new_v4();
        let cfg = ScriptConfig {
            tcp: Some(TcpScriptConfig {
                bind: SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0),
                token: Some(token),
                token_file: None,
            }),
        };
        let mut results = build_transports(&cfg);
        assert_eq!(results.len(), 1);
        let started = results
            .remove(0)
            .expect("bind should succeed on loopback :0");
        let TransportReport::Tcp(report) = &started.report;
        assert_eq!(report.token, Some(token));
        assert!(report.addr.ip().is_loopback());
        assert_ne!(report.addr.port(), 0, "OS should have assigned a real port");
        assert!(report.token_file.is_none());
    }

    #[test]
    fn build_transports_surfaces_bind_failure() {
        // Bind once to pin the port, then ask for the same port again so
        // the second bind fails. Loopback :0 lets the OS pick; we read
        // the port off the first listener and reuse it.
        let first = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken = first.local_addr().unwrap();

        let cfg = ScriptConfig {
            tcp: Some(TcpScriptConfig {
                bind: taken,
                token: None,
                token_file: None,
            }),
        };
        let mut results = build_transports(&cfg);
        assert_eq!(results.len(), 1);
        let err = results
            .remove(0)
            .expect_err("port already taken should fail");
        assert_eq!(err.kind, TransportKind::Tcp);
        // AddrInUse on Linux/mac; pin the kind so a regression that
        // silently swallows the error (e.g. None on bind failure) trips.
        assert_eq!(err.error.kind(), std::io::ErrorKind::AddrInUse);
    }
}
