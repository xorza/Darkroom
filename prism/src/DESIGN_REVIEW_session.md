# Design review: prism/src/session.rs  (2026-04-23)

## Current design

`Session` is the per-app "bundle" owning: a pure `SessionState` subset (project data: `func_lib`, `view_graph`, `execution_stats`, `argument_values_cache`, `status`, `config`, `autorun`), a `Worker` handle, three worker→session channels (`execution_stats_rx`, `argument_values_rx`, `print_out_rx`), an `ActionUndoStack`, a `graph_dirty: bool` sync flag, a cloned `UiContext` for `request_redraw`, and (newly) a `ScriptExecutor` held solely for `Drop`. The `SessionState` vs `Session` split exists so unit tests can exercise pure action-application logic without building a worker or touching tokio (`session.rs:60–75`, test module at `session.rs:454+`). `SessionState` is `pub` with all fields `pub`, and `Session` re-exposes it via `pub state: SessionState` (`session.rs:115`).

The per-frame control flow is: `MainUi::render` calls `session.update_shared_status()` (polls the three channels, updates `autorun` from `worker.is_event_loop_started()`, writes into `state.status` / `state.execution_stats` / `state.argument_values_cache`), then renders with `&mut session` threaded into `graph_ctx`, then calls `session.handle_output(frame_output)` which applies queued `GraphUiAction`s via `state.apply_actions`, records them to the undo stack, flips `graph_dirty`, and sends a batched set of `WorkerMessage`s. The `graph_dirty: bool` invariant is: "there are unsent graph changes for the worker." It is set in `replace_graph` and `refresh_graph`, cleared only at the one `WorkerMessage::Update` send site in `handle_output` (`session.rs:348`).

Load-bearing decisions: (1) action application is idempotent and the undo stack coalesces per-gesture via `gesture_key`, so `handle_actions` has no pending-action state and is safe under egui's multi-pass rendering (comment at `session.rs:421–428`); (2) worker results are pushed via callback, then consumed on the next frame via `Slot::take` ("latest wins") and mpsc `try_recv` ("every message"); (3) UI-visible `autorun` is a cached mirror of worker state, refreshed once per frame.

## Overall take

The core shape is sound: the idempotent-action pipeline and worker-via-channels pattern are both well-matched to egui's multi-pass rendering, and the two-tier `SessionState` / `Session` split has a real testability motivation. But the tier boundary is leaky enough that it arguably isn't doing the work it's supposed to — every external caller reaches straight through `pub state` — and the worker-event plumbing has accrued three parallel receiver types where one would do. The new `ScriptExecutor` field is load-bearing for nothing except Drop, and lives on `Session` largely because that's where `#[tokio::main]`'s context can reach it.

## Findings

### [F1] `SessionState` vs `Session` tier is leaky; `pub state` makes the encapsulation theatre

- **Category**: Abstraction
- **Impact**: 4/5 — every change to a `SessionState` field can silently affect 33+ external read/write sites, with no compile-time boundary that catches an accidental mutation
- **Effort**: 3/5 — within-module refactor + touch-up of ~5 call sites in `main_ui.rs` / `graph_ui/mod.rs`
- **Current**: `SessionState` is `#[derive(Debug, Default)] pub struct` with every field `pub`, and `Session` re-exposes it via `pub state: SessionState` (`session.rs:67–75`, `:115`). Callers reach in for both reads (`session.state.config.current_path`, `session.state.status`, `session.state.autorun`) and *writes*: `graph_ui/mod.rs:106` takes `&mut session.state.argument_values_cache` and threads it through the entire UI render as a mutable borrow via `GraphContext::argument_values_cache`, used by `node_details_ui.rs:93` (`get_mut`). That's 33+ call sites touching the innards.
- **Problem**: The split's stated goal is unit-testability (comment at `session.rs:60–65` — "No worker, no tokio, no async channels"), and the tests do construct `SessionState::default()` and exercise `apply_actions`. But the production encapsulation is gone: the `pub`→`pub`→`pub` chain means `Session`'s boundary is pure convention. A reviewer trying to understand what state `MainUi` depends on has to grep 33 fields, not read a method list. Worse, the `&mut session.state.argument_values_cache` pattern means "Session" doesn't own its own cache — arbitrary render code mutates it mid-frame.
- **Alternative(s)**:
  - **(a) Delete `SessionState` as a public type.** Inline its fields into `Session`; expose a `pub(crate) fn test() -> Self` gated on `#[cfg(test)]` that builds a `Session` without a real `Worker` (stub `Worker` trait or a `Worker::stub()` constructor). Tests keep their pure-data shape; external callers get method accessors. Highest encapsulation, biggest touchup.
  - **(b) Keep the split but demote visibility.** `pub(crate) struct SessionState { pub(crate) … }`, drop `pub state` for `state: SessionState`, add accessor methods: `session.view_graph()`, `session.status()`, `session.argument_values_cache_mut()`. Tests use `pub(crate)`. Medium effort, still explicit coupling surface.
  - **(c) Keep `SessionState` public but make it a *read-only* handle.** Expose via `pub fn state(&self) -> &SessionState` instead of `pub state`. Every mutation routes through `Session` methods. Breaks the `&mut argument_values_cache` pattern — you'd need a dedicated `Session::argument_values_cache_mut()` or move cache ownership to a per-frame context built by `MainUi`.
- **Recommendation**: Do (b). It keeps the tests unchanged and the migration is mechanical, while giving the module a real boundary. Reconsider (a) only if the `Worker` stub becomes easy (e.g. the worker is later split into a trait for other reasons).

### [F2] Three heterogeneous worker→session receivers for one logical inbox

- **Category**: Data structures
- **Impact**: 3/5 — removes three polling paths + inconsistent disconnect handling; single extension point for new worker events
- **Effort**: 3/5 — touches `Worker` callback/send surface plus `update_shared_status`
- **Current**: `execution_stats_rx: Slot<Result<ExecutionStats, execution_graph::Error>>`, `argument_values_rx: Slot<(NodeId, Option<ArgumentValues>)>`, `print_out_rx: UnboundedReceiver<String>` (`session.rs:119–121`). `update_shared_status` (`session.rs:231–279`) polls all three with different semantics: Slot `.take()` for stats+args (latest-wins), mpsc `try_recv` with explicit `Disconnected` → `panic!` (`session.rs:239`) for print. Adding a new worker-emitted signal means adding a fourth field and a fourth polling branch.
- **Problem**: Three parallel types performing the same role — "things the worker tells Session." The API surface for adding a worker event is larger than it needs to be, and the mismatch between `Slot` (latest-wins) and mpsc (every-message) forces the session to know the "how" of each signal, not just the "what."
- **Alternative(s)**:
  - **(a) Single `enum WorkerEvent { ExecutionFinished(Result<…>), ArgumentValues(NodeId, Option<…>), Print(String), … }` + one `UnboundedReceiver<WorkerEvent>`.** `update_shared_status` becomes a `while let Ok(ev) = rx.try_recv()` with a match. The "latest wins" semantics of stats/args, if load-bearing, are reconstructed by dropping older variants during the drain (cheap). One polling loop, one disconnect path.
  - **(b) Trait-objectify the receivers: `Vec<Box<dyn WorkerSubscription>>`** — over-engineered for three fixed variants; don't.
- **Recommendation**: (a) **depends on whether latest-wins for `execution_stats` is load-bearing.** If dropping a stale `ExecutionStats` in the drain is fine (it almost certainly is — the newer result invalidates the older cache in the same batch), do it. If some subsystem reads partial intermediate stats before the latest arrives, leave `Slot`. My read is that it's fine to unify.

### [F3] `SessionState::apply_actions` + `Session::handle_actions` — near-duplicate names, different tiers

- **Category**: Types
- **Impact**: 2/5 — readability only, no behavior change
- **Effort**: 1/5 — renames
- **Current**: `SessionState::apply_actions(&[GraphUiAction]) -> bool` (pure apply, `session.rs:96`) is the unit-testable part; `Session::handle_actions(&mut FrameOutput) -> bool` (`session.rs:417`) wraps it with undo recording and `refresh_graph`. `handle_actions` is the one called from `handle_output`; `apply_actions` is internal-plus-tests.
- **Problem**: Two functions called `…_actions` in the same module, one on each tier, differ in side effects but not in name. Readers of `handle_output` at `session.rs:316` (`self.graph_dirty |= self.handle_actions(output);`) can't tell from the call site alone whether undo recording happened here or at the inner `state.apply_actions`.
- **Alternative(s)**:
  - **(a) Rename inner to `SessionState::apply` and outer to `Session::commit_actions`.** The inner is a mutator (apply to state); the outer is a commit point (records history, cues the worker).
  - **(b) Leave names; add a doc cross-reference.** Already partially done (`session.rs:94–95`), but comments rot.
- **Recommendation**: Do (a). Free clarity win.

### [F4] `ScriptExecutor` on `Session` is situational, not semantic

- **Category**: Responsibility
- **Impact**: 2/5 — may become awkward when scripts start emitting GraphUiActions; no immediate cost
- **Effort**: 2/5 — move to `PrismApp` or a new `AppServices` struct
- **Current**: `Session` holds `_script_executor: ScriptExecutor` (`session.rs:131`) whose sole role is `Drop`. Scripts don't yet interact with Session; the current wire is just "listener exists."
- **Problem**: The listener will eventually need to emit actions into Session's pipeline, so the placement looks prescient. But the transport layer doesn't belong inside the thing-that-receives-scripts: `Session` is the model the scripts *target*, not the *router* that decides "network → model." Once the Lua handoff lands, we'll want `ScriptRequest`s to be fed through the same `FrameOutput`/`handle_output` pipeline as the UI, which means a non-`Session` component (e.g. `PrismApp` or a `ScriptRouter`) holds both sides.
- **Alternative(s)**:
  - **(a) Move `ScriptExecutor` back to `PrismApp`**, alongside `session` and `main_ui`. Tokio context is fine — `run_native` is called inside `#[tokio::main]`, so eager construction in `PrismApp::new` works (it did before the lazy-init detour). When the Lua bridge lands, the bridge lives there too, pulling `&mut session` from the same struct.
  - **(b) Leave it.** Cheap and wrong-shape-but-harmless until scripts actually do something.
- **Recommendation**: Depends on the timing of the Lua bridge. If it lands within a few weeks, move to `PrismApp` now to avoid a second migration; if not, leave for a future redesign.

## Considered and rejected

- **Make `graph_dirty: bool` a typed state (`InSync | Dirty`)**. Only one site flips to `false`, the semantics are local, and `bool` with a nearby comment is fine. Effort > impact.
- **Push-based `autorun` instead of per-frame pull from `worker.is_event_loop_started()`.** The current one-frame lag is invisible to the user; the worker's event loop state is already reported via callbacks for more meaningful signals (stats, print). A second subscription just for autorun is overkill.
- **Split `update_shared_status` into per-signal handlers.** Tempting, but the three branches share no state and inlining them is readable enough. The *real* cleanup is F2 (one enum), not decomposition.
