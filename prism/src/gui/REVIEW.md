# GUI Module Review: Data Flow, API & Simplification Suggestions

## Current Architecture Summary

### Call Chain
```
MainUi::render()
  -> GraphUi::render(&mut Gui, &mut AppData, &Bump)
       -> GraphContext constructed from AppData fields
       -> GraphLayout::update()
       -> GraphBackgroundRenderer::render()
       -> ConnectionUi::render()
       -> NodeUi::render_nodes()       -> returns PortInteractCommand
       -> process_connections()         -> uses PortInteractCommand
       -> render_buttons()             -> overlay at scale=1.0
       -> NodeDetailsUi::show()        -> overlay at scale=1.0
       -> handle_new_node_popup()      -> overlay
       -> update_zoom_and_pan()
  <- returns to MainUi
  -> AppData::handle_interaction()     -> drains GraphUiInteraction
```

### Data Flow
```
AppData (owned state)
  |
  v
GraphContext (borrowed view: func_lib + view_graph + execution_stats)
  |
  +---> GraphLayout (computed screen positions, per-frame)
  +---> ConnectionUi (connection curves cache + temp drag)
  +---> NodeUi (renders nodes, returns PortInteractCommand)
  +---> ConstBindUi (const value editors, RAII frame pattern)
  |
  v
GraphUiInteraction (collected actions + errors + run_cmd)
  |
  v
AppData::handle_interaction() (drains actions -> undo stack -> worker)
```

---

## Issues & Suggestions

### 1. GraphUiInteraction lives on AppData but is a GUI-internal concern

**Problem:** `GraphUiInteraction` is a field on `AppData` (`app_data.interaction`), but it is purely a frame-local communication channel between GUI subsystems and the post-render action handler. It gets `clear()`-ed every frame in `handle_interaction()`. Having it on `AppData` means every GUI method receives `&mut GraphUiInteraction` as a separate parameter alongside `&mut GraphContext`, creating wide function signatures and passing it through 4-5 levels of call depth.

**Suggestion:** Move `GraphUiInteraction` to be a field on `GraphUi` instead (or return it from `GraphUi::render()`). The render method would return the collected interaction, and `MainUi` would pass it to `AppData::handle_interaction()`. This eliminates one parameter from nearly every internal GUI function and makes ownership clearer.

```rust
// Before (current):
fn render(&mut self, gui: &mut Gui, app_data: &mut AppData, arena: &Bump) {
    // ... uses app_data.interaction everywhere
}

// After:
fn render(&mut self, gui: &mut Gui, app_data: &mut AppData, arena: &Bump) -> GraphUiInteraction {
    let mut interaction = GraphUiInteraction::default();
    // ... uses local interaction
    interaction
}
```

This also removes the `MainUi::handle_run_shortcuts` writing directly to `app_data.interaction.run_cmd` -- shortcuts could return a `RunCommand` instead.

---

### 2. GraphContext wraps 3 fields but adds little value

**Problem:** `GraphContext` is a 23-line struct that bundles `&FuncLib`, `&mut ViewGraph`, and `Option<&ExecutionStats>`. All three already live on `AppData`. The struct is constructed at the top of `render()` from `AppData` fields, then `execution_stats` is also accessed directly from `AppData` in several places (e.g. `render_connections` takes `app_data.execution_stats.as_ref()` separately). This partial wrapping creates inconsistency -- sometimes data comes from `ctx`, sometimes from `app_data`.

**Suggestion:** Either:
- **(a) Remove `GraphContext`** and pass `AppData` (or a subset) directly. The GUI already takes `&mut AppData`, so the context struct adds an indirection layer without meaningful encapsulation.
- **(b) Make `GraphContext` the sole data conduit** -- put `execution_stats` inside it properly and stop accessing `app_data.execution_stats` separately. Also consider putting `interaction: &mut GraphUiInteraction` in it to reduce parameter count further.

Option (b) would look like:
```rust
struct GraphContext<'a> {
    func_lib: &'a FuncLib,
    view_graph: &'a mut ViewGraph,
    execution_stats: Option<&'a ExecutionStats>,
    interaction: &'a mut GraphUiInteraction,
}
```

This eliminates passing `interaction` as a separate param everywhere.

---

### 3. Connection breaking + node deletion is tangled across GraphUi, ConnectionUi, NodeUi, and ConstBindUi

**Problem:** When the user draws a breaker line, the "broke" state is computed independently in three places:
1. `ConnectionUi::render()` -- sets `curve.broke` on data/event connections
2. `ConstBindUi` (via `ConstBindFrame::render_const_input`) -- sets `curve.broke` on const bindings
3. `NodeUi::render_nodes()` / `render_body()` -- checks `breaker.intersects_rect(layout.body_rect)` for node deletion

Then `apply_broken_connections()` chains `connections.broke_iter()` and `node_ui.const_bind_ui.broke_iter()`, and `remove_nodes_hit_by_breaker()` reads `node_ui.node_ids_hit_breaker`.

**Suggestion:** Consolidate breaking logic into a single pass. After rendering, have one method that collects all "broke" items (connections + const bindings + nodes) and applies them together. This could be a method on a new `BreakerResult` struct, or simply a function that takes the three iterators. The key simplification is making the "what did the breaker hit?" question answerable in one place rather than three.

---

### 4. PortInteractCommand priority-merge pattern is fragile

**Problem:** `PortInteractCommand` uses a `priority()` + `prefer()` pattern where every port rendering call returns a command, and they're merged by priority across all ports of all nodes. This works but is non-obvious -- the priority values (0, 5, 8, 10, 15) are magic numbers, and the semantics depend on rendering order.

**Suggestion:** Replace with an explicit "first match wins" or accumulator pattern. Since egui guarantees only one widget is interacted with at a time, at most one port will return a non-`None` command per frame. A simpler pattern:

```rust
// Instead of merging by priority across all ports:
let cmd = port_cmd.prefer(new_cmd);

// Use early-return or Option:
if result.is_none() { result = check_port_interaction(...); }
```

Or keep the current pattern but document why the priorities exist and add a `const` for each level.

---

### 5. Dual action stacks (`actions1` / `actions2`) are confusing

**Problem:** `GraphUiInteraction` has `actions1` (coalesced, deferred) and `actions2` (immediate). The caller iterates both via `action_stacks()`. The names `actions1` and `actions2` don't convey their purpose.

**Suggestion:** Rename to `coalesced_actions` and `immediate_actions`. Or simplify further: since `immediate()` actions are never coalesced, just use a single `Vec<GraphUiAction>` and flush the pending coalesced action before pushing immediate ones (which is already done). The two-stack pattern exists only so they form separate undo groups, but this could be made explicit:

```rust
struct GraphUiInteraction {
    action_groups: Vec<Vec<GraphUiAction>>,  // each inner vec = one undo group
    pending_coalesced: Option<GraphUiAction>,
    // ...
}
```

---

### 6. Scale ping-pong between GraphUi::render levels

**Problem:** In `GraphUi::render()`, scale is set multiple times:
1. `gui.set_scale(ctx.view_graph.scale)` -- for graph-space rendering
2. `gui.set_scale(1.0)` -- for overlay buttons and panels
3. The overlay code runs at scale=1.0 while graph code runs at graph scale

This means the scale changes mid-render, and any code reading `gui.scale()` gets different values depending on when it's called. The `Style` is rebuilt via `Rc::make_mut` on every scale change.

**Suggestion:** Avoid mutating scale on the shared `Gui`. Instead:
- Render graph content in a child `Gui` with the graph scale applied
- Render overlays in the parent `Gui` at scale=1.0
- This makes it impossible for overlay code to accidentally use graph scale

```rust
// Graph-space rendering in a scaled child:
gui.new_child_with_scale(ctx.view_graph.scale, |scaled_gui| {
    self.graph_layout.update(scaled_gui, &ctx);
    // ...
});

// Overlays at scale=1.0 (no scale change needed):
self.render_buttons(gui, ...);
```

---

### 7. NodeDetailsUi duplicates execution status logic from NodeUi

**Problem:** Both `NodeUi` (via `NodeExecutionInfo`) and `NodeDetailsUi` (via `ExecutionStatus` / `get_execution_status`) independently look up execution stats for a node by scanning `stats.node_errors`, `stats.missing_inputs`, `stats.executed_nodes`, `stats.cached_nodes`. These are two separate enums with the same cases doing the same linear scans.

**Suggestion:** Unify into a single `NodeExecutionInfo` (or similar) that both can use. The lookup can be a method on `ExecutionStats` or a free function in a shared location:

```rust
// In scenarium or a shared module:
impl ExecutionStats {
    fn node_status(&self, node_id: NodeId) -> NodeExecutionStatus { ... }
}
```

---

### 8. ConstBindUi RAII frame pattern is clever but hard to follow

**Problem:** `ConstBindUi::start()` returns a `ConstBindFrame` that holds `CompactInsert` (from `compact_insert_start()`) and tracks hovered state. The `Drop` impl on `ConstBindFrame` updates the parent's `hovered_link`. This works but is non-obvious -- the frame *must* be dropped at the right time, and the `Drop` impl has side effects that affect next-frame rendering.

**Suggestion:** Make it explicit with a `finish()` method instead of relying on `Drop`:

```rust
let mut frame = self.const_bind_ui.start();
// ... render const bindings ...
frame.finish(); // explicitly commits hovered state
```

This is more readable and doesn't depend on drop ordering. The `CompactInsert` already uses this pattern (it's consumed by `drop`), so at minimum add a comment explaining the RAII contract.

---

### 9. `process_connections` has convoluted control flow

**Problem:** `process_connections()` matches on interaction mode twice with an early-return pattern and an intermediate `gui.interact()` call between them:

```rust
match self.interaction.mode() {
    PanningGraph => return,
    Idle => { ...; return; }
    Breaking | Dragging => {
        gui.ui.interact(rect, id, Sense::all());  // capture overlay
    }
};
match self.interaction.mode() {
    PanningGraph | Idle => unreachable!(),
    Breaking => { ... }
    Dragging => { ... }
}
```

The double-match exists solely to insert an overlay interaction between the two passes.

**Suggestion:** Restructure as a single match with the overlay capture inside each relevant arm:

```rust
match self.interaction.mode() {
    PanningGraph => return,
    Idle => { self.handle_idle_state(...); }
    BreakingConnections => {
        capture_overlay(gui);
        self.handle_breaking_connections(...);
    }
    DraggingNewConnection => {
        capture_overlay(gui);
        self.handle_dragging_connection(...);
    }
}
```

---

### 10. `handle_new_node_popup` mixes popup lifecycle with connection state cleanup

**Problem:** When the new-node popup closes without a selection, `handle_new_node_popup` calls `self.interaction.reset_to_idle()` and `self.connections.stop_drag()`. This couples popup UI with connection drag state cleanup. Similar cleanup happens in `handle_background_click`, `handle_breaking_connections`, `handle_dragging_connection`, and `should_cancel_interaction` -- the same two-line pattern (`reset_to_idle` + `stop_drag`) appears 6+ times.

**Suggestion:** Create a single `cancel_interaction()` method on `GraphUi`:

```rust
fn cancel_interaction(&mut self) {
    self.interaction.reset_to_idle();
    self.connections.stop_drag();
}
```

This is already partially done in `should_cancel_interaction` but only for Escape/right-click. Unify all call sites.

---

### 11. Autorun state flows through GraphUiInteraction unnecessarily

**Problem:** `autorun` is a field on `AppData`. The GUI reads it from `app_data.autorun`, and when the button is clicked, it sets `app_data.interaction.run_cmd = RunCommand::StartAutorun/StopAutorun`. Then `handle_interaction()` reads `run_cmd` and sends worker messages. The autorun toggle takes a round-trip through the interaction system for no reason -- it's not undoable and doesn't need coalescing.

**Suggestion:** Either:
- Have `render_buttons` return a `RunCommand` directly (or set it on a return struct), skipping the interaction channel
- Or accept it as-is since `RunCommand` is already minimal. But if `GraphUiInteraction` moves to `GraphUi` (suggestion 1), this becomes a natural part of the return value.

---

### 12. `GraphUi::render` does too many things

**Problem:** `GraphUi::render` is the 948-line orchestrator that handles: cancellation, background frame, child UI creation, context construction, background interaction, layout, dots, connections, nodes, connection processing, scale switching, overlay buttons, node details panel, new node popup, zoom/pan. This makes the render flow hard to trace.

**Suggestion:** Group into named phases:

```rust
fn render(&mut self, gui: &mut Gui, app_data: &mut AppData, arena: &Bump) {
    self.handle_cancellation(gui);
    let rect = self.draw_background_frame(gui);

    gui.new_child(..., |gui| {
        let mut ctx = ...;
        self.render_graph_content(gui, &mut ctx, app_data);
        self.render_overlays(gui, &mut ctx, app_data, arena);
        self.handle_input(gui, &mut ctx, app_data);
    });
}
```

Where `render_graph_content` covers layout + background + connections + nodes, `render_overlays` covers buttons + details + popup, and `handle_input` covers zoom/pan/connection processing.

---

## Summary: Priority Ranking

| # | Suggestion | Impact | Effort |
|---|-----------|--------|--------|
| 1 | Move `GraphUiInteraction` out of `AppData` | High - simplifies all signatures | Medium |
| 10 | Unify `cancel_interaction()` pattern | Medium - removes duplication | Low |
| 2 | Fix `GraphContext` to be sole data conduit | Medium - removes inconsistency | Low |
| 9 | Simplify `process_connections` control flow | Medium - readability | Low |
| 12 | Split `GraphUi::render` into phases | Medium - readability | Medium |
| 5 | Rename/simplify dual action stacks | Low-Med - readability | Low |
| 7 | Unify execution status lookup | Low-Med - removes duplication | Low |
| 3 | Consolidate breaker logic | Medium - removes scatter | Medium |
| 6 | Avoid scale ping-pong | Medium - correctness | Medium |
| 4 | Simplify PortInteractCommand merging | Low - readability | Low |
| 8 | Make ConstBindFrame explicit | Low - readability | Low |
| 11 | Simplify autorun flow | Low | Low |
