# GUI Module Review (Round 2)

Post-cleanup review after implementing suggestions 1-3, 5, 7, 9-12 from the first review.

## Current State

The architecture is clean. `GraphUi::render()` has clear 3-phase structure with scoped scaling. `GraphContext` is the sole data conduit. `GraphUiInteraction` lives on `GraphUi` where it belongs. Breaker results are collected in one pass. Action coalescing is explicit with named stacks.

---

## Remaining Suggestions

### 1. ConnectionUi::render() mixes rendering with mutation

**Problem:** `ConnectionUi::render()` (~110 lines) does three things in one loop:
1. Renders bezier curves (visual)
2. Detects breaker intersections (state mutation on `curve.broke`)
3. Applies double-click deletion (graph mutation via `input.binding = Binding::None`)

The double-click deletion directly mutates `node.inputs` and pushes actions to `ui_interaction` from inside the render pass. This means the render method has side effects on the graph model — a connection can disappear mid-render while other connections are still being drawn.

**Suggestion:** Collect double-click deletions into a `Vec` during the render loop and apply them afterward (similar to how `node_ids_to_remove` works in `NodeUi`). This separates "what to draw" from "what to change".

```rust
// During render loop:
if response.double_clicked_by(PointerButton::Primary) {
    deletions.push((node_id, input_idx, input.binding.clone()));
}

// After render loop:
for (node_id, input_idx, before) in deletions {
    // apply mutation + push action
}
```

**Impact:** Medium — cleaner separation of rendering and mutation  
**Effort:** Low

---

### 2. `NodeUi::render_nodes()` passes too many &mut params through the call chain

**Problem:** `render_nodes()` takes 5 parameters (`gui`, `ctx`, `graph_layout`, `interaction`, `breaker`), then passes subsets of these to `handle_node_drag()` (4 params), `const_bind_frame.render()` (6 params), `render_cache_btn()` (3 params), etc. Most of these functions need the same 3-4 things.

**Suggestion:** Bundle the rendering context into a short-lived struct:

```rust
struct NodeRenderCtx<'a> {
    gui: &'a mut Gui<'a>,
    ctx: &'a mut GraphContext<'a>,
    graph_layout: &'a mut GraphLayout,
    interaction: &'a mut GraphUiInteraction,
    breaker: Option<&'a ConnectionBreaker>,
}
```

This reduces all the 4-6 param signatures to a single `&mut NodeRenderCtx`. The struct is constructed at the start of `render_nodes()` and dropped at the end.

**Impact:** Medium — reduces noise in function signatures  
**Effort:** Medium (borrow checker may resist bundling `&mut` references)

---

### 3. PortInteractCommand priority values are magic numbers

**Problem:** `PortInteractCommand::priority()` returns magic values `0, 5, 8, 10, 15` without explanation. The `prefer()` method picks the higher-priority command but doesn't document why these specific values exist or what invariants they encode.

**Suggestion:** Add named constants and a comment explaining the priority order:

```rust
impl PortInteractCommand {
    // Click > DragStop > DragStart > Hover > None
    // Higher priority wins when multiple ports report commands in the same frame.
    const PRIORITY_NONE: u8 = 0;
    const PRIORITY_HOVER: u8 = 5;
    const PRIORITY_DRAG_START: u8 = 8;
    const PRIORITY_DRAG_STOP: u8 = 10;
    const PRIORITY_CLICK: u8 = 15;
}
```

**Impact:** Low — readability  
**Effort:** Low

---

### 4. ConstBindFrame relies on Drop for side effects

**Problem:** `ConstBindFrame` uses `Drop` to commit the `currently_hovered_connection` state back to `ConstBindUi.hovered_link`. This is the only side effect in the `Drop` impl. It works correctly but is non-obvious — a reader won't expect `drop(const_bind_frame)` to update hover state.

**Suggestion:** Replace with an explicit `finish()` method that consumes `self`:

```rust
impl<'a> ConstBindFrame<'a> {
    fn finish(self) {
        *self.prev_hovered_connection = self.currently_hovered_connection;
        std::mem::forget(self); // skip Drop
    }
}
```

Or simpler: just add a comment at the `drop(const_bind_frame)` call site explaining the RAII contract:

```rust
// Commits hovered_link state back to ConstBindUi on drop
drop(const_bind_frame);
```

**Impact:** Low — readability  
**Effort:** Low

---

### 5. `render_buttons()` mixes view-action logic with button rendering

**Problem:** `render_buttons()` renders two button groups (top: view controls, bottom: execution controls), then applies view actions (`reset_view`, `view_selected`, `fit_all`) at the end. The view-action application calls free functions (`view_selected_node`, `fit_all_nodes`) that mutate `ctx.view_graph`. This mixes "render buttons" with "apply navigation commands".

**Suggestion:** Have `render_buttons()` return a small struct describing what was clicked, and apply the view actions in the caller:

```rust
struct ButtonResult {
    response: Response,
    fit_all: bool,
    view_selected: bool,
    reset_view: bool,
}
```

Then in `render()`:
```rust
let buttons = self.render_buttons(gui, &mut ctx);
if buttons.reset_view { ... }
if buttons.view_selected { view_selected_node(gui, &mut ctx, ...) }
// etc.
```

This makes `render_buttons` purely about rendering and the caller responsible for applying effects.

**Impact:** Low-Medium — cleaner separation  
**Effort:** Low

---

### 6. `new_node_ui.rs` accesses `gui.ui` directly

**Problem:** In `NewNodeUi::show()`, there's a direct access to `gui.ui` (the private field) instead of going through `gui.ui()`:

```rust
gui.ui.interact(
    gui.rect,
    Id::new("temp background for new node ui"),
    Sense::all(),
);
```

This bypasses the `Gui` wrapper's public API. The `ui` field is not `pub` (it's only accessible within the module due to Rust's struct field visibility within the same crate), suggesting this was not intentional.

**Suggestion:** Change to `gui.ui().interact(...)`. If there's a borrow issue with `gui.rect`, extract it to a local first (same pattern used in `capture_overlay`).

**Impact:** Low — consistency  
**Effort:** Low

---

### 7. `LogUi` accesses `gui.ui` directly, bypasses Gui wrapper entirely

**Problem:** `LogUi::render()` uses `gui.ui` (the raw field) exclusively via `frame.show(gui.ui, ...)`. It never calls `gui.ui()` or uses any `Gui` methods. The entire render method works directly on the underlying egui `Ui`.

**Suggestion:** Either:
- **(a)** Have `LogUi::render()` take `&mut Ui` and `&Style` directly instead of `&mut Gui`, making explicit that it doesn't use the Gui wrapper.
- **(b)** Refactor to use `gui.ui()` properly and use Gui helpers where applicable.

Option (a) is simpler and more honest about the actual dependency.

**Impact:** Low — API clarity  
**Effort:** Low

---

### 8. `handle_node_drag` is a long free function that could be a method

**Problem:** `handle_node_drag()` (50 lines) is a free function taking 5 parameters. It handles selection, drag state, position updates, and layout refresh. It's the most complex free function in `node_ui.rs` and is only called from `render_nodes()`.

**Suggestion:** Make it a method on `NodeUi` (or on a `NodeRenderCtx` if suggestion 2 is adopted). This would let it access `self` state directly and reduce the parameter count.

Alternatively, split it into two parts: `handle_selection()` and `handle_drag()`, since these are independent concerns that happen to share the same `response`.

**Impact:** Low — readability  
**Effort:** Low

---

### 9. GraphBackgroundRenderer has excessive assertions for scale normalization

**Problem:** `wrap_scale_multiplier()` has 10 assertions for what is a simple "find nearest power-of-2 multiplier" operation. While correctness-focused, this is excessive for a pure math function with bounded inputs (scale is clamped to `0.2..4.0` by the zoom logic).

**Suggestion:** Keep the pre-condition asserts (finite, positive) but remove the intermediate ones (`k_low.is_finite`, `k_low <= k_high`, `contains(&k)`) since they can't fail if the inputs are valid. Add a unit test instead that covers the edge cases.

**Impact:** Low — readability  
**Effort:** Low

---

### 10. Style is fully rebuilt on every scale change

**Problem:** `Style::set_scale()` calls `Style::new()` which reconstructs the entire `Style` struct (all fonts, colors, shadows, sub-styles). Most fields don't depend on scale — colors, corner radius ratios, etc. are scale-independent. Only ~15 of ~50 fields actually change with scale.

**Suggestion:** Split `Style` into `StyleBase` (scale-independent) and `ScaledStyle` (scale-dependent). `set_scale()` would only rebuild `ScaledStyle`. Or simpler: cache the last-used scale and skip rebuild if unchanged (which `Gui::set_scale` already checks via `ui_equals`, but `with_scale` calls `set_scale` twice per frame — once to set graph scale, once to restore).

This is a micro-optimization and only matters if profiling shows it as a hotspot. The `Rc::make_mut` pattern means the clone only happens when the `Rc` is shared, which is every frame due to child Gui construction.

**Impact:** Low (potential perf, no correctness issue)  
**Effort:** Medium

---

## Summary: Priority Ranking

| # | Suggestion | Impact | Effort |
|---|-----------|--------|--------|
| 1 | Separate double-click deletion from render pass | Medium | Low |
| 5 | Return button results instead of applying inline | Low-Med | Low |
| 2 | Bundle NodeUi render params into context struct | Medium | Medium |
| 3 | Named constants for port priority values | Low | Low |
| 4 | Make ConstBindFrame Drop explicit | Low | Low |
| 6 | Fix `gui.ui` direct access in new_node_ui | Low | Low |
| 7 | LogUi should take `&mut Ui` instead of `&mut Gui` | Low | Low |
| 8 | Make handle_node_drag a method or split it | Low | Low |
| 9 | Reduce assertions in wrap_scale_multiplier | Low | Low |
| 10 | Avoid full Style rebuild on scale change | Low | Medium |
