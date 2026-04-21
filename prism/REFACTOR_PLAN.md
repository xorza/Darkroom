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

### Step 2 — `InputSnapshot`: sample input once per frame

**Problem.** `gui.ui().input(|i| …)` is called from many call sites (shortcut
handlers in `main_ui.rs:166-229`, three sequential closures in
`new_node_ui.rs:136-141`, wheel/pointer reads in `graph_ui.rs:759-778`, pointer
state in `graph_ui.rs:202-214`). Between closures egui can deliver new events,
so the same logical frame observes different input. The audit called this out
as a concrete race risk.

**Target.**
```rust
pub struct InputSnapshot {
    pub pointer_pos: Option<Pos2>,
    pub primary: ButtonPhase,   // Pressed | Down | Released | None
    pub secondary_pressed: bool,
    pub modifiers: egui::Modifiers,
    pub keys_pressed: SmallVec<[Key; 4]>,
    pub scroll: Vec2,
    pub wheel_lines: f32,
    pub escape_pressed: bool,
}
```
Captured once at the top of `MainUi::render` and threaded to `GraphUi::render`
and every sub-view. Removes the `PointerButtonState` helper entirely (REVIEW F5).

**Work.**
1. Add `prism/src/input/snapshot.rs` with `InputSnapshot::capture(&egui::Ui)`.
2. Replace every `ui.input(|i| …)` in `gui/` and `main_ui.rs` with reads from
   the snapshot. (~30 call sites based on current grep.)
3. Collapse `collect_scroll_mouse_wheel_deltas`
   (`graph_ui.rs:757-778`) into `InputSnapshot::capture`.
4. Delete `PointerButtonState` + `get_primary_button_state`.

**Verify.** Interactions — click, drag connection, scroll zoom, ctrl-Z, ESC
cancel — all work identically. Add one unit test of the capture function by
feeding a synthetic `egui::RawInput` through a minimal `Context::run` and
asserting field contents.

**Size / risk.** ~1 day. Medium — touches many files, all mechanical.

---

### Step 3 — Interaction as a real enum-with-data state machine

**Problem.** `GraphInteractionState` (`src/gui/interaction_state.rs:29-36`)
keeps `mode: InteractionMode` **plus** a `connection_breaker: ConnectionBreaker`
field that is only meaningful in `BreakingConnections`. Callers reach for
`self.interaction.breaker()` in places where the breaker mustn't exist
(`graph_ui.rs:151` passes it into `render_nodes` regardless of mode).
Cancellation is spread across `cancel_interaction` (`graph_ui.rs:105-108`),
`should_cancel_interaction` (`:197`), and a handful of inline
`reset_to_idle()` calls (`:259, 350, 433, 740`) — each must remember to also
call `connections.stop_drag()`. The audit's Finding #3.

**Target.**
```rust
pub enum Interaction {
    Idle,
    Panning { anchor: Pos2 },
    DraggingConnection { drag: ConnectionDrag },
    BreakingConnections { breaker: ConnectionBreaker },
}

impl Interaction {
    pub fn cancel(&mut self) { *self = Self::Idle; }
    pub fn update(&mut self, input: &InputSnapshot, /*…*/) -> Option<Transition>;
}
```
Variant-local data means the breaker literally cannot be reached from `Idle`.
`cancel` is one atomic assignment; there is nothing else to forget.

Additionally, `ConnectionUi::temp_connection` moves **into**
`DraggingConnection`. That removes the dual-source-of-truth problem where
"what is the UI doing?" must be inferred from both `interaction.mode()` and
`connections.temp_connection`.

**Work.**
1. Introduce the new enum in `prism/src/interaction/mod.rs`; keep the old
   module as a thin shim that delegates, so callers can be migrated one file
   at a time.
2. Move `ConnectionBreaker` state into `BreakingConnections`; remove the
   always-present field.
3. Move `temp_connection` into `DraggingConnection`; `ConnectionUi` becomes a
   stateless renderer that takes `Option<&ConnectionDrag>`.
4. Delete every explicit `connections.stop_drag()` call — variant replacement
   handles it.
5. Delete `GraphInteractionState::breaker()`.

**Verify.** The existing tests in `interaction_state.rs` migrate and should
still pass (expanded: "BreakingConnections → Idle drops breaker", "Idle has
no breaker accessor — compile-time check via trybuild", "cancel from
DraggingConnection also drops pending temp").

**Size / risk.** ~2 days. Medium. Touches `graph_ui.rs`, `connection_ui.rs`,
`interaction_state.rs`. Public surface is internal, so refactor can be
mechanical.

---

### Step 4 — Render functions become pure; mutations go through actions

**Problem.** Several `render_*` functions mutate state mid-frame:
- `node_ui.rs:125-173` accumulates `node_ids_to_remove` and removes nodes from
  the layout during the render loop.
- `const_bind_ui.rs:59-85` clears bindings inside render.
- `graph_ui.rs:process_connections` issues graph mutations while drawing.

This hides the origin of mutations (hard to grep for "where do nodes get
deleted?"), makes render impossible to call speculatively (e.g. for a preview
or a test double), and defers surprises until action-apply time.

**Target.** Every `render_*` returns `Vec<Intent>` (or pushes into a
`&mut IntentBuffer`). The orchestrator (`GraphUi::render` today,
`view::graph::mod.rs` after Step 5) is the only place that converts intents
to `GraphUiAction`s. The existing `GraphUiInteraction` becomes a thin wrapper
over that buffer.

A single rule in CI: `grep -R 'fn render' prism/src/view/ | xargs -I{} check
that body has no view_graph mutation`. (Can be enforced via a trait bound:
make render take `&ViewGraph`, not `&mut ViewGraph`.)

**Work.**
1. Add `Intent` enum (a superset of `PortInteractCommand` + connection
   creations + deletions + node removals).
2. Change `node_ui::render_nodes` signature to take `&ViewGraph` + return
   `Vec<Intent>`. Move the `remove_node` logic into the orchestrator.
3. Same for `const_bind_ui`.
4. Fold `process_connections` into the orchestrator; the render pass only
   surfaces port interactions.
5. Delete `GraphContext`'s `&mut view_graph` field — it becomes `&ViewGraph`.
   The cache is the only remaining mutable field; lift it out of the
   context too (pass it explicitly where needed, once per frame).

**Verify.** Regression: deleting a node while a new-node popup is open must
still close the popup (currently emergent; now explicit via an intent).
Add unit tests on the `Intent → GraphUiAction` translator — this is the first
piece of the view layer that is egui-free and thus testable.

**Size / risk.** ~3 days. Medium-high. This is the central payoff: render is
now a pure projection. All state changes converge on one function.

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
          ├─ Step 2 ─┐
          │          ├─ Step 3 ─┐
          │          │          ├─ Step 4 ─┐
          │          │          │          ├─ Step 5 ─┐
          │          │          │          │          ├─ Step 7
Step 8 ✅ ┘          │          │          └─ Step 6 ─┘
                     │          └─ Step 9
```

- Steps 1 and 8 shipped. The end-state of Step 8 (full `&'a Style` borrow
  instead of `Rc<Style>`) is deferred to Step 8.2 — do it after Step 3.
- Step 2 (InputSnapshot) is a hard prerequisite for Step 3's clean
  interaction API.
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
