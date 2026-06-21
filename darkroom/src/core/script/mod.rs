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
use rhai::{Array, Dynamic, Engine};
use scenarium::function::FuncId;
use scenarium::graph::{Node, NodeId};
use serde::{Deserialize, Serialize};

use crate::core::document::view_node::ViewNode;
use crate::core::edit::intent::Intent;
use crate::core::func_lib::SharedFuncLib;
use crate::core::wake::Wake;
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
    /// executor side (which has the shared `FuncLib` cell and any other
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
    /// [`palantir::HostHandle::quit`].
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

/// Owns the background executor task plus the TCP transport feeding it.
/// Dropping it cancels everything and aborts the tasks.
#[derive(Debug)]
pub struct ScriptExecutor {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
    _transport: TransportHandle,
}

impl ScriptExecutor {
    /// Spawn the TCP transport's listener task and the executor task that
    /// drains it. Must be called from a tokio runtime context. `action_tx`
    /// is the host-side sender for the side effects script functions emit.
    pub fn new(
        transport: tcp::TcpTransport,
        action_tx: mpsc::UnboundedSender<ScriptMessage>,
        func_lib: SharedFuncLib,
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
            run_executor(rx, cancel_task, inbound, func_lib).await;
        });

        Self {
            cancel,
            task: Some(task),
            _transport: transport,
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
    func_lib: SharedFuncLib,
) {
    let state: StdoutBuffer = Arc::new(Mutex::new(String::new()));
    let engine = build_engine(state.clone(), inbound, func_lib);
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

fn build_engine(stdout: StdoutBuffer, inbound: InboundSender, func_lib: SharedFuncLib) -> Engine {
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

/// Dual-sink `print`: append to the caller's reply buffer AND notify the
/// host (which echoes it). Hook fires synchronously during the script
/// run, so the buffer is guaranteed to still describe the active request
/// when `run_script` drains it.
fn wire_print_hook(engine: &mut Engine, stdout: StdoutBuffer, inbound: InboundSender) {
    engine.on_print(move |msg| {
        {
            let mut buf = stdout.lock().unwrap();
            buf.push_str(msg);
            buf.push('\n');
        }
        inbound.send(ScriptMessage::Print {
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
/// `glam::Vec2`). One small bridge here keeps the rest of the model
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
        inbound.send(ScriptMessage::RunOnce);
    });
}

/// `shutdown()` — ask the host to quit. Pushed through the inbound
/// channel like every other side effect; `App` translates it into
/// [`palantir::HostHandle::quit`].
fn register_shutdown(engine: &mut Engine, inbound: InboundSender) {
    engine.register_fn("shutdown", move || {
        inbound.send(ScriptMessage::Shutdown);
    });
}

/// `apply(action)` / `apply_all(actions)` — the generic mutation
/// surface. Every `Intent` variant is reachable through these
/// via `serde::Deserialize`; new variants light up automatically with
/// no per-variant glue. `apply_all` ships everything in a single
/// `ScriptMessage::Apply` so the batch is one undo step.
fn register_mutations(engine: &mut Engine, inbound: InboundSender) {
    {
        let inbound = inbound.clone();
        engine.register_fn(
            "apply",
            move |action: Dynamic| -> Result<(), Box<rhai::EvalAltResult>> {
                let action = decode_action(&action).map_err(|e| format!("apply: {e}"))?;
                inbound.send(ScriptMessage::Apply(vec![action]));
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
            inbound.send(ScriptMessage::Apply(actions));
            Ok(())
        },
    );
}

/// `list_funcs()` → array of object-maps mirroring `Func`'s Serialize
/// derive (id, name, category, inputs, outputs, …; `lambda` is
/// `#[serde(skip)]`). Lets scripts query the live FuncLib without a
/// separate registry on this side.
fn register_introspection(engine: &mut Engine, func_lib: SharedFuncLib) {
    engine.register_fn("list_funcs", move || -> Array {
        // `load` per call so promote/publish growth is visible to scripts.
        // Skip (don't panic on) any func that fails to serialize, so a bad
        // entry can't take down the executor task mid-eval.
        func_lib
            .load()
            .funcs
            .iter()
            .filter_map(|f| rhai::serde::to_dynamic(f).ok())
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
fn register_host_helpers(engine: &mut Engine, func_lib: SharedFuncLib) {
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
            // `load` per call so a func added since startup still resolves.
            let lib = func_lib.load();
            let func = lib
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
                // Script-created nodes set their inputs explicitly; no
                // default-seeding (that's the interactive-palette path).
                bindings: vec![],
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
            target: "darkroom::script",
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
    runtime: Runtime,
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
    /// `func_lib` is the shared swappable library cell; the executor `load`s
    /// it per call, so promote/publish growth from the GUI is reflected in
    /// running scripts on their next access.
    pub fn start(cfg: &ScriptConfig, func_lib: SharedFuncLib, wake: Wake) -> Option<Self> {
        let tcp_cfg = cfg.tcp.as_ref()?;
        let (transport, report) = match tcp::start(tcp_cfg) {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!(error = %e, "script-tcp: bind failed; scripting disabled");
                return None;
            }
        };
        announce_tcp(&report);
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
        // `ScriptExecutor::new` spawns the executor + transport listener
        // tasks, so it needs an ambient runtime.
        let executor = {
            let _guard = runtime.enter();
            ScriptExecutor::new(transport, tx, func_lib, wake)
        };
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
