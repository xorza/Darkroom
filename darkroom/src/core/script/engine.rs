//! Rhai `Engine` construction: resource caps, the `print`/`run`/`shutdown`/
//! `apply`/`apply_all`/`list_funcs`/host-helper registrations, and the
//! `prelude.rhai` install. [`build_engine`] assembles all of it into the
//! single `Engine` the executor pins for its whole lifetime.

use std::sync::Arc;

use glam::Vec2;
use rhai::{Array, Dynamic, Engine};
use scenarium::FuncId;
use scenarium::Library;
use scenarium::{Func, Node, NodeId};
use serde::Serialize;

use crate::core::document::ItemRef;
use crate::core::edit::intent::types::Intent;

use super::{InboundSender, ScriptMessage, StdoutBuffer};

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

pub(crate) fn build_engine(
    stdout: StdoutBuffer,
    inbound: InboundSender,
    library: Arc<Library>,
) -> Engine {
    let mut engine = Engine::new();
    configure_caps(&mut engine);
    wire_print_hook(&mut engine, stdout, inbound.clone());
    register_run(&mut engine, inbound.clone());
    register_shutdown(&mut engine, inbound.clone());
    register_mutations(&mut engine, inbound);
    register_introspection(&mut engine, library.clone());
    register_host_helpers(&mut engine, library);
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
/// [`aperture::HostHandle::quit`].
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

#[derive(Debug, Serialize)]
struct ScriptFuncEvent<'a> {
    name: &'a str,
}

#[derive(Debug, Serialize)]
struct ScriptFunc<'a> {
    id: scenarium::FuncId,
    name: &'a str,
    category: &'a str,
    sink: bool,
    uncacheable: bool,
    default_cache_mode: scenarium::CacheMode,
    behavior: scenarium::FuncBehavior,
    version: u64,
    description: &'a Option<String>,
    inputs: &'a [scenarium::FuncInput],
    outputs: &'a [scenarium::FuncOutput],
    events: Vec<ScriptFuncEvent<'a>>,
}

impl<'a> From<&'a Func> for ScriptFunc<'a> {
    fn from(func: &'a Func) -> Self {
        Self {
            id: func.id,
            name: &func.name,
            category: &func.category,
            sink: func.sink,
            uncacheable: func.uncacheable,
            default_cache_mode: func.default_cache_mode,
            behavior: func.behavior,
            version: func.version,
            description: &func.description,
            inputs: &func.inputs,
            outputs: &func.outputs,
            events: func
                .events
                .iter()
                .map(|event| ScriptFuncEvent { name: &event.name })
                .collect(),
        }
    }
}

/// `list_funcs()` → array of script-facing function descriptors.
fn register_introspection(engine: &mut Engine, library: Arc<Library>) {
    engine.register_fn("list_funcs", move || -> Array {
        library
            .funcs
            .iter()
            .map(|func| {
                rhai::serde::to_dynamic(ScriptFunc::from(func))
                    .expect("script function descriptor must serialize")
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
/// - `make_add_node(func_id, x, y)` — looks the func up in `Library` and
///   shapes a node from it (`From<&Func> for Node`), positioned at
///   `(x, y)`. Wrapped by `create_node` in `prelude.rhai`. Func nodes
///   only (`def: None`); subgraph instancing isn't scriptable yet.
/// - `make_move_node(node_id, x, y)` — an `Intent::MoveSelection` for the
///   one node (no pins). Wrapped by `move_node`.
///
/// Registered inside a static `host` module so callers reach them as
/// `host::name(...)` — visually marked as internal and kept off the
/// bare-name surface. Keep this module small: prefer script-side helpers
/// in `prelude.rhai` when a thing can be expressed via `apply` + the
/// already-shaped maps.
fn register_host_helpers(engine: &mut Engine, library: Arc<Library>) {
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
            // The startup snapshot suffices: the func table never changes
            // after assembly.
            let func = library
                .by_id(&func_id)
                .ok_or_else(|| format!("unknown func id: {id}"))?;
            let node: Node = func.into();
            let action = Intent::AddNode {
                pos: Vec2::new(x as f32, y as f32),
                node_id: NodeId::unique(),
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
            let key = ItemRef::Node(node_id);
            let action = Intent::MoveSelection {
                grabbed: key,
                moves: vec![(key, Vec2::new(x as f32, y as f32))],
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
