//! Scripting boundary for darkroom.
//!
//! The executor is transport-agnostic: every transport produces
//! [`ScriptRequest`] values into a shared bounded `tokio::mpsc`
//! queue, and the executor runs each one inside a single
//! `rhai::Engine` pinned to the executor task. Each request gets
//! back a single [`ScriptResult`] on its [`tokio::sync::oneshot`]
//! reply channel.
//!
//! Scripts talk back to the editor via a separate [`SessionInbound`]
//! channel: functions registered on the engine (`print`, `apply`,
//! `run`, …) push messages; [`crate::app::App`] drains them at the top
//! of every frame and applies them through the same intent/undo path
//! the GUI uses. This keeps the executor task off the UI thread.
//!
//! Rhai is sandbox-by-default (no filesystem, process, or network
//! access unless the host opts in), so there's no nil-globals /
//! whitelist-`_ENV` ceremony. We just layer a few resource caps on
//! top: max operations, max string/array/map size — see
//! [`build_engine`].
//!
//! Runtime: a [`ScriptHost`] owns a dedicated tokio runtime (mirroring
//! `WorkerBridge`); the executor loop `await`s the next request (zero
//! CPU when idle), runs it, then yields back to the scheduler.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use glam::Vec2;
use palantir::HostHandle;
use rhai::{Array, Dynamic, Engine};
use scenarium::function::FuncId;
use scenarium::graph::{Node, NodeId};
use scenarium::prelude::FuncLib;
use serde::{Deserialize, Serialize};

use crate::document::view_node::ViewNode;
use crate::edit::intent::Intent;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
/// callers: bind every enabled transport, surface each outcome via
/// tracing, and return the bound transports ready to hand to
/// [`ScriptExecutor::new`]. Use [`build_transports`] directly when you
/// need raw outcomes (tests, custom surfacing).
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

/// Log post-bind outcomes that the transport itself doesn't cover:
/// auth-disabled warning (louder than the `auth=false` kv on tcp.rs's
/// "listening" line) and token-file write success/failure. The bind
/// event is logged inside the transport. Split out from
/// [`build_transports`] so binding stays pure and unit-testable.
pub(crate) fn announce(report: &TransportReport) {
    match report {
        TransportReport::Tcp(r) => announce_tcp(r),
    }
}

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

/// Inbound signals from the script executor to [`crate::app::App`]. Each
/// registered script function pushes a variant here; `App` drains the
/// queue at the top of every frame (woken by the host's `Notify` after
/// each successful send). Kept separate from the request/reply channel so
/// the executor can complete a script without round-tripping through the
/// editor for every side effect.
#[derive(Debug)]
pub enum SessionInbound {
    /// A `print(msg)` call from a script. The TCP client also receives
    /// this text back in `ScriptResult.print`; the local echo (stderr) is
    /// `App`'s job. Per-peer attribution is the transport's (it traces the
    /// peer addr at the tracing layer).
    Print { msg: String },
    /// A batch of graph mutations issued by a script. Built on the
    /// executor side (which has `Arc<FuncLib>` and any other inputs the
    /// actions need) and applied verbatim by the editor through the same
    /// intent/undo path the GUI uses — one batch is one undo entry. Keeps
    /// the script→graph boundary symmetric with the GUI→graph boundary,
    /// with no editor-side per-variant glue. Empty vecs are no-ops.
    Apply(Vec<Intent>),
    /// `run()` — evaluate the graph once. `App` routes it to
    /// [`crate::app::App::run_graph`]. The deprecated editor's autorun
    /// on/off pair has no analogue here: the new editor runs on demand.
    RunOnce,
    /// `shutdown()` — ask the host to quit, via
    /// [`palantir::HostHandle::quit`].
    Shutdown,
}

/// Opaque "wake the consumer" callback fired after every successful
/// send on the inbound channel. The host wires it to whatever its
/// event loop needs (egui's `request_repaint`, headless's `Notify`,
/// the TUI's `Notify`); the script crate doesn't care — from here
/// it's just `Fn()`. Keeps `script/` free of frontend types.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

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
    notify: Notify,
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
        notify: Notify,
    ) -> Self
    where
        I: IntoIterator<Item = Box<dyn ScriptTransport>>,
    {
        let inbound = InboundSender {
            tx: action_tx,
            notify,
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
        let task = tokio::spawn(async move {
            run_executor(rx, cancel_task, inbound, func_lib).await;
        });

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
) {
    let state: StdoutBuffer = Arc::new(Mutex::new(String::new()));
    let engine = build_engine(state.clone(), inbound, func_lib);
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
    }
}

fn build_engine(stdout: StdoutBuffer, inbound: InboundSender, func_lib: Arc<FuncLib>) -> Engine {
    let mut engine = Engine::new();
    configure_caps(&mut engine);
    wire_print_hook(&mut engine, stdout, inbound.clone());
    register_run(&mut engine, inbound.clone());
    register_shutdown(&mut engine, inbound.clone());
    register_mutations(&mut engine, inbound);
    register_introspection(&mut engine, func_lib.clone());
    register_host_helpers(&mut engine, func_lib);
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

/// `run()` — trigger one graph evaluation. Bypasses the undo stack;
/// `App` routes it to `App::run_graph` after applying any pending intents
/// (so the worker sees the latest graph before evaluating).
fn register_run(engine: &mut Engine, inbound: InboundSender) {
    engine.register_fn("run", move || {
        inbound.send(SessionInbound::RunOnce);
    });
}

/// `shutdown()` — ask the host to quit. Pushed through the inbound
/// channel like every other side effect; `App` translates it into
/// [`palantir::HostHandle::quit`].
fn register_shutdown(engine: &mut Engine, inbound: InboundSender) {
    engine.register_fn("shutdown", move || {
        inbound.send(SessionInbound::Shutdown);
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

/// Narrow native primitives that the prelude wraps in friendlier names.
/// Both build a fully-formed [`Intent`] in Rust (where types are checked)
/// and hand it back as a Rhai map — sparing `prelude.rhai` from
/// hand-building nested maps for the variants that carry a [`Vec2`]
/// (whose serde shape the prelude shouldn't have to know):
///
/// - `make_add_node(func_id, x, y)` — looks the func up in `FuncLib` and
///   shapes a node from it (`From<&Func> for Node`), positioned at
///   `(x, y)`. Wrapped by `create_node` in `prelude.rhai`. Func nodes
///   only (`def: None`); subgraph instancing isn't scriptable yet.
/// - `make_move_node(node_id, x, y)` — an `Intent::MoveNodes` for the one
///   node. Wrapped by `move_node`.
///
/// Registered inside a static `host` module so callers reach them as
/// `host::name(...)` — visually marked as internal and kept off the
/// bare-name surface. Keep this module small: prefer script-side helpers
/// in `prelude.rhai` when a thing can be expressed via `apply` + the
/// already-shaped maps.
fn register_host_helpers(engine: &mut Engine, func_lib: Arc<FuncLib>) {
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
            let node: Node = func.into();
            let view_node = ViewNode {
                id: node.id,
                pos: Vec2::new(x as f32, y as f32),
            };
            let action = Intent::AddNode {
                view_node,
                node,
                def: None,
            };
            rhai::serde::to_dynamic(&action)
                .map_err(|e| format!("make_add_node: encode failed: {e}").into())
        },
    );
    module.set_native_fn(
        "make_move_node",
        move |id: &str,
              x: rhai::FLOAT,
              y: rhai::FLOAT|
              -> Result<Dynamic, Box<rhai::EvalAltResult>> {
            let node_id: NodeId = id
                .parse()
                .map_err(|e| format!("invalid node id {id:?}: {e}"))?;
            let action = Intent::MoveNodes {
                grabbed: node_id,
                to: vec![(node_id, Vec2::new(x as f32, y as f32))],
            };
            rhai::serde::to_dynamic(&action)
                .map_err(|e| format!("make_move_node: encode failed: {e}").into())
        },
    );
    engine.register_static_module("host", rhai::Shared::new(module));
}

fn wire_debug_hook(engine: &mut Engine) {
    engine.on_debug(|msg, src, pos| {
        tracing::debug!(
            target: "darkroom-egui::script",
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

/// Default TCP port when `--script-bind` gives only an IP (or isn't set).
const DEFAULT_SCRIPT_PORT: u16 = 34567;

fn default_bind() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_SCRIPT_PORT)
}

/// Owns the scripting runtime: a dedicated tokio runtime (mirroring
/// `WorkerBridge`), the [`ScriptExecutor`] feeding off it, and the
/// receiving end of the executor→editor [`SessionInbound`] channel that
/// [`crate::app::App`] drains each frame. Built only when `--script-tcp`
/// bound at least one listener; dropping it cancels every task and shuts
/// the runtime down.
pub struct ScriptHost {
    // `executor` and `runtime` are RAII holders, never read after
    // construction: dropping `executor` cancels + aborts the executor and
    // transport tasks, then `runtime` (declared last, so dropped last)
    // shuts down the threads they ran on. Held only for that drop order.
    #[allow(dead_code)]
    executor: ScriptExecutor,
    inbound_rx: mpsc::UnboundedReceiver<SessionInbound>,
    #[allow(dead_code)]
    runtime: Runtime,
}

impl std::fmt::Debug for ScriptHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptHost").finish_non_exhaustive()
    }
}

impl ScriptHost {
    /// Bind every transport `cfg` enables, spin up a runtime, and start
    /// the executor on it. Returns `None` when `cfg` enabled nothing or no
    /// transport bound (a failed bind is logged in `start_transports`) —
    /// the normal "scripting off" case, not an error. `func_lib` is a
    /// startup snapshot shared with the executor; later library growth
    /// (promote/publish) isn't reflected in already-running scripts.
    pub fn start(cfg: &ScriptConfig, func_lib: Arc<FuncLib>, host: HostHandle) -> Option<Self> {
        let transports = start_transports(cfg);
        if transports.is_empty() {
            return None;
        }
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!(error = %e, "failed to build script runtime; scripting disabled");
                return None;
            }
        };
        let (tx, inbound_rx) = mpsc::unbounded_channel();
        let notify: Notify = {
            let host = host.clone();
            Arc::new(move || host.request_repaint())
        };
        // `ScriptExecutor::new` spawns the executor + transport listener
        // tasks, so it needs an ambient runtime.
        let executor = {
            let _guard = runtime.enter();
            ScriptExecutor::new(transports, tx, func_lib, notify)
        };
        Some(Self {
            executor,
            inbound_rx,
            runtime,
        })
    }

    /// Non-blocking drain of everything scripts have pushed since the last
    /// frame. `App` applies each message on the UI thread.
    pub fn drain(&mut self) -> Vec<SessionInbound> {
        let mut out = Vec::new();
        while let Ok(inbound) = self.inbound_rx.try_recv() {
            out.push(inbound);
        }
        out
    }
}

/// CLI flags that configure the script TCP listener, flattened into the
/// editor's top-level args in `main.rs`. All optional; with no
/// `--script-tcp` the listener stays off and the editor behaves exactly
/// as before.
#[derive(clap::Args, Debug, Default)]
pub struct ScriptArgs {
    /// Enable the TCP script listener.
    #[arg(long)]
    pub script_tcp: bool,
    /// Bind address: a bare port (`34567` / `:34567`), an IP (uses the
    /// default port), or a full `host:port`. Defaults to `127.0.0.1:34567`.
    /// A non-loopback bind widens exposure and warns at startup.
    #[arg(long, value_name = "ADDR")]
    pub script_bind: Option<String>,
    /// Require this 16-byte UUID auth token from every client.
    #[arg(long, value_name = "UUID", conflicts_with = "script_no_auth")]
    pub script_token: Option<Uuid>,
    /// Accept any client without a handshake (loopback bind still
    /// advised). Mutually exclusive with `--script-token`.
    #[arg(long)]
    pub script_no_auth: bool,
    /// Write a JSON discovery file (`{"port": N, "token": "..."}`) at
    /// startup so a client can find the address + token.
    #[arg(long, value_name = "PATH")]
    pub script_token_file: Option<PathBuf>,
}

impl ScriptArgs {
    /// Resolve these flags into a [`ScriptConfig`]. Returns the
    /// listener-off default unless `--script-tcp` is set. When on with
    /// neither `--script-token` nor `--script-no-auth`, a fresh random
    /// token is minted so the listener defaults to authenticated (surfaced
    /// via the discovery file / startup log).
    pub fn to_config(&self) -> ScriptConfig {
        if !self.script_tcp {
            return ScriptConfig::default();
        }
        let bind = match &self.script_bind {
            Some(spec) => parse_bind_spec(spec).unwrap_or_else(|e| {
                eprintln!("--script-bind: {e}; falling back to {}", default_bind());
                default_bind()
            }),
            None => default_bind(),
        };
        let token = if self.script_no_auth {
            None
        } else {
            Some(self.script_token.unwrap_or_else(Uuid::new_v4))
        };
        ScriptConfig {
            tcp: Some(TcpScriptConfig {
                bind,
                token,
                token_file: self.script_token_file.clone(),
            }),
        }
    }
}

/// Parse a `--script-bind` spec into a `SocketAddr`. Accepts a bare port
/// (`"34567"` / `":34567"`), a bare IP (`"0.0.0.0"`, `"::1"` → default
/// port), or a full socket addr (`"127.0.0.1:8080"`, `"[::1]:8080"`).
fn parse_bind_spec(s: &str) -> Result<SocketAddr, String> {
    const DEFAULT_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    // Bare port: "34567" or ":34567". `strip_prefix` (not
    // `trim_start_matches`) so "::1" isn't collapsed to "1".
    let port_candidate = s.strip_prefix(':').unwrap_or(s);
    if let Ok(port) = port_candidate.parse::<u16>() {
        return Ok(SocketAddr::new(DEFAULT_IP, port));
    }
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_SCRIPT_PORT));
    }
    s.parse::<SocketAddr>()
        .map_err(|e| format!("invalid bind spec {s:?}: {e}"))
}
