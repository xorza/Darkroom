# Prism Refactor Plan

A proposal to restructure the `prism` crate for simpler flow, stronger isolation,
a smaller blast radius per change, and fewer foot-guns. This is an **architectural**
plan; tactical micro-fixes are already tracked in `src/gui/REVIEW.md` and are
referenced — not repeated — below.

The plan is broken into independently-shippable steps. Each step states the
problem it addresses, the target design, the concrete work, how to verify it,
and a rough size/risk estimate. Steps are ordered so earlier ones unblock later
ones; you can stop at any boundary without leaving the codebase in a worse
state.

---

## 1. Current shape (one-paragraph recap)

`ScenariumEditor` owns `AppData` + `MainUi`. `MainUi` runs a three-phase
render: panels (top menu, bottom log, central) → inside `CentralPanel`,
`GraphUi::render` does **content** (layout, grid, connections, nodes), then
**overlays** (buttons, details panel, new-node popup), then **zoom/pan**.
User intent flows as `PortInteractCommand`s and ad-hoc mutations into
`GraphUiInteraction`, which collects `GraphUiAction`s. At the end of the frame
`app_data.handle_interaction()` drains them, pushes them through an
`UndoStack<ViewGraph>`, and (if any action affects computation) refreshes the
`Worker`. See `src/app_data.rs:33-55`, `src/main_ui.rs:31-159`,
`src/gui/graph_ui.rs:87-191`.

## 2. Design principles the refactor should enforce

These are the invariants every step below converges on. If a change violates
any of them it needs justification.

1. **One writer per concern.** `ViewGraph` is mutated only by applying a
   `GraphUiAction`. Nothing else holds a `&mut` path to it during a frame.
2. **Render reads, doesn't mutate.** A function called `render_*` must not
   emit actions or remove nodes. It returns an intent (command/action/event)
   to the orchestrator.
3. **Input is sampled once per frame.** A single `InputSnapshot` replaces the
   scattered `gui.ui().input(|i| …)` calls.
4. **Interaction state is a real state machine.** Transitions go through one
   function; the active variant carries its own data; cancellation is atomic.
5. **Actions are the only persistence seam.** Save/load, undo/redo, and
   network-style replay all use the same `GraphUiAction` stream.
6. **Lifetimes are boring.** No `PhantomData` beyond what the language needs;
   no `Rc<Style>` clones on hot paths; no context struct with five live
   `&mut` fields.

## 2a. Progress audit against principles

Updated after each commit. `grep` counts are taken from `prism/src/gui/`
plus `main_ui.rs` and `app_data.rs` — i.e. everywhere the view layer
lives.

| # | Principle | Status | Remaining debt |
|---|---|---|---|
| 1 | One writer per concern | ✅ **done** | `grep 'view_graph\.\(graph\.\(add\|remove_node\|by_id_mut\)\|view_nodes\.\(add\|by_key_mut\)\|pan\|scale\|selected_node_id\)\s*='` in `gui/` returns **0 hits**. Every `ViewGraph` mutation in the view layer goes through `GraphUiAction::apply`. |
| 2 | Render reads, doesn't mutate | ✅ **done** | `GraphContext.view_graph: &ViewGraph` (not `&mut`). Render paths take `&GraphContext`. The const-bind value editor (previously the last holdout) now uses a per-frame local `StaticValue` draft; the widget mutates the draft and an `InputChanged` action carries the change. The only remaining `&mut GraphContext` is `node_details_ui::show` / `show_content` because of `ctx.argument_values_cache.get_mut` for lazy preview textures — that's UI cache state, not domain. |
| 3 | Input sampled once per frame | ✅ **done** | `grep '\.input(|' prism/src/gui prism/src/main_ui.rs` (excluding widget internals like `text_edit` / `drag_value` / `popup_menu` / `connection_bezier`) returns **0 hits** in the view/interaction layer. Widget internals still call `ui.input` directly — acceptable, they are self-contained. |
| 4 | Real state machine with atomic cancel | ✅ **done** | `Interaction` enum with variant-local data; `cancel()` is one assignment; no remaining `cancel_interaction + stop_drag` pairs. |
| 5 | Actions as the persistence seam | ✅ **done** | Undo/redo goes through `GraphUiAction`; `apply()` is the single mutation site. Save/load uses snapshot serde of `ViewGraph`, which is a different seam by design (not actions-as-changelog). |
| 6 | Boring lifetimes | **partial** | `PhantomData` gone; `Rc<Style>` clone topology simplified but `Rc` itself still in place (Step 8.2). `GraphContext.view_graph` is now `&ViewGraph`; only `&mut ArgumentValuesCache` remains, and only `node_details_ui` actually needs the `mut` (lazy preview cache). Could be split further but at diminishing returns. |

**Deviations from the original plan — on purpose.**

1. **No separate `Intent` enum.** The original Step 4 described a new `Intent`
   type distinct from `GraphUiAction`. In practice `GraphUiAction` +
   `GraphUiInteraction` buffer already behave exactly like an intent
   pipeline, so we kept one concept instead of two. The only place the
   render pass surfaces a typed "intent" is `NodesFrameResult` (removed /
   broken node IDs + preferred port command) — small, localised.
2. **Idempotent `apply()` + `handle_actions` applies (Step 4.0).** Wasn't
   in the original Step 4. Became necessary once we committed to moving
   mutations out of render: `push_current` assumed actions had already
   been applied, so render was the *only* place mutations could happen.
   Flipping the contract (apply in `handle_actions`; idempotent guards so
   replay is safe) is what lets every Step 4.1 migration remove an inline
   write without deadlocking on "who runs `apply`?".
3. **Migration order vs. plan list.** Plan enumerated 6 classes; we
   actually migrated 7 (added node-name editor). The `StaticValueEditor`
   migration remains — it's the sole blocker for principles 1 + 2
   reaching 100%.

**What remains in one sentence.**

Principles 1–5 are at 100% (render no longer mutates `ViewGraph`). What
remains is test coverage (Step 7 — first render-boundary unit tests now
possible because the seam is clean) and the lifetime polish in principle
6 (`Rc<Style>` → `&Style`, Step 8.2).

## 3. Target module layout

```
prism/src/
├── app/              ← was app_data.rs + main_ui.rs orchestration
│   ├── mod.rs
│   ├── state.rs      ← AppData (domain state only: graph, lib, config, cache)
│   ├── worker_link.rs← async channel wiring, stats/previews/prints
│   └── session.rs    ← save/load/exit/shortcut routing, owns UndoStack
├── model/            ← unchanged surface, slimmed internals
│   ├── view_graph.rs
│   ├── view_node.rs
│   ├── action.rs     ← was graph_ui_action.rs; pure data + apply/undo
│   └── argument_values_cache.rs
├── input/            ← NEW: pre-parsed per-frame input
│   ├── snapshot.rs   ← InputSnapshot (keys, pointer, wheel, modifiers)
│   └── shortcuts.rs  ← ShortcutMap (undo/save/run/…), returns Intents
├── interaction/      ← NEW: state machine lifted out of gui/
│   ├── mod.rs        ← Interaction enum (was InteractionMode + data)
│   ├── panning.rs
│   ├── dragging_connection.rs
│   ├── breaking_connections.rs
│   └── idle.rs
├── view/             ← was gui/, renamed to de-emphasize egui
│   ├── mod.rs        ← Gui<'a>, new_child, with_scale
│   ├── graph/        ← graph_ui.rs split (see step 5)
│   │   ├── mod.rs    ← orchestrator only
│   │   ├── layout.rs
│   │   ├── nodes.rs
│   │   ├── connections.rs
│   │   ├── overlays.rs
│   │   └── zoom_pan.rs
│   ├── details.rs
│   ├── new_node.rs
│   ├── log.rs
│   ├── background.rs
│   └── style/…
├── widgets/          ← was common/, renamed; no semantic change
└── main.rs
```

Motivations:
- `app_data` is currently the only place that touches both domain and worker;
  splitting it makes the "pure state" easy to unit test without async.
- `input/` centralizes the `input(|i| …)` hot-spots identified by the audit
  (`new_node_ui.rs:136-141`, repeated shortcut reads in `main_ui.rs:167-229`,
  duplicate scroll reads in `graph_ui.rs:759-778`).
- `interaction/` makes the state machine a first-class module with
  variant-local data, removing the `breaker()` foot-gun (`graph_ui.rs:151`
  passes the breaker even in idle mode).
- `view/` renaming signals that egui is an implementation detail of rendering,
  not of the domain — the boundary tests can mock it.

---

## 4. Staged work plan

Each step is shippable on its own and leaves `cargo nextest run && cargo fmt &&
cargo check && cargo clippy --all-targets -- -D warnings` green.

### Step 1 — Fix the cheap, high-value items already enumerated ✅ DONE

**Problem.** `src/gui/REVIEW.md` listed 18 concrete local cleanups
(duplicated disconnect logic, dead `brighten` / `_scaled_u8`, hardcoded colors
in `node_details_ui`, `PointerButtonState` over-abstraction, repeated shadow
construction, `path_length` recompute, unused `NonNull` import, etc.). These
were risk-free and shrank the surface area for the bigger moves.

**Work done.**
- **F1** — extracted `disconnect_connection(key, ctx, ui_interaction)` in
  `gui/connection_ui.rs`; both the breaker path (`graph_ui.rs`) and the
  double-click path now call it. The breaker path lost ~40 lines.
- **F2, F3** — removed dead `brighten` and `_scaled_u8` from `gui/style.rs`.
- **F4** — `node_details_ui` now reads error / missing-inputs colors from
  `style.node.{errored,missing_inputs}_shadow.color` instead of hard-coded
  RGB values.
- **F5** — deleted `PointerButtonState` and `get_primary_button_state`;
  `process_connections` reads the boolean inline.
- **F6** — `GraphBackgroundRenderer::rebuild_texture` no longer takes the
  unused `_ctx` parameter.
- **F7** — extracted `update_curve_interaction(...)` in `connection_ui.rs`;
  the data- and event-connection render loops now share it.
- **F8** — `ConnectionBreaker` maintains `cached_length: f32`, updated in
  `add_point` / reset in `reset`; removed the O(n) `path_length()` recompute.
- **F9** — `Style::new` uses a local `status_shadow(color)` helper; the four
  status shadow constructions are one line each.
- **F10** — `render_port_labels` now uses a single closure parameterised by
  `LabelSide` (Right / Left of port); three near-identical loops collapsed.
- **F11** — `LogUi::render` calls `gui.ui()` instead of the private field.
- **F12** — `new_node_ui::show_category_functions` calls `gui.ui()` instead
  of the private field.
- **F13, F15, F16, F18** — `Gui<'a>` dropped the redundant
  `PhantomData<&'a mut Ui>`, collapsed `new` to delegate to `new_with_scale`,
  removed the commented-out `#[derive(Debug)]`, and removed the unused
  `NonNull` import.
- **F14** — `node_details_ui` spacings now come from
  `gui.style.padding` / `small_padding`.
- **F17** — `GraphUiInteraction`'s `errors` / `run_cmd` /
  `request_argument_values` fields are now private; callers use
  `pop_error()`, `run_cmd()`, `set_run_cmd()`, `request_argument_values()`,
  `set_request_argument_values()`.
- `src/gui/REVIEW.md` deleted — every finding was applied, so the file was
  pure history.

**Verification.** `cargo nextest run -p prism` (22 tests pass),
`cargo fmt`, `cargo clippy --all-targets -- -D warnings` — all green.

**Size / risk.** Shipped; ~14 files touched, no behaviour change expected
or observed.

---

### Step 2 — `InputSnapshot`: sample input once per frame ✅ DONE

**Problem.** `gui.ui().input(|i| …)` was called from many call sites
(shortcut handlers in `main_ui.rs`, three sequential closures in
`new_node_ui.rs::should_close_popup`, wheel/pointer reads scattered through
`graph_ui.rs`, plus the now-removed `PointerButtonState` helper). Between
closures egui can deliver new events, so the same logical frame observed
different input — a concrete race risk called out by the audit.

**Work done.**
- Added `prism/src/input/{mod,snapshot}.rs` with
  `InputSnapshot::capture(&egui::Context)`. Single pass over the event
  stream collects: pointer state (hover/interact pos, primary
  pressed/down/released/clicked, secondary_pressed, any_pressed, any_click),
  modifiers, `keys_pressed: Vec<Key>`, combined `scroll_delta` (smooth plus
  any `MouseWheelUnit::Point` events), `wheel_lines` (line/page magnitude),
  and raw `zoom_delta`.
- Helpers on the snapshot: `key_pressed`, `escape_pressed`, `cmd`,
  `cmd_only`, `cmd_shift`, `cancel_requested`, `zoom_delta_unless_cmd`.
- `MainUi::render` now captures the snapshot once at frame start and passes
  it into `handle_shortcuts` and `graph_ui.render`.
- `handle_shortcuts` shrank from five separate `input(|…|)` closures per
  shortcut to straight-line `if input.cmd_only(Key::S) { … }` checks.
- `GraphUi::render` threads `&InputSnapshot` into `process_connections`,
  `update_zoom_and_pan`, `apply_scroll_zoom`, and `handle_new_node_popup`.
- `GraphUi::should_cancel_interaction` removed — call site uses
  `input.cancel_requested()` directly.
- Removed the free function `collect_scroll_mouse_wheel_deltas`; replaced
  with `(input.scroll_delta, input.wheel_lines)`.
- `NewNodeUi::show` + `should_close_popup` take `&InputSnapshot`; the
  three-closure race in `should_close_popup` collapsed to three field
  reads from the same snapshot.
- `PointerButtonState` was already deleted in Step 1; `primary_down` inline
  now reads `input.primary_pressed || input.primary_down`.
- 5 new unit tests in `input::snapshot::tests` covering the cmd/cmd_only/
  cmd_shift boolean matrix, `escape_and_cancel`, and
  `zoom_delta_unless_cmd`.

**Out of scope for this pass.** `common/text_edit.rs`, `common/drag_value.rs`,
`common/popup_menu.rs`, and `common/connection_bezier.rs` still call
`ui.input(|…|)` internally. They're widget-internal and not on the race
paths the audit flagged; migrating them is mechanical follow-up.

**Verification.** 27 tests pass (22 pre-existing + 5 new),
`cargo clippy --all-targets -- -D warnings` green.

**Size / risk.** Shipped; 7 files touched plus the new `input/` module.

---

### Step 3 — Interaction as a real enum-with-data state machine ✅ DONE

**Problem.** `GraphInteractionState` kept `mode: InteractionMode` **plus**
a `connection_breaker: ConnectionBreaker` field that was only meaningful in
`BreakingConnections`, and the in-flight drag lived on `ConnectionUi` under
`temp_connection`. That produced a dual-source-of-truth — "what is the UI
doing?" had to be read from both `interaction.mode()` and
`connections.temp_connection` — and cancellation had to remember to reset
both (`reset_to_idle() + connections.stop_drag()`).

**Work done.**
- Rewrote `gui/interaction_state.rs` as an enum-with-data:
  ```rust
  pub enum Interaction {
      Idle,
      Panning,
      DraggingConnection(ConnectionDrag),
      BreakingConnections(ConnectionBreaker),
  }
  ```
- Deleted `InteractionMode`, `GraphInteractionState`, `breaker_mut`,
  `add_breaker_point`, `transition_to`, `reset_to_idle`,
  `is_breaking_connections`, `is_dragging_connection`. The breaker and
  drag accessors (`breaker`, `breaker_mut`, `drag`, `drag_mut`) now return
  `Option<&T>` / `Option<&mut T>` — variant-local, so `Idle.breaker()` is
  literally `None`.
- `Interaction::cancel()` is one assignment: `*self = Self::Idle`. Every
  call site that used to do `reset_to_idle() + connections.stop_drag()`
  now calls `cancel()` and drops the variant data in one shot.
- Moved the in-flight `ConnectionDrag` out of `ConnectionUi` and into
  `Interaction::DraggingConnection`. `ConnectionUi` is now a stateless
  renderer plus a mesh-buffer cache (`temp_connection_bezier`); the drag
  flow hits the variant via `self.interaction.drag_mut()`.
- Split `ConnectionUi::update_drag` into two pieces: the idle→Dragging
  transition moves into a free function `handle_idle` in `graph_ui.rs`
  that calls `Interaction::start_dragging`; the in-drag advancement is a
  free function `connection_ui::advance_drag(&mut ConnectionDrag, …)`.
- `process_connections` now matches directly on `&mut self.interaction`,
  binding the breaker / drag by reference — impossible to call the wrong
  helper from the wrong variant.
- `render_connections` likewise matches the enum to pick between showing
  the breaker mesh and the temp-connection preview.
- `create_const_binding` reads from `self.interaction.drag()` instead of
  the removed `ConnectionUi.temp_connection` field.
- Tests in `interaction_state` rewritten (5 tests): default idle, start
  variants install only their own data, cancel drops data, cross-variant
  transition replaces data.

**Verification.** 28 tests pass; `cargo clippy --all-targets -- -D warnings`
green.

**Size / risk.** Shipped; 3 files rewritten (`interaction_state.rs`,
`connection_ui.rs`, `graph_ui.rs`). No behaviour change intended or
observed; the dual-source-of-truth problem and the
`reset_to_idle + stop_drag` pair are now impossible by construction.

---

### Step 4 — Render functions become pure; mutations go through actions

Split into two phases. The key insight: render functions mutate `ViewGraph`
today because in-flight UI state (drag delta, text-edit draft, hover) has
nowhere else to live. Move that state into the `Interaction` enum; then
render stops having a reason to mutate the domain.

---

#### Step 4.0 — Foundation: idempotent `apply` + `handle_actions` applies ✅ DONE

The undo stack historically assumed actions were applied inline during
render — `push_current` just recorded them. That made it impossible for a
render function to *defer* a mutation: if it didn't mutate inline, the
mutation never happened. This step fixes the contract.

**Work done.**
- Made `GraphUiAction::apply` idempotent for the three non-idempotent
  variants (`NodeAdded` / `NodeRemoved` / `EventConnectionChanged`). Each
  guards its mutation on "is this the current state?"; replay is a no-op.
- `AppData::handle_actions` now calls `action.apply(&mut view_graph)` for
  every action in the frame's action stacks, before `push_current`. This
  is the single authoritative mutation site. Inline mutations that
  currently exist become redundant no-ops (idempotency makes this safe);
  migrating them off is Step 4.1.
- 3 unit tests for the idempotency contract (the three non-trivial
  variants).

**Cost.** Every emitted action is re-applied once per frame. For
idempotent actions re-applying already-applied state is fast (a lookup and
an equality-type check), but it is not free — the full benefit only
materializes once Step 4.1 removes inline mutations, at which point this
double-work disappears.

---

#### Step 4.1 — Phase A: Interaction state owns in-flight edits

**Problem.** `handle_node_drag` writes `view_node.pos` every frame during
a drag. `StaticValueEditor` hands egui a `&mut StaticValue` that is
mutated on every keystroke. `handle_background_click` / drag code writes
`view_graph.selected_node_id`. These are UI-in-flight states masquerading
as domain state.

**Target.** Each in-flight edit is a variant of `Interaction` with its
own transient buffer:

```rust
pub enum Interaction {
    Idle,
    Panning,
    DraggingConnection(ConnectionDrag),
    BreakingConnections(ConnectionBreaker),
    DraggingNodes { /* node_id → accumulated offset */ },       // NEW
    EditingConstBind { node_id, input_idx, draft: StaticValue }, // NEW
}
```

Render reads `(ViewGraph, Interaction)` together when presenting:
- Node position = `view_node.pos + interaction.drag_offset_for(id)`.
- Const value = `interaction.draft_for(id, idx).unwrap_or(&binding.value)`.

Commit moments (drag release, Enter, focus loss) emit the action; the
action's `apply` mutates `ViewGraph`; `Interaction` returns to `Idle`.
Nothing in the render path needs `&mut ViewGraph` for these paths
anymore.

**Work plan (one commit per class of mutation).**

1. **Node drag** ✅ DONE. `Interaction::DraggingNode(NodeDrag { node_id, start_pos, offset })`.
   `GraphLayout::update` takes `&Interaction`; each `NodeLayout::update`
   composes `view_node.pos + interaction.node_drag_offset_for(id)`. On
   drag start `handle_node_drag` calls `state.start_node_drag`; on each
   frame it accumulates `offset += delta / scale`; on release it emits
   `NodeMoved { before: start_pos, after: start_pos + offset }` and
   cancels. `ViewGraph` is never written during the drag — the committed
   position lands via `NodeMoved::apply`. Deleted the now-unused
   `common::drag_state` module and the `NodeIds::drag_start` id salt.
2. **Selection** ✅ DONE. `handle_background_click`'s inline
   `view_graph.selected_node_id = None` removed; the emitted
   `NodeSelected` action's apply handles it. Combined with the
   node-drag commit which already dropped the inline write in
   `handle_node_drag`, selection is now fully routed through actions.
3. **Zoom / pan** ✅ DONE. Split `update_zoom_and_pan` into three
   ingredients — `drive_pan_interaction_state` (Idle↔Panning
   transitions), `compute_scroll_zoom` (pure `(scale, pan) → (scale,
   pan)`), and middle-drag delta — then emit exactly one
   `ZoomPanChanged` per frame via `emit_zoom_pan`. Buttons for reset /
   view-selected / fit-all became pure target-computing functions
   (`fit_all_nodes_target`, `view_selected_node_target`) whose result
   flows into the same `emit_zoom_pan`. No more inline writes to
   `view_graph.pan` / `scale` anywhere in render.
4. **Const-bind editor** — the big one. Replace `StaticValueEditor`'s
   `&mut StaticValue` with `&mut draft: StaticValue` owned by
   `Interaction::EditingConstBind`. On commit emit `InputChanged`. Also
   the node-name editor in `node_details_ui` — same pattern.
5. **Connection create/delete** ✅ DONE. `apply_data_connection` /
   `apply_event_connection` / `disconnect_connection` now take
   `&ViewGraph` / `&GraphContext`, read the current state, and emit the
   action — no inline binding / subscriber mutation.
6. **Node add/remove** ✅ DONE. `handle_new_node_selection::Func` now
   builds the `Node` + `ViewNode` locally and emits `NodeAdded`; apply
   does the insert. The orchestrator's render-time removal loop and
   `apply_breaker_results`' node path now just emit `NodeRemoved` —
   `apply` does the `remove_node`. Also cleaned up the
   `node_details_ui` name editor's inline `name.clone_from`.

**Verify.** At the end of Step 4.1, `grep "view_graph\.\|view_nodes\." prism/src/gui/` returns nothing inside render functions.
All mutations funnel through `handle_actions`. The per-frame double-work
from 4.0 disappears (inline mutations are gone, so apply does the real
work once).

**Size / risk.** 6 commits, each one self-contained and verifiable. Main
risk is the const-bind editor rewrite — it's the only place that needs a
non-mechanical change. Drag lag concern is moot because the drag offset
lives in `Interaction` and render composes it — no 1-frame lag.

---

#### Step 4.2 — Phase B: Tighten render signatures ✅ DONE (steps 1-2)

**Problem.** Even after 4.1, render took `&mut GraphContext` which
carried `&mut ViewGraph`. Nothing used the `mut`, but the signature
permitted it.

**Work done.**
1. `GraphContext.view_graph: &'a ViewGraph` (dropped the `mut`).
2. Every `render_*` function in `graph_ui.rs` / `node_ui.rs` /
   `connection_ui.rs` that took `&mut GraphContext` now takes
   `&GraphContext`. Only `node_details_ui::show` / `show_content` still
   take `&mut` — needed for `ctx.argument_values_cache.get_mut` (lazy
   preview-texture fill, UI cache state not domain state).

**Out of scope for now.** The cache split (original item 3) gives
diminishing returns; two leaf functions still needing `&mut` is
acceptable. If it proves painful later, split the cache into an
`&mut ArgumentValuesCache` side channel.

See Step 7 for the test additions that made this step pay off.

---

#### Step 4 work done in this pass

- Step 4.0 shipped (idempotent apply + `handle_actions` applies).
- Step 4 narrow: `NodeUi::render_nodes` → `NodesFrameResult`; orchestrator
  applies removals. (Done earlier in this step before the rethink.)
- First 4.1 migration: `const_bind_ui` no longer does inline
  `input.binding = Binding::None`; the emitted `InputChanged` action
  does it via `apply`.

**Verification.** 31 tests pass; `cargo clippy --all-targets -- -D warnings`
green.

---

### Step 5 — Split `graph_ui.rs` along phase boundaries

**Problem.** `src/gui/graph_ui.rs` is ~907 lines mixing orchestration,
layout, rendering of four primitives (background, connections, nodes, ports),
overlay handling, zoom/pan, and disconnection logic. The audit flagged module
bloat plus the fact that zero tests exist for the file.

**Target.** After Step 4 the file is almost entirely orchestration. Split it:

```
view/graph/
├── mod.rs          ~150 lines — the three-phase orchestrator
├── layout.rs       — was ::update / port resolution
├── nodes.rs        — was ::node_ui render + port interaction decoding
├── connections.rs  — drawing + drag preview
├── overlays.rs     — buttons, details panel dispatch, new-node popup dispatch
└── zoom_pan.rs     — was update_zoom_and_pan
```

`mod.rs` only knows the pipeline; each sibling is < 300 lines and tested where
non-egui logic lives (layout math, hit tests, intent decoding).

**Work.** Mechanical move. Do it after Step 4 so what moves is already pure
and easy to reason about.

**Verify.** `cargo clippy --all-targets -- -D warnings` green; diff the
render output on a snapshot test graph (see Step 7).

**Size / risk.** ~1 day. Low after Step 4.

---

### Step 6 — `AppData` splits into `AppState` + `Session`

**Problem.** `AppData` (`src/app_data.rs:33-55`) bundles domain state
(graph, lib, cache, config) with session/runtime concerns (worker, async
channels, dirty flag, print buffer, undo stack). It's 400+ lines and the
unit-testable parts are trapped behind the async parts.

**Target.**
```rust
pub struct AppState {          // pure; Serialize + Clone + testable
    pub view_graph: ViewGraph,
    pub func_lib: FuncLib,
    pub argument_values_cache: ArgumentValuesCache,
    pub execution_stats: Option<ExecutionStats>,
    pub config: Config,
    pub autorun: bool,
    pub status: String,
}

pub struct Session {           // side-effectful; owns the worker + undo
    pub state: AppState,
    worker: Worker,
    execution_rx: Slot<ExecutionStats>,
    argument_rx: Slot<ArgumentValues>,
    print_rx: UnboundedReceiver<String>,
    undo_stack: Box<dyn UndoStack<ViewGraph>>,
    graph_dirty: bool,
}
```
`Session::apply(actions)` is the single entry point for action application:
it pushes to the undo stack, refreshes if any action `affects_computation`,
and drains worker callbacks. `AppState` alone is enough to unit-test
everything the view layer cares about.

**Work.**
1. Extract `AppState` first, leaving `AppData` as a facade.
2. Move worker wiring to `Session`.
3. Delete the facade once all call sites are updated.

**Verify.** Existing behaviour + new tests: apply a sequence of actions to an
`AppState`, assert the resulting `ViewGraph` is what we expect; no egui, no
tokio.

**Size / risk.** ~2 days. Medium.

---

### Step 7 — Deterministic snapshot tests ✅ PARTIAL

**Problem.** The audit's Finding #10: zero tests for any GUI interaction
path. After Steps 2–4.2 there is finally something testable: the helper
functions that sit right at the render → action boundary take `&ViewGraph`
+ port refs / input + drag + commands and return actions or state
transitions. No egui runtime needed.

**Work done in the initial pass (21 new tests, 52 total from 31):**

- `graph_ui.rs` test module:
  - `apply_data_connection` — rejects self-loop, detects transitive
    cycle, produces correct `InputChanged`, round-trips through `apply`
    so the binding lands correctly.
  - `apply_event_connection` — rejects self-loop, no-op when already
    subscribed, produces correct `EventConnectionChanged { Added }`.
  - `handle_idle` — stays idle when primary not down, transitions to
    `DraggingConnection` on `DragStart`, to `BreakingConnections` on
    background click, stays idle on hover-only.
- `connection_ui.rs` test module:
  - `advance_drag` — `None` clears end-port, `Hover` snaps to compatible
    / rejects same-kind, `DragStop` with snap returns `FinishedWith`,
    without snap returns the right `FinishedWithEmpty*` for either drag
    direction.
  - `disconnect_connection` — input variant emits `InputChanged::None`
    (no-op when already `None`), event variant emits
    `EventConnectionChanged::Removed` (no-op when not subscribed).

**What's still unaddressed (Step 7.2 — optional, not yet shipped):**

- `tests/action_apply.rs`: `apply_all` + `undo_all` round-trip for every
  action variant on a realistic fixture. A weaker version already exists
  in `common/undo_stack/action_undo_stack.rs::undo_roundtrip_all_action_variants_with_json_snapshots`
  which does JSON-snapshot comparison. Could be extended to cover the
  new paths (particularly `ZoomPanChanged` + `NodeSelected`).
- `tests/session_worker.rs`: tokio integration test that sends a graph
  update and asserts cache invalidation. Requires exposing more of
  `AppData`'s worker channel — deferred until Step 6.
- True render-boundary tests (feed an egui mock `Ui` into
  `GraphUi::render`, assert emitted actions). Achievable via
  `egui::__run_test_ui` helpers; this is the natural follow-up.

---

### Step 8 — Drop the redundant `Rc<Style>` clones and `PhantomData` in `Gui<'a>` ✅ DONE

**Problem.** `Gui<'a>` cloned `Rc<Style>` twice per child: the parent
bumped it before the closure, then `new_with_scale` bumped it again inside.
Worse, `positioned_ui` and `scroll_area` did `Gui::new(ui, &style)` (scale
1.0) immediately followed by `set_scale(scale)`, which triggered
`Rc::make_mut` and forced a full `Style` re-allocation because the clone
above had bumped refcount ≥ 2. `PhantomData` was already dropped in Step 1.

**Work done.**
- Added a private `Gui::with_style(ui, Rc<Style>, scale)` constructor that
  takes the `Rc<Style>` by value — one bump total.
- `horizontal` / `horizontal_justified` / `vertical` / `new_child` now clone
  the `Rc` once and move it into the child `Gui` via `with_style`, instead
  of the old clone-then-clone-inside pattern.
- `positioned_ui::show` and `scroll_area::show` replaced the
  `Gui::new(ui, &style) + set_scale(scale)` pair with a single
  `Gui::new_with_scale(ui, &style, scale)` call — eliminates the `Style`
  re-allocation the make_mut used to force every frame.
- Public API (`new`, `new_with_scale`, `set_scale`, `with_scale`) unchanged;
  only the internal clone topology moved.

**Kept for Step 8.2 (not done here).** Replacing `Rc<Style>` with a plain
`&'a Style` borrow is still the end-state, but it requires threading a
lifetime through every layout helper and reworking `with_scale` to
construct a fresh scaled `Style` rather than mutating in place. That is
more invasive and belongs after Step 3 (interaction refactor) reduces the
number of call sites that thread `Gui`.

**Verification.** `cargo nextest run -p prism` (22/22),
`cargo clippy --all-targets -- -D warnings` — green.

**Size / risk.** Shipped; 3 files touched.

---

### Step 9 — Replace ad-hoc `.unwrap()`s on lookups with a `GraphView` helper

**Problem.** The audit's Finding #1: `graph.by_id().unwrap()`,
`func_lib.by_id().unwrap()`, `node_layouts.by_key().unwrap()` are scattered
through interaction code. A stale ID (a node deleted between frames) panics
the app.

**Target.** A `GraphView<'a>` wrapper around `&ViewGraph` exposing:
```rust
impl<'a> GraphView<'a> {
    fn node(&self, id: NodeId) -> Option<NodeRef<'a>>;
    fn func(&self, id: FuncId) -> Option<&Func>;
    fn assert_node(&self, id: NodeId) -> NodeRef<'a>;  // panics with context
}
```
Interaction code uses `node(id)?` and early-returns / no-ops on stale IDs;
only the apply-action path uses `assert_node` (where a missing node is a
real logic bug).

**Work.** Thread `GraphView` through the view layer (naturally available
after Step 4 since `GraphContext` already hands out `&ViewGraph`). Convert
unwraps one file at a time.

**Verify.** A stress test that randomly deletes a node each frame and scripts
user interactions — should never panic.

**Size / risk.** ~1 day. Low.

---

## 5. What explicitly stays

- **`GraphUiAction` as the action vocabulary.** It is the right shape; it
  survives every step above. If anything, it gets *more* central.
- **`UndoStack<ViewGraph>` trait + `ActionUndoStack` impl.** Correct design,
  tested, no need to touch.
- **`ViewGraph` / `ViewNode` split from the scenarium core graph.** Good
  separation of "what the user sees" from "what executes". Keep.
- **`bumpalo::Bump` arena per frame.** Fine; reset on the existing boundary.
- **Workspace dependency layout.** `prism` depending on `scenarium`,
  `imaginarium`, `palantir`, `common` is correct.

## 6. What explicitly goes away

| Thing | Why |
|---|---|
| `PointerButtonState` | Subsumed by `InputSnapshot::primary` |
| `GraphContext::view_graph: &mut` | Renders don't mutate (Step 4) |
| `GraphInteractionState::breaker()` accessor | Variant-local data (Step 3) |
| `ConnectionUi::temp_connection` field | Lives in `Interaction::DraggingConnection` |
| `Gui::_marker: PhantomData` | Redundant (Step 8) |
| `Rc<Style>` in `Gui` | `&Style` is enough (Step 8) |
| Direct `.unwrap()` on ID lookups in render/interaction | `GraphView` helper (Step 9) |
| `_scaled_u8` / `brighten` dead code in `style.rs` | Dead (Step 1) |

## 7. Expected payoff, in order of user-visible benefit

1. **Fewer panics on stale state.** Step 9 + Step 3 remove the two known
   classes (ID-after-delete unwraps; breaker accessed in wrong mode).
2. **No input races.** Step 2 collapses the multi-closure reads that the
   audit called out as plausibly bug-producing.
3. **Undo correctness tested.** Step 7 adds round-trip coverage the codebase
   has never had.
4. **Easier contributions.** A developer landing a new interaction only
   touches `interaction/` (new variant) and `view/graph/overlays.rs` (new
   render branch), instead of editing six files and remembering four places
   to cancel.
5. **Faster frames.** Step 8's removal of per-frame `Rc` clones + Step 4's
   removal of redundant port lookups should be measurable on large graphs.

## 8. Order of execution (recommended)

```
Step 1 ✅ ┐
          ├─ Step 2 ✅ ┐
          │            ├─ Step 3 ✅ ┐
          │            │            ├─ Step 4.0 ✅ ─┐
          │            │            │               ├─ Step 4.1 (Interaction) ─┐
          │            │            │               │                            ├─ Step 4.2 ─┐
          │            │            │               │                            │  (signatures) ├─ Step 5 ─┐
          │            │            │               │                            │               │           ├─ Step 7
Step 8 ✅ ┘            │            │               │                            │               └─ Step 6 ─┘
                       │            │               │                            └─ Step 9
                       └─ Step 8.2                   └ — (each 4.1 site is one commit)
```

- Shipped: 1, 2, 3, 8, Step 4 narrow, Step 4.0.
- Next: Step 4.1 migrations, one mutation class per commit. Order:
  node drag → selection → const-bind editor → zoom/pan → connection
  add/delete → node add/remove. The big one (const-bind editor) is the
  only non-mechanical change.
- Step 4.2 (signatures + render tests) is the payoff — clean
  `&ViewGraph` signatures and the first unit tests for render behaviour.
- Step 8.2 (`Rc<Style>` → `&Style`) is unblocked; run anytime.
- Step 3 (interaction enum) is a prerequisite for Step 4's pure renders.
- Step 4 unblocks both the mechanical split (Step 5) and the state split
  (Step 6).
- Step 7 (tests) is most valuable after Step 6 gives a pure `AppState`.
- Step 9 is low-risk and can slot in anywhere after Step 4.

## 9. Verification gates per step

Every step must pass the project's standard gate:

```
cargo nextest run && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings
```

Additionally, after Steps 2, 3, 4, and 6, manually smoke-test:

- Load one of the sample graphs in the repo.
- Add/remove nodes; connect/disconnect inputs and events.
- Undo/redo covers every mutation path.
- Zoom + pan + break-connections stroke + new-node popup.
- Save, quit, re-open, confirm state round-trips.

## 10. Out of scope

- Moving `prism` onto a different UI toolkit.
- Reworking `scenarium`'s graph/worker API. (Prism is a consumer; the
  boundary there is fine.)
- Persistence format changes.
- Adding new features. This plan only reshapes what already exists.
