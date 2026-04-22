# Design review: prism/src/gui/graph_ui/  (2026-04-22)

## Current design

Top-level graph view. `GraphUi` is an 8-field orchestrator struct that owns every per-frame UI subsystem (gesture, connections, graph_layout, node_ui, dots_background, new_node_ui, node_details_ui, output). `render()` runs a three-phase pipeline per frame: **(1) content** (nodes, connections, background at graph scale); **(2) overlays** (buttons, details panel, new-node popup); **(3) zoom/pan** (only if no overlay is hovered). Behavior is split across four files — `mod.rs` holds the entry point and struct, while `connections.rs`, `overlays.rs`, and `pan_zoom.rs` each add `impl GraphUi` blocks adding private methods that freely access all fields.

Action discipline is strong: every graph mutation flows through `FrameOutput` as a `GraphUiAction` and is applied end-of-frame in `AppData::handle_actions`. Rendering is side-effect-free with respect to `ViewGraph`. Pure helpers (`handle_idle`, `build_data_connection_action`, `build_event_connection_action`) are extracted for unit testing — tests live under `mod.rs`'s `tests` module and cover cycle detection, event subscription, and the idle-state transition table.

Load-bearing invariants: `refresh_galleys` must run before any `node_layout` read; nodes must register their widgets before background click state is read (for click-priority routing); `maybe_capture_overlay` must register between connections and ports so ports stay on top; `gesture` stays in `DraggingConnection` through popup dismissal so `create_const_binding` can read the drag's input port. All documented in comments.

## Overall take

The module is well-designed. The three-phase pipeline is explicit, the action boundary is consistent, and testable pure helpers have been pulled out where it matters. Remaining issues are mid-sized cosmetic/testability concerns, not architectural. No rethink.

## Findings

### [F1] `ButtonResult` represents at-most-one-action as three independent bools
- **Category**: Types
- **Impact**: 2/5 — cosmetic + minor expressiveness
- **Effort**: 1/5 — single struct change
- **Current**: `ButtonResult { response, fit_all, view_selected, reset_view }` at `mod.rs:41-46`. Consumed at `mod.rs:227-238`:
  ```rust
  if buttons.reset_view { ... }
  if buttons.view_selected && let Some(...) = ... { ... }
  if buttons.fit_all { ... }
  ```
  All three bools are mutually exclusive by construction (each fires on a distinct button's click; only one button can be clicked per frame).
- **Problem**: Three booleans pretend to be independent but are actually an enum. A reader must infer mutual exclusivity from UI semantics; the type allows states the UI can't produce (e.g. all three true).
- **Alternative**:
  ```rust
  enum ViewButtonAction { ResetView, ViewSelected, FitAll }
  struct ButtonResult { response: Response, action: Option<ViewButtonAction> }
  ```
  The consumer becomes `match buttons.action { Some(ResetView) => ..., ... }` — exhaustiveness gives the compiler a say.
- **Recommendation**: Do it when next touching overlays — it's trivial, but not urgent on its own.

### [F2] Pure helpers and orchestration mixed in `connections.rs`
- **Category**: Responsibility / Abstraction
- **Impact**: 3/5 — improves testability of action-emitting logic
- **Effort**: 3/5 — refactor several methods to take explicit params instead of `&mut self`
- **Current**: `connections.rs` contains `impl GraphUi` methods (`process_connections`, `handle_drag_result`, `apply_connection`, `apply_breaker_results`) that each produce graph-ui actions, alongside pure helpers (`handle_idle`, `build_data_connection_action`, `build_event_connection_action`). The pure helpers have unit tests; the impl methods do not, because they need a full `GraphUi` instance to invoke.
- **Problem**: Action-emitting logic is the riskiest code — it's where graph-state transitions originate. But only the already-pure helpers are tested. `handle_drag_result`, for instance, decides between "commit the connection", "open new-node popup", and "cancel" based on the drag update; a bug there silently breaks the drag → popup → const-bind flow. No unit test can reach it because it's on `GraphUi`.
- **Alternative**: Split each impl method into a pure "decide what to do" helper + a thin method that applies the decision.
  ```rust
  // pure, testable
  fn plan_drag_result(result: ConnectionDragUpdate, ctx: &GraphContext) -> DragOutcome { ... }
  enum DragOutcome { Cancel, OpenPopup(Pos2), OpenPopupFromConnection(Pos2), Commit(GraphUiAction), CommitEventConn(GraphUiAction) }
  
  // thin dispatcher
  fn handle_drag_result(&mut self, outcome: DragOutcome) { match outcome { ... } }
  ```
  Tests cover `plan_drag_result`; dispatcher is a trivial match.
- **Recommendation**: Do it for `handle_drag_result` + `apply_connection`. Probably skip for `apply_breaker_results` — the body is already mostly a gather-and-dispatch over an iterator, and breaker hits are already tested through the `disconnect_connection` helper.

### [F3] `Error` is a connection-specific type with a generic name at module scope
- **Category**: Types / Abstraction
- **Impact**: 1/5 — naming only
- **Effort**: 1/5 — rename + move
- **Current**: `pub(crate) enum Error { CycleDetected { ... } }` at `mod.rs:48-68`. Sole variant, sole consumers: the two `build_*_connection_action` helpers in `connections.rs` and their tests. `FrameOutput::add_error` takes it.
- **Problem**: The name "Error" at a module-level namespace suggests module-wide errors — but it's exclusively about connection-cycle detection. Future reviewers may add unrelated variants under the same name rather than introducing new types.
- **Alternative**: Rename to `ConnectionError` and move to `connections.rs`. `FrameOutput` already takes it by generic name (`add_error(error: Error)`) — the type is re-exported up the chain, so renaming is a touch-up across a handful of sites.
- **Recommendation**: Do it — it's a 10-minute change that prevents a likely naming mistake.

## Considered and rejected

- **Break up the `GraphUi` god-struct into composed subsystems with explicit borrows.** The struct is 8 fields deep and every submodule accesses all of them via `&mut self`. At first glance this looks like classic god-object. But the orchestration genuinely needs cross-subsystem coordination — `process_connections` touches `gesture`, `connections`, `output`, `node_ui.const_bind_ui`, and `new_node_ui` in one call chain. Splitting into composed subsystems would require threading half a dozen mutable borrows through every method, which is strictly worse for readability. The current god-struct + distributed `impl` blocks is the right tradeoff for a top-level UI orchestrator.

- **Give `ConnectionDragUpdate` clearer variant names.** Current: `Finished` / `FinishedWithEmptyOutput { input_port }` / `FinishedWithEmptyInput { output_port }` / `FinishedWith { input_port, output_port }`. "EmptyOutput" is ambiguous — could read as "output endpoint exists but is empty" instead of "no output endpoint". Alternatives like `EndedAt(Some(input), None)` lose the type-level exhaustiveness, and `EndedAtInput { input }` etc. are more verbose without clearer semantics. Current names are slightly awkward but not a real source of bugs.

- **Make `maybe_capture_overlay` a free function.** It takes `&mut Gui` + `&Gesture`, ignores self. But it lives in `impl GraphUi` for namespacing symmetry with the rest of `connections.rs`, and the `Self::maybe_capture_overlay(...)` call-site reads naturally. Not worth changing.

- **Make the three-phase pipeline explicit in code structure (phase methods with typed inputs/outputs).** The `render_content` → `render_overlays` → `update_zoom_and_pan` sequence has implicit data dependencies (overlay hover suppresses zoom/pan; background response flows phase 1 → phase 3). Turning these into explicit pipeline values (`Phase1Output { overlay_hovered: bool }`) would encode the dependencies in types. But: the doc comment at `mod.rs:92-95` already describes the pipeline, and the three methods sit consecutively in `render()` — readers don't actually lose track. Formalizing would add types for no gain.
