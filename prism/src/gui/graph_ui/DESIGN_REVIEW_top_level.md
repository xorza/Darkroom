# Design review: prism/src/gui/graph_ui top-level  (2026-04-25)

Scope: top-level orchestration of `graph_ui` — `mod.rs`, `ctx.rs`,
`frame_output.rs`, `gesture.rs`, `pan_zoom.rs`, `layout.rs`,
`background.rs`, `overlays.rs`, `port.rs`. Submodules `nodes/` and
`connections/` reviewed only at their public surface.

## Current design

`GraphUi` is a stateful renderer that holds eight per-graph fields
(`gesture`, `connections`, `graph_layout`, `node_ui`, `dots_background`,
`new_node_ui`, `node_details_ui`, `argument_values_cache`). Each frame
the host calls `render(&mut self, gui, ctx, render_events, input,
output)` with a freshly built read-only `GraphContext` (borrows of
`FuncLib`, `ViewGraph`, `ExecutionStats`, plus a derived
`NodeExecutionIndex`) and a `FrameOutput` buffer. The renderer never
mutates `ViewGraph` directly — every change is queued as a
`GraphUiAction` in `output.actions` and applied in
`Session::commit_actions` after the frame returns.

The interaction model is a single `Gesture` enum (`Idle | Panning |
DraggingConnection | BreakingConnections | DraggingNode`) — one variant
at a time, transitions are atomic assignments. Reset on graph swap is
done by `*self = Self::default()` inside `apply_render_events`, then
later `Cache` events in the same batch apply to the fresh state.

The body of `render` runs three ordered phases inside a `with_scale`
scope: (1) **content** — `refresh_galleys`, `handle_node_interactions`,
background dots, connections, a transient overlay-capture region,
node bodies, then `process_connections`; (2) **overlays** — view-
control buttons, run/autorun buttons, details panel, new-node popup
(returns `overlay_hovered`); (3) **zoom/pan** — only when no overlay
is hovered and the gesture is `Idle | Panning`. Several intra-phase
orderings are load-bearing and documented only in comments (see F1).

## Overall take

The core split is right. Read-only `GraphContext` + write-only
`FrameOutput` + a single `Gesture` enum is a clean shape: there's
exactly one way for the renderer to change graph state, exactly one
piece of "what is the user doing" state, and exactly one path
(`apply_render_events`) by which the model talks back to the renderer.
The recent F1/F2/F3 changes (RenderEvent::Reset, pure shortcut
routing, AppCommand off FrameOutput) are tightening the same axis.

Remaining smells are localized: a fragile orderings-in-comments
contract inside `render_content`, mixed ownership on `GraphUi`'s flat
field list, and a few small abstraction leaks. Nothing here suggests
the module wants a rewrite.

## Findings

### [F1] `render_content`'s ordering invariants live in comments, not types

- **Category**: Contract
- **Impact**: 4/5 — quietly breaks selection / clicks / z-order if a
  refactor reorders calls; failures are visual, not test failures
- **Effort**: 3/5 — refactor within `mod.rs` + `connections/handlers.rs`
- **Current**: `mod.rs:131-156` and `mod.rs:172-214` rely on at least
  four orderings, each justified only by a comment:
  1. `handle_node_interactions` runs *before* `render_nodes` so this
     frame's drag delta folds into `gesture` before render reads it
     (`mod.rs:178-188`).
  2. `render_nodes` runs *before* `background_response.clicked()` is
     consumed at `mod.rs:133`, otherwise the background eats clicks
     meant for nodes (`mod.rs:124-130`).
  3. `Self::maybe_capture_overlay` runs *between* `render_connections`
     and `render_nodes` so port widgets keep higher z-order than the
     overlay (`mod.rs:194-197`, `handlers.rs:91-98`).
  4. `process_connections` runs *after* `render_nodes` so it can read
     `nodes_result.port_cmd` and `broken_nodes` produced this frame
     (`mod.rs:199-213`).
- **Problem**: Each invariant is correct today, but the contract is
  "call these methods in this exact order or things fail in
  hard-to-reproduce ways." Items (2) and (3) are about egui widget
  registration order; items (1) and (4) are about data flow. The
  helper names don't hint that swapping them breaks anything. A new
  contributor inlining or reordering for clarity will silently break
  the editor.
- **Alternative(s)**:
  - **Fold the orderings into one helper.** `render_content` is
    already private; replace the four sibling calls with a single
    `render_graph_content(...)` that *internally* runs them in the
    right order and returns nothing. Move the invariants into the
    helper's body where reordering is visibly local. This costs a
    function but removes the "call site must know" clause.
  - **Pair `maybe_capture_overlay` with `render_connections`.** The
    overlay only matters when a connection-related gesture is active;
    its current call site is structurally between connection-render
    and node-render only because z-order requires it. Make
    `render_connections` end by registering the overlay itself —
    callers no longer need to know about it. (Today it's a `Self::`
    associated fn in `connections/handlers.rs`; the asymmetry is
    suspicious — F4.)
  - **Document the invariant with `#[must_use]`-style return values**:
    `handle_node_interactions` could return a `NodeInteractionsToken`
    that `render_nodes` consumes, encoding "this must run first" in
    the type. Heavier; only worth it if F1's failures actually bite.
- **Recommendation**: Do the first two together. They cost little and
  remove a class of bug from the most-touched function in the module.

### [F2] `GraphUi`'s flat field list mixes render-only state with model-shaped state

- **Category**: State / Responsibility
- **Impact**: 3/5 — lets a new field accidentally inherit "reset on
  graph swap" semantics it shouldn't have (or vice versa), and
  obscures which fields are renderer-internal vs. host-visible
- **Effort**: 2/5 — group fields under one named struct
- **Current**: `mod.rs:64-78` declares eight fields side by side. The
  `Reset` handler is `*self = Self::default()` (`mod.rs:92`), which
  silently extends to *every* field added to the struct. The tests in
  `tests.rs:8-26` only verify that `gesture` and `argument_values_cache`
  reset; the implicit contract for the other six is "Default is
  correct." `argument_values_cache` is also notable: it's a model-flow
  field (mutated by `RenderEvent::Cache` from Session) sitting next to
  pure UI fields.
- **Problem**: "Reset on graph swap" is an invariant of the *whole*
  struct, not declared per-field. Future fields like a user
  preferences cache, a frame-rate counter, or a panel-collapse state
  would silently get wiped on every graph load. There's no way to
  opt-out short of redesigning the reset path. The flat list also
  hides that `argument_values_cache` is conceptually "Session-owned
  buffer the renderer happens to host" while `dots_background` is
  pure renderer-internal infrastructure.
- **Alternative(s)**:
  - **Two-layer state**: `GraphUi { per_graph: PerGraphState,
    persistent: PersistentState }`. `PerGraphState` holds gesture,
    layout, popups, cache, connections, node_ui — the things that
    must reset on graph swap. `Reset` only clears `per_graph`.
    `dots_background` (a texture handle that's expensive to rebuild)
    moves to `persistent`. Cost: one indirection per access.
  - **Explicit reset method** instead of `*self = Self::default()`:
    `self.reset_per_graph()` calls each per-graph field's own
    `clear()` / `*x = Default::default()`. Adding a new field forces
    the author to think about whether to clear it. More verbose, no
    indirection cost.
- **Recommendation**: Do the explicit-reset variant. It's the lowest-
  cost protection against the silent-extension footgun and aligns
  with the existing test (`reset_clears_cache_and_gesture` already
  enumerates the fields it cares about).

### [F3] `render_buttons` bundles two unrelated toolbars

- **Category**: Responsibility
- **Impact**: 2/5 — readability, no correctness risk
- **Effort**: 1/5 — split one function into two
- **Current**: `overlays.rs:22-120` renders the top view-control bar
  (FitAll/ViewSelected/ResetView) and the bottom run-control bar
  (RunOnce/Autorun) in one function, returning `ButtonResult { response,
  action, run_cmd }`. `response` is the bitwise-OR of the two bars'
  responses (`overlays.rs:78` `response |= ...`), used only for one
  consumer (`mod.rs:253` to compute `overlay_hovered`).
- **Problem**: The two bars share nothing — different positions,
  different actions, different output channels. Combining them buys
  one OR'd response at the cost of a 100-line function and a struct
  (`ButtonResult` in `mod.rs:57-62`) that exists only to carry three
  values across one call. The inlined `Layout::new().fill_width().
  apply(gui)` for the top bar but not the bottom bar (`overlays.rs:34`)
  is the kind of accidental asymmetry this shape invites.
- **Alternative**: Split into `render_view_buttons(&mut self, gui) ->
  (Response, Option<ViewButtonAction>)` and `render_exec_buttons(&mut
  self, gui, autorun) -> (Response, Option<RunCommand>)`. Drop
  `ButtonResult` and the local `mut action` / `mut run_cmd`
  accumulators. The OR happens at the one consumer.
- **Recommendation**: Do it. Mechanical refactor, removes a type.

### [F4] `maybe_capture_overlay` is an associated fn on `GraphUi` that lives in the wrong module

- **Category**: Abstraction / Responsibility
- **Impact**: 2/5 — minor; complicates F1's "fold the invariant into
  one helper" fix
- **Effort**: 1/5 — move the function
- **Current**: `connections/handlers.rs:99-110` defines `pub(in
  crate::gui::graph_ui) fn maybe_capture_overlay(gui, gesture)` on
  `impl GraphUi`. It takes no `self` (just reads `gesture: &Gesture`
  passed in), but it lives inside `connections/handlers.rs` and is
  invoked from the orchestrator at `mod.rs:197`. The doc-comment on
  the function admits the placement is odd (`handlers.rs:91-98`):
  "that subtlety is why this lives here and not inside
  `process_connections`."
- **Problem**: It's a free function pretending to be a method,
  declared as a method on `GraphUi` but neither using `self` nor
  living near the orchestrator. The location was chosen to keep all
  z-order/connection logic together, but the call site is in
  `mod.rs`, so a reader chasing the orchestrator must jump into
  `connections/` to see what runs at that step.
- **Alternative**: Make it a free function `capture_gesture_overlay(
  gui: &mut Gui, gesture: &Gesture)` in `gesture.rs` (it reads only
  `Gesture`) or inline it into the F1 consolidated helper. Either
  removes the cross-module hop.
- **Recommendation**: Inline it as part of F1. Don't bother as a
  standalone change.

### [F5] `pan_zoom::compute_scroll_zoom` quietly drops scroll input on a wheel-driven zoom frame

- **Category**: Contract
- **Impact**: 2/5 — it's the documented behavior and the test
  (`pan_zoom.rs:294-307`) pins it; flagging only because the
  decision is encoded in code structure, not types
- **Effort**: 2/5 — replace the implicit then_else with a typed
  intent enum
- **Current**: `pan_zoom.rs:101-104` selects `(zoom_delta, pan_delta)`
  via `(mouse_wheel_delta.abs() > EPSILON).then_else(...)`. If wheel
  is non-zero, `pan_delta` is forced to `Vec2::ZERO`. Then at line
  109, the `if (zoom_delta - 1.0).abs() > EPSILON { ... } else {
  new_pan += pan_delta }` chooses zoom *or* pan but never both. The
  comment at line 110-112 explains: "a wheel tick can produce both a
  zoom signal and a scroll signal; applying both would push the graph
  under the cursor."
- **Problem**: The two-step gate (compute pan_delta as ZERO, then
  branch on zoom_delta) is correct but reads as accidental. The
  contract "this frame is either a zoom or a pan, never both" is
  derived from arithmetic rather than declared. Anyone tweaking the
  scroll/zoom mapping must reconstruct the invariant.
- **Alternative**: Resolve input upstream into `enum ScrollIntent {
  None, Zoom(f32), Pan(Vec2) }` (computed once in `InputSnapshot` or
  a small helper). `compute_scroll_zoom` then matches on the enum.
  The "never both" invariant is enforced by the type, the `then_else`
  trick disappears, and the wheel-takes-precedence rule lives at the
  enum-construction site where it can be tested in isolation.
- **Recommendation**: Depends on whether `ScrollIntent` would be
  useful elsewhere (gesture.rs? input.rs?). Standalone, the change
  is neutral. As part of a larger input-handling cleanup, do it.

## Considered and rejected

- **Replace `FrameOutput`'s scratch fields with `enum FrameSignal`
  collected into one `Vec`.** Tempting for symmetry, but the current
  split (actions buffer applied at end-of-frame vs. side-channel
  signals consumed by `Session::handle_output`) is *load-bearing* for
  egui's multi-pass rendering — see the doc-comment in
  `frame_output.rs:35-44`. Coalescing them would force a stateful
  collapse step in `commit_actions` that doesn't exist today.

- **Move `argument_values_cache` to `Session`.** Its contents include
  GPU texture handles (per the doc-comment "UI-owned per-node
  texture/value cache"), which are eframe-context-bound. Migrating
  ownership would either leak `egui::Context` into `Session` or
  require a parallel "renderer-only" half on `GraphUi` anyway.
  Current placement is correct; F2's grouping is the right fix.

- **Replace `Gesture` with separate `Option<NodeDrag>` /
  `Option<ConnectionDrag>` / `Option<...>` fields.** The current
  enum is exactly the right shape: "user is doing one thing,
  variant carries that thing's data." `gesture.rs:1-9` makes this
  explicit, and the `cancel()` / `start_*` methods preserve the
  invariant by assignment. Don't touch.
