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

### Step 4 — Render functions become pure; mutations go through actions ✅ PARTIAL

**Scope limit.** The full "render takes `&ViewGraph`, returns intents,
orchestrator applies them" model conflicts with the current undo
architecture: `ActionUndoStack::push_current` expects actions to have
*already* been applied (it stores a snapshot + action delta for undo/redo,
it does not itself apply). Moving to pure-projection rendering means
reversing that contract — `push_current` would need to `.apply()` the
actions itself, and every existing mutation path (drag, selection, const
edit, connection create/delete, zoom/pan) would have to stop mutating
`ViewGraph` directly at render time. That touches every render site, the
undo stack impl, and every action's apply/undo pair — a substantial
change with real regression risk.

Rather than do that poorly, this pass extracts the clearest structural
offender and flags the rest for Step 4.2.

**Work done in this pass.**
- `NodeUi::render_nodes` used to hold two buffers (`node_ids_to_remove`,
  `node_ids_hit_breaker`) and call `ctx.view_graph.remove_node()` itself
  at the end of the render loop. It now returns a `NodesFrameResult`
  containing `{ port_cmd, removed_nodes, broken_nodes }`. The orchestrator
  (`GraphUi::render`) applies the removals explicitly right after the
  render call, which is the only place `remove_node` is now invoked from
  rendering. The buffers are gone — no hidden per-frame state on `NodeUi`.
- `NodeUi::broke_node_iter` deleted; `apply_breaker_results` now takes
  `&[NodeId]` directly from `NodesFrameResult.broken_nodes`.
- `PortInteractCommand` gained `#[derive(Default)]` so the result struct
  can be `Default`-constructed without a custom constructor.

**What remains (Step 4.2).**
1. `const_bind_ui.rs` still mutates `node.inputs[i].binding` mid-render
   (via the `return true` signal back to `render_nodes`), and the
   `StaticValueEditor` path edits `&mut StaticValue` in place while the
   user types. Making these pure requires either (a) routing edits through
   the action queue and reconciling the egui text buffer against the
   post-apply state, or (b) keeping edits in place but clearly marking
   them as "live-edit" mutations outside the intent model.
2. `handle_node_drag` still writes `view_graph.view_nodes[i].pos` and
   `view_graph.selected_node_id` directly during the render loop. Same
   live-edit problem as (1).
3. `GraphContext` still exposes `&mut ViewGraph` plus `&mut
   ArgumentValuesCache`. Flattening it out of render is Step 4.3.
4. The undo-stack contract change (`push_current` → `apply + record`) is
   the prerequisite for (1) and (2). Do it as a standalone refactor.

**Verification.** 28 tests pass; `cargo clippy --all-targets -- -D warnings`
green. No behaviour change expected or observed — the removal path
executes at the same point, just one indirection further up.

**Size / risk.** Shipped a narrow slice; 2 files touched. The full Step 4
is retitled Step 4.2 and kept in the DAG.

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

### Step 7 — Deterministic snapshot tests

**Problem.** The audit's Finding #10: zero tests for any GUI interaction path.
After Steps 2–6 there is finally something testable: the pipeline
`InputSnapshot + AppState + Interaction → Vec<Intent> → Vec<GraphUiAction> →
AppState'`. All egui-free.

**Target.**
- `tests/graph_intents.rs`: given a fixture `AppState` plus an
  `InputSnapshot`, assert the intents produced by each render phase.
- `tests/action_apply.rs`: given `AppState` + `Vec<GraphUiAction>`, assert
  that `apply_all` + `undo_all` round-trips to the original state (key
  invariant for undo correctness; currently only spot-checked).
- `tests/session_worker.rs`: tokio test that sends a graph update, receives
  `ExecutionStats`, and asserts the cache is invalidated.

**Work.** Build two or three realistic `AppState` fixtures (empty, small
graph, graph with event subscribers + bindings + selection); reuse them
across tests.

**Verify.** CI; aim for at least one test per `GraphUiAction` variant.

**Size / risk.** ~2 days. Low, pure addition.

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
          │            │            ├─ Step 4 (partial ✅) ┐
          │            │            │                       ├─ Step 4.2 ─┐
          │            │            │                       │             ├─ Step 5 ─┐
          │            │            │                       │             │          ├─ Step 7
Step 8 ✅ ┘            │            │                       │             └─ Step 6 ─┘
                       │            │                       └─ Step 9
                       └─ Step 8.2
```

- Steps 1, 2, 3, 8 shipped; Step 4 shipped a narrow slice. Step 8.2 (full
  `&'a Style` borrow) and Step 4.2 (pure renders + undo-contract change)
  are the next non-trivial pieces.
- Step 4.2 depends on reworking `UndoStack::push_current` to apply
  actions; once that lands, const-bind and node-drag mutations can move
  out of render.
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
