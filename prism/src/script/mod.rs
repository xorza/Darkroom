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
//! `SessionInbound` channel: functions registered on the engine (like
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

use egui::Pos2;
use rhai::{Array, Dynamic, Engine};
use scenarium::function::FuncId;
use scenarium::prelude::FuncLib;

use crate::model::Intent;
use crate::model::ViewNode;
use crate::ui_host::UiHost;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, yield_now};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod session;
pub mod tcp;
#[cfg(test)]
mod tests;

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

/// Convenience wrapper around [`build_transports`] for production
/// callers: bind every enabled transport, announce each success on
/// stdout, log each failure via tracing, and return the bound
/// transports ready to hand to [`ScriptExecutor::new`]. Use
/// [`build_transports`] directly when you need raw outcomes (tests,
/// custom surfacing).
pub fn start_transports(cfg: &ScriptConfig) -> Vec<Box<dyn ScriptTransport>> {
    build_transports(cfg)
        .into_iter()
        .filter_map(|result| match result {
            Ok(started) => {
                announce(&started.report);
                Some(started.transport)
            }
            Err(e) => {
                tracing::error!(
                    kind = ?e.kind,
                    error = %e.error,
                    "script transport disabled (bind failed)",
                );
                None
            }
        })
        .collect()
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

/// Inbound signals from the script executor to [`crate::session::Session`].
/// Each registered script function pushes a variant here; Session drains
/// the queue every frame. Kept separate from the request/reply channel so
/// the executor can complete a script without round-tripping through
/// Session for every side effect.
#[derive(Debug)]
pub enum SessionInbound {
    /// A `print(msg)` call from a script. Lands on Session's status log.
    /// Per-peer attribution is the transport's job (TCP traces peer addr
    /// at the tracing layer); the status bar just sees the message.
    Print { msg: String },
    /// A batch of graph mutations issued by a script. Built on the
    /// executor side (which has `Arc<FuncLib>` and any other inputs the
    /// actions need) and applied verbatim by Session through the same
    /// commit path the GUI uses — same undo stack as a single step, same
    /// dirty-tracking, same autorun re-execution. Keeps the script→graph
    /// boundary symmetric with the GUI→graph boundary, with no
    /// Session-side per-variant glue. Empty vecs are no-ops.
    Apply(Vec<Intent>),
    /// Trigger a single graph evaluation: the worker runs every
    /// terminal node once. Routed through Session's `pending_run_cmd`
    /// so handle_output's existing dirty-then-run sequencing applies.
    RunOnce,
    /// Start the worker's event loop (autorun). Subsequent graph
    /// mutations re-trigger evaluation automatically until stopped.
    StartAutorun,
    /// Stop the worker's event loop.
    StopAutorun,
}

/// `mpsc::UnboundedSender<SessionInbound>` + a host-side ping. Every
/// script side-effect needs to wake the consumer's event loop so
/// `Session::frame` runs and drains the inbound queue — without this,
/// a script that fires actions on a quiet GUI canvas (no mouse, no
/// keyboard) sees them pile up unnoticed until the next user input.
/// Mirrors the worker path, which pings the same host after every
/// result.
#[derive(Clone)]
struct InboundSender {
    tx: mpsc::UnboundedSender<SessionInbound>,
    ui_host: Arc<dyn UiHost>,
}

impl std::fmt::Debug for InboundSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboundSender").finish_non_exhaustive()
    }
}

impl InboundSender {
    fn send(&self, inbound: SessionInbound) {
        // Channel send only fails if the receiver is dropped, which
        // happens during shutdown — Session is going away, no need to
        // ping it for a redraw it'll never serve.
        if self.tx.send(inbound).is_ok() {
            self.ui_host.request_redraw();
        }
    }
}

/// Buffer accumulating the in-flight script's `print(...)` output. The
/// executor runs one script at a time on a single tokio task, so there's
/// no real contention — the `Mutex` exists because Rhai's `on_print` hook
/// requires a `'static + Sync` callback (sync-mode Rhai). Drained by
/// `run_script` after each request and shipped as `ScriptResult.print`.
type StdoutBuffer = Arc<Mutex<String>>;

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
        action_tx: mpsc::UnboundedSender<SessionInbound>,
        func_lib: Arc<FuncLib>,
        ui_host: Arc<dyn UiHost>,
    ) -> Self
    where
        I: IntoIterator<Item = Box<dyn ScriptTransport>>,
    {
        let inbound = InboundSender {
            tx: action_tx,
            ui_host: ui_host.clone(),
        };
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
            tokio::spawn(
                async move { run_executor(rx, cancel_task, inbound, func_lib, ui_host).await },
            );

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
    inbound: InboundSender,
    func_lib: Arc<FuncLib>,
    ui_host: Arc<dyn UiHost>,
) {
    let state: StdoutBuffer = Arc::new(Mutex::new(String::new()));
    let engine = build_engine(state.clone(), inbound, func_lib, ui_host);
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
    stdout: StdoutBuffer,
    inbound: InboundSender,
    func_lib: Arc<FuncLib>,
    ui_host: Arc<dyn UiHost>,
) -> Engine {
    let mut engine = Engine::new();
    configure_caps(&mut engine);
    wire_print_hook(&mut engine, stdout, inbound.clone());
    register_run(&mut engine, inbound.clone());
    register_shutdown(&mut engine, ui_host);
    register_mutations(&mut engine, inbound.clone());
    register_introspection(&mut engine, func_lib.clone());
    register_host_helpers(&mut engine, inbound, func_lib);
    wire_debug_hook(&mut engine);
    install_prelude(&mut engine);
    engine
}

/// Resource caps. None of these are individually load-bearing for
/// correctness — they bound a runaway script's blast radius (CPU,
/// memory, recursion) to something the host can absorb.
fn configure_caps(engine: &mut Engine) {
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_string_size(MAX_STRING_SIZE);
    engine.set_max_array_size(MAX_ARRAY_LEN);
    engine.set_max_map_size(MAX_MAP_LEN);
    engine.set_max_variables(MAX_VARIABLES);
    engine.set_max_strings_interned(MAX_STRINGS_INTERNED);
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_expr_depths(MAX_EXPR_DEPTH, MAX_FN_EXPR_DEPTH);
}

/// Dual-sink `print`: append to the caller's reply buffer AND notify
/// Session for the local status bar. Hook fires synchronously during
/// the script run, so the buffer is guaranteed to still describe the
/// active request when `run_script` drains it.
fn wire_print_hook(engine: &mut Engine, stdout: StdoutBuffer, inbound: InboundSender) {
    engine.on_print(move |msg| {
        {
            let mut buf = stdout.lock().unwrap();
            buf.push_str(msg);
            buf.push('\n');
        }
        inbound.send(SessionInbound::Print {
            msg: msg.to_string(),
        });
    });
}

/// Decode a `Intent` from a Rhai `Dynamic` with numeric
/// coercion. Routes through `serde_json::Value` as the intermediate
/// because its `Deserializer` impl is lenient about widths — `f64 →
/// f32`, `i64 → i32`, etc. all narrow silently. Rhai's own
/// `from_dynamic` is strict (`expecting f32, got f64`), which is the
/// safer default but inconvenient when the host type is f32 (e.g.
/// `egui::Pos2`). One small bridge here keeps the rest of the model
/// free of `#[serde(with = …)]` annotations.
fn decode_action(d: &Dynamic) -> Result<Intent, String> {
    let json = serde_json::to_value(d).map_err(|e| format!("encode to JSON: {e}"))?;
    serde_json::from_value(json).map_err(|e| format!("decode Intent: {e}"))
}

/// `run()` — worker run-state primitive: trigger one evaluation.
/// Bypasses the undo stack; Session routes it to
/// `WorkerMessage::ExecuteTerminals` after applying any pending
/// intents (so the worker sees the latest graph before evaluating).
/// The autorun on/off pair lives in the `host` module instead, since
/// `start_autorun()` / `stop_autorun()` (prelude) are the names users
/// should reach for; `host::set_autorun(bool)` is the underlying
/// primitive.
fn register_run(engine: &mut Engine, inbound: InboundSender) {
    engine.register_fn("run", move || {
        inbound.send(SessionInbound::RunOnce);
    });
}

/// `shutdown()` — ask the host to close. Hooked directly into the
/// frontend's close path (egui's `ViewportCommand::Close`, headless's
/// exit flag) rather than routing through `SessionInbound`, so it
/// works even in modes whose loop doesn't drain the inbound queue.
/// The script that called it still completes and replies normally;
/// the actual teardown happens after the executor task is cancelled
/// during host shutdown.
fn register_shutdown(engine: &mut Engine, ui_host: Arc<dyn UiHost>) {
    engine.register_fn("shutdown", move || {
        ui_host.close_app();
    });
}

/// `apply(action)` / `apply_all(actions)` — the generic mutation
/// surface. Every `Intent` variant is reachable through these
/// via `serde::Deserialize`; new variants light up automatically with
/// no per-variant glue. `apply_all` ships everything in a single
/// `SessionInbound::Apply` so the batch is one undo step.
fn register_mutations(engine: &mut Engine, inbound: InboundSender) {
    {
        let inbound = inbound.clone();
        engine.register_fn(
            "apply",
            move |action: Dynamic| -> Result<(), Box<rhai::EvalAltResult>> {
                let action = decode_action(&action).map_err(|e| format!("apply: {e}"))?;
                inbound.send(SessionInbound::Apply(vec![action]));
                Ok(())
            },
        );
    }
    engine.register_fn(
        "apply_all",
        move |actions: Array| -> Result<(), Box<rhai::EvalAltResult>> {
            let actions: Vec<Intent> = actions
                .into_iter()
                .enumerate()
                .map(|(i, d)| decode_action(&d).map_err(|e| format!("apply_all[{i}]: {e}")))
                .collect::<Result<_, _>>()?;
            inbound.send(SessionInbound::Apply(actions));
            Ok(())
        },
    );
}

/// `list_funcs()` → array of object-maps mirroring `Func`'s Serialize
/// derive (id, name, category, inputs, outputs, …; `lambda` is
/// `#[serde(skip)]`). Lets scripts query the live FuncLib without a
/// separate registry on this side.
fn register_introspection(engine: &mut Engine, func_lib: Arc<FuncLib>) {
    engine.register_fn("list_funcs", move || -> Array {
        func_lib
            .funcs
            .iter()
            .map(|f| {
                rhai::serde::to_dynamic(f).expect("Func is Serialize-clean (no skipped errors)")
            })
            .collect()
    });
}

/// Narrow native primitives that the prelude wraps in friendlier
/// names. Two flavors today:
///
/// - `make_add_node` — builds a `GraphUiAction::AddNode` map (lookup
///   into FuncLib + `From<&Func> for Node`). Wrapped by `create_node`
///   in `prelude.rhai`.
/// - `set_autorun(bool)` — flips the worker event loop on or off.
///   Wrapped by `start_autorun` / `stop_autorun` in `prelude.rhai`.
///
/// Both are registered inside a static `host` module so callers have
/// to use `host::name(...)` — visually marked as internal and kept
/// off the bare-name surface autocomplete sees first. Keep this module
/// small: prefer adding script-side helpers in `prelude.rhai` when a
/// thing can be expressed via `apply`, `run`, and primitive args.
fn register_host_helpers(engine: &mut Engine, inbound: InboundSender, func_lib: Arc<FuncLib>) {
    let mut module = rhai::Module::new();
    module.set_native_fn(
        "make_add_node",
        move |id: &str,
              x: rhai::FLOAT,
              y: rhai::FLOAT|
              -> Result<Dynamic, Box<rhai::EvalAltResult>> {
            let func_id: FuncId = id
                .parse()
                .map_err(|e| format!("invalid func id {id:?}: {e}"))?;
            let func = func_lib
                .by_id(&func_id)
                .ok_or_else(|| format!("unknown func id: {id}"))?;
            let node: scenarium::graph::Node = func.into();
            let view_node = ViewNode {
                id: node.id,
                pos: Pos2::new(x as f32, y as f32),
            };
            let action = Intent::AddNode { view_node, node };
            rhai::serde::to_dynamic(&action)
                .map_err(|e| format!("make_add_node: encode failed: {e}").into())
        },
    );
    module.set_native_fn(
        "set_autorun",
        move |on: bool| -> Result<(), Box<rhai::EvalAltResult>> {
            inbound.send(if on {
                SessionInbound::StartAutorun
            } else {
                SessionInbound::StopAutorun
            });
            Ok(())
        },
    );
    engine.register_static_module("host", rhai::Shared::new(module));
}

fn wire_debug_hook(engine: &mut Engine) {
    engine.on_debug(|msg, src, pos| {
        tracing::debug!(
            target: "prism::script",
            src = src.unwrap_or("<script>"),
            line = pos.line().unwrap_or(0),
            col = pos.position().unwrap_or(0),
            "{msg}"
        );
    });
}

/// Compile [`prelude.rhai`] once and register the resulting functions
/// as a global module. Every helper defined there (`create_node`,
/// `connect`, `move_node`, …) becomes callable from any user script.
/// Adding a new ergonomic helper is a one-function edit to that file —
/// no Rust changes — provided the action can be built from `apply` and
/// existing host helpers.
fn install_prelude(engine: &mut Engine) {
    let ast = engine
        .compile(include_str!("prelude.rhai"))
        .expect("prelude.rhai must parse");
    let module = rhai::Module::eval_ast_as_new(rhai::Scope::new(), &ast, engine)
        .expect("prelude.rhai must evaluate without side effects");
    engine.register_global_module(rhai::Shared::new(module));
}

/// Run a single request end-to-end: resolve its session, publish the
/// origin into shared state so the `on_print` hook can tag it, evaluate
/// the script, drain the accumulated stdout, and return the reply.
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
