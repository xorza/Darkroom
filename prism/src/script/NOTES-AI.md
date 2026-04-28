# prism scripting subsystem

Rhai-based scripting layer that lets external clients drive the
ViewGraph as if they were the GUI. Same actions, same undo stack, same
autorun loop — just a different input source.

## Files

- `mod.rs` — top-level types + executor wiring. Defines `ScriptConfig`,
  `ScriptRequest`/`ScriptResult` (per-call wire shape), `SessionInbound`
  (executor → Session channel), `ScriptExecutor` (owns the executor
  task), and `build_engine` (composed of `configure_caps`,
  `wire_print_hook`, `register_mutations`, `register_introspection`,
  `register_host_helpers`, `wire_debug_hook`, `install_prelude`).
- `prelude.rhai` — built-in Rhai helpers loaded into every engine
  (`create_node`, `connect`, `disconnect`, `move_node`, `select_node`,
  `rename_node`). Edit here to add a new ergonomic helper.
- `session.rs` — `SessionStore`: per-client Rhai scope keyed by UUID,
  with idle-timeout reaping and a hard cap. Lets a TCP client resume
  the same scope across reconnects.
- `tcp.rs` — TCP transport implementation: bind, auth handshake, frame
  loop, discovery file. Implements `ScriptTransport` from `mod.rs`.
- `tests.rs` — unit tests for the engine surface.

## Pipeline

```
CLI flags ──► ScriptConfig
                 │
                 ▼
   Session::new builds:
     • Arc<FuncLib>                   (canonical func registry)
     • mpsc<SessionInbound>           (executor → Session)
     • Notify = Arc<Fn()>             (wakeup; wraps ui_host.request_redraw)
     • build_transports(cfg)          (binds eagerly, reports errors)
     • ScriptExecutor::new(transports, tx, func_lib, notify)
                 │
                 ▼
   Per transport: accept loop pushes ScriptRequest into a bounded mpsc
                 │
                 ▼
   run_executor:  loop { select cancel | rx.recv() } → run_script
                 │
                 ▼
   build_engine:  configure_caps, wire_print_hook, register_mutations,
                  register_introspection, register_host_helpers,
                  wire_debug_hook, install_prelude
                 │
                 ▼
   InboundSender (channel + Notify) — every script side-effect calls
   `inbound.send(...)`, which pushes on the channel AND fires Notify
   so the consumer's event loop wakes promptly
                 │
                 ▼
   Session::drain_inbound (each frame):
     • Print { msg }      → status log
     • Apply(Vec<action>) → commit_action_slice (single undo step)
                 │
                 ▼
   commit_action_slice — the one mutation site, shared with the GUI's
   FrameOutput path. ViewGraph mutated, undo recorded, dirty flagged.
```

## Design decisions

- **Transport-agnostic executor.** `ScriptTransport` trait + bounded
  request mpsc lets the executor stay a single tokio task regardless
  of how scripts arrive. TCP is the only impl today; stdin / WebSocket /
  test harness can plug in without changes.

- **Per-session Rhai scope** (`SessionStore`). A client opens a session
  by sending a nil session-id; subsequent requests with the same id
  resume the same Rhai `Scope`. Reaped after `SESSION_IDLE_TIMEOUT`,
  capped at `MAX_SESSIONS`. Lets clients hold variables across requests
  without us tracking sockets.

- **Apply(Vec<GraphUiAction>) as the unification primitive.** Scripts
  and the GUI both push the same enum onto Session. `commit_action_slice`
  is the only mutation site; it doesn't know or care who emitted the
  action. New `GraphUiAction` variants are scriptable for free
  (deserialize via `apply()`).

- **Notify as `Arc<dyn Fn()>`.** The script crate doesn't import
  `UiHost` or any frontend type. Session passes
  `move || ui_host.request_redraw()` and the script crate just calls
  it. Tests pass `Arc::new(|| {})`.

- **Generic `apply` + thin host helpers.** The `apply` / `apply_all`
  Rhai functions deserialize any `GraphUiAction` via its Serialize
  derive. Helpers for actions whose payload only the host can build
  (currently just `make_add_node`, since `From<&Func> for Node` lives
  in scenarium and depends on FuncLib) live in Rust; everything else
  lives in `prelude.rhai` as pure script. The split is "Rust helpers
  return action *data*; Rhai helpers compose action *flows*."

- **Sandbox-by-default Rhai + resource caps.** No filesystem / process
  / network access. Caps on operations, string/array/map size, scope
  vars, interned strings, recursion, and AST depth bound a runaway
  script's blast radius.

- **Rhai stays on its default i64/f64.** Rhai's `from_dynamic` is strict
  about numeric widths (decoding f64 → f32 fails with "Output type
  incorrect"). To make script-built action maps decode into types
  containing f32 (`egui::Pos2`) or i32 fields, `decode_action` routes
  through `serde_json::Value` as the intermediary — its `Deserializer`
  impl narrows widths silently. One bridge function in `mod.rs`, no
  per-field annotations on the model side. Encode (`to_dynamic`) is
  always permissive (widening), so it stays direct.

- **Cooperative shutdown.** Dropping `ScriptExecutor` cancels its
  `CancellationToken` and aborts the executor + transport tasks, so
  sockets and discovery files are released before the app returns.

## Adding a new helper

1. **Pure script (preferred):** add a function to `prelude.rhai` that
   composes `apply(...)` from primitive args. Works for any action
   whose payload is buildable from caller-supplied data. Examples:
   `connect`, `disconnect`, `move_node`, `select_node`, `rename_node`.

2. **Host helper (only when needed):** if the action requires data only
   the host has (FuncLib lookups, ViewGraph walks for back-references),
   add a Rust function in `register_host_helpers` that returns the
   action *map* via `rhai::serde::to_dynamic`. Then write the ergonomic
   wrapper in `prelude.rhai` that calls your helper and `apply`s the
   result. Pattern: `make_add_node` + Rhai `create_node`.

## Adding a new transport

Implement `ScriptTransport` (push `ScriptRequest`s into the executor
channel, watch the `CancellationToken`), construct it in
`build_transports`, and add a variant to `TransportReport` /
`TransportKind` for startup reporting. The executor doesn't care.

## Open architectural questions

- `SessionInbound::Print` and `SessionInbound::Apply` are unrelated
  side-effect kinds sharing one channel. A second channel for status
  logs would clarify, but adds plumbing for no concrete benefit yet.
- `apply()` exposes every `GraphUiAction` variant — including UI-only
  ones (`SelectNode`, `MoveNode`, `ChangeZoomPan`). Soft fence is
  what helpers we expose; if stricter is wanted, split the enum into
  `GraphMutation` (script-allowed) and `UiState` (GUI-only).
- The crate could be extracted: only `model::ViewNode` and
  `model::graph_ui_action::GraphUiAction` are pulled from prism. YAGNI
  until there's a second consumer.
