# Design review: prism/src/gui/  (2026-04-25)

Scope (per user): how data structures are placed and how the file tree is organized. Code-level polish is out of scope — see `/improve-module` for that.

## Current design

The crate's GUI module is a 47-file, ~9.9k-LOC tree with three layers of structure:

1. **Top-level `gui/`** — entry (`mod.rs` defines the `Gui<'_>` wrapper, `app.rs` is the `eframe::App` impl, `main_window.rs` wires panels), plus a flat pile of feature files: `node_ui.rs`, `connection_ui.rs`, `const_bind_ui.rs`, `connection_breaker.rs`, `connection_bezier.rs`, `node_layout.rs`, `node_details_ui.rs`, `new_node_ui.rs`, `graph_layout.rs`, `graph_ctx.rs`, `frame_output.rs`, `gesture.rs`, `graph_background.rs`, `style.rs`, `value_editor.rs`, `log_ui.rs`, `image_utils.rs`, `ui_host.rs`.
2. **`gui/graph_ui/`** — a submodule split into `mod.rs` (render entry + tests), `connections.rs`, `overlays.rs`, `pan_zoom.rs`. Each submodule adds an `impl GraphUi` block.
3. **`gui/widgets/`** — a coherent leaf module of reusable widgets.

The runtime ownership is straight-line: `GuiApp { session, main_window }` → `MainWindow { graph_ui, log_ui, style }` → `GraphUi { gesture, connections, graph_layout, node_ui, dots_background, new_node_ui, node_details_ui, argument_values_cache }`. Per-frame data flows through two value types: `GraphContext<'a>` (immutable bundle borrowed from `Session`) on the way in, `FrameOutput` (action queue + side channels) on the way out. `Gui<'_>` wraps `egui::Ui` and threads a styled, scaled child wrapper everywhere.

Load-bearing decisions:
- **`GraphContext`/`FrameOutput` as the only IO contract** between the GUI and `Session`. Nothing inside `graph_ui` mutates `ViewGraph` directly — actions flow through a buffer that `Session::commit_actions` applies.
- **`Gesture` as a single-variant enum** holding the per-frame interaction state (drag connection, drag node, pan, breaker). Variant data is the data the gesture needs.
- **Galleys cached, layout computed**: `GraphLayout` keeps `NodeGalleys` (shaped text — expensive); `NodeLayout` is a pure value computed at every call site from `(galleys, view_pos, drag_offset, scale)`.

## Overall take

The data-flow architecture (read-only context in, action buffer out, gesture as a tagged union, layout computed on demand) is sound — there's no obvious load-bearing alternative that's better. **The weakness is file-and-type placement**, not the data shapes themselves: the `gui/` directory is flat where `graph_ui/` is hierarchical, related types are scattered across files for historical reasons, and one struct (`NodeUi`) survives as a one-field wrapper that no longer pays for its existence. The findings below are entirely structural.

## Findings

### [F1] `gui/` is flat where `graph_ui/` is hierarchical — files don't reflect ownership

- **Category**: File structure
- **Impact**: 3/5 — improves navigability and forces module boundaries to mean something
- **Effort**: 2/5 — pure file-move + import fixups
- **Current**: `node_ui.rs` (547), `connection_ui.rs` (852), `const_bind_ui.rs` (231), `connection_bezier.rs`, `connection_breaker.rs`, `node_layout.rs`, `graph_layout.rs`, `gesture.rs`, `graph_ctx.rs`, `frame_output.rs`, `node_details_ui.rs`, `new_node_ui.rs`, `graph_background.rs` all live as siblings of `graph_ui/`, even though every one of them is only consumed by code inside `graph_ui/`. Meanwhile `graph_ui/` itself was split into `mod.rs` + 3 submodules. Rationale for the split is not visible — the criterion appears to be "this file got too long," not "this is a separately ownable concern."
- **Problem**: A reader can't tell from the directory tree what depends on what. `style.rs`, `image_utils.rs`, `ui_host.rs`, `value_editor.rs`, `log_ui.rs` are crate-wide concerns; they should sit alongside `widgets/`. The graph-editor-only files (`gesture`, `graph_layout`, `graph_ctx`, `node_layout`, `node_ui`, `connection_*`, `const_bind_ui`, `node_details_ui`, `new_node_ui`, `graph_background`, `frame_output`) should live inside `graph_ui/`. After the move, `gui/mod.rs` exposes `Gui`, `MainWindow`, `app`, `style`, `widgets/`, `graph_ui/`, and a couple of utilities — that's the actual public shape.
- **Alternative**:
  ```
  gui/
    mod.rs                  // Gui<'_>, run, ScopedGui
    app.rs                  // GuiApp
    main_window.rs          // MainWindow + file_menu + shortcuts
    style.rs
    log_ui.rs
    ui_host.rs
    image_utils.rs
    value_editor.rs
    widgets/                (unchanged)
    graph_ui/
      mod.rs                // GraphUi, render entry
      ctx.rs                // GraphContext (renamed from graph_ctx.rs)
      frame_output.rs
      gesture.rs
      layout.rs             // GraphLayout (galley cache) + NodeLayout (pure)
      port.rs               // PortKind + PortRef + PortInfo (see F3)
      background.rs         // GraphBackgroundRenderer
      overlays.rs           // (unchanged)
      pan_zoom.rs           // (unchanged)
      nodes/
        mod.rs              // node body interaction + render
        const_bind.rs       // ConstBindUi
        details.rs          // NodeDetailsUi
        new_node.rs         // NewNodeUi
      connections/
        mod.rs              // ConnectionUi (render + temp drag preview)
        types.rs            // ConnectionKey, ConnectionCurve, ConnectionDrag, BrokeItem
        bezier.rs
        breaker.rs
        actions.rs          // advance_drag, disconnect_connection, order_ports (current connections.rs in graph_ui/)
  ```
- **Recommendation**: Do it. The internal API doesn't change — only paths.

### [F2] `connection_ui.rs` (852 LOC) is a four-concept file

- **Category**: File structure / Missing named modules
- **Impact**: 3/5 — biggest single file in the module, mixes orthogonal concerns
- **Effort**: 3/5 — split is mechanical, but `pub(crate)` visibility + cross-file `impl` blocks need re-checking
- **Current**: `connection_ui.rs` defines:
  1. **Port types** — `PortKind`, used by ports, connections, layout, gesture (`connection_ui.rs:40-61`).
  2. **Drag state machine** — `ConnectionDrag`, `ConnectionDragUpdate`, `advance_drag`, `try_snap_to_port`, `finish_drag`, `order_ports` (`connection_ui.rs:62-103, 352-440`).
  3. **Render-time curve cache** — `ConnectionCurve`, `HighlightCurve`, `ConnectionUi`, `render`, `render_temp_connection`, `update_curve_interaction`, `render_highlight_curve`, `show_curve` (`connection_ui.rs:105-343, 451-528`).
  4. **Action emission** — `ConnectionKey`, `BrokeItem`, `disconnect_connection`, `apply_connection_deletions` (`connection_ui.rs:20-46, 387-440`).
- **Problem**: A reader looking for "what is a port" lands in the middle of a connection-rendering file. `PortKind` is imported by `graph_layout.rs` (which defines `PortRef` containing a `PortKind`!), `node_ui.rs`, `gesture.rs`, `connection_breaker.rs` — none of which are about connection rendering. Also: tests at the bottom mix drag-state-machine tests with action-emission tests with no separator beyond comments.
- **Alternative**: Split as in F1 above (`port.rs`, `connections/types.rs`, `connections/actions.rs`, `connections/mod.rs` for the renderer). Each new file lands in the 100–300 LOC range. Tests move to live next to the code under test.
- **Recommendation**: Do it, bundled with F1.

### [F3] `PortKind` and `PortRef` are split across files but tightly coupled

- **Category**: Missing named module / Data structure placement
- **Impact**: 3/5 — fixes a circular-feeling import (`graph_layout.rs` importing from `connection_ui.rs` for a primitive)
- **Effort**: 2/5 — move two types, update imports
- **Current**: `PortKind` is in `connection_ui.rs:40-61`. `PortRef { node_id, port_idx, kind: PortKind }` and `PortInfo { port: PortRef, center: Pos2 }` are in `graph_layout.rs:14-25`. `port_center(port: &PortRef)` is on `NodeLayout` (`node_layout.rs:260-267`). Every consumer that wants to talk about a port has to import `PortKind` from `connection_ui` and `PortRef`/`PortInfo` from `graph_layout` — two unrelated-looking modules for one concept.
- **Problem**: There's no `port` module, even though "port" is one of the central nouns of the graph editor. The current placement is an artifact of where each type was first needed.
- **Alternative**: Create `graph_ui/port.rs` containing `PortKind`, `PortRef`, `PortInfo`. Remove the corresponding lines from `connection_ui.rs` and `graph_layout.rs`. `node_layout.rs::port_center` stays put — it's a method on `NodeLayout`, not on `PortRef`.
- **Recommendation**: Do it. Trivial and clarifying.

### [F4] `NodeUi` is a vestigial wrapper around `ConstBindUi`

- **Category**: State that shouldn't exist
- **Impact**: 2/5 — removes one indirection and a misleading struct name
- **Effort**: 1/5 — rename + flatten
- **Current**: `NodeUi { const_bind_ui: ConstBindUi }` (`node_ui.rs:110-113`). The two methods on `NodeUi` — `handle_node_interactions` and `render_nodes` — only ever read `self.const_bind_ui` via `start()` to get a frame helper; everything else is a free helper. The struct's name suggests it owns node-rendering state, but all the render work uses parameters (`gui`, `ctx`, `graph_layout`, `gesture`).
- **Problem**: `node_ui.NodeUi.const_bind_ui` is three layers of name to reach the actual state. The `NodeUi` struct doesn't model a thing — the only state in node rendering is the const-bind cache.
- **Alternative**: Hoist `const_bind_ui: ConstBindUi` to be a direct field of `GraphUi`. Demote `handle_node_interactions` and `render_nodes` to free functions in `graph_ui/nodes/mod.rs` (taking `gesture`, `graph_layout`, `const_bind: &mut ConstBindUi`, etc. by parameter). Caller sites already pass these in by name.
- **Recommendation**: Do it.

### [F5] `graph_ui/` submodules carve up `impl GraphUi` instead of carving up state

- **Category**: Abstraction / File structure
- **Impact**: 2/5 — current arrangement compiles fine; the issue is conceptual coherence
- **Effort**: 4/5 — would touch most of `graph_ui/`'s methods
- **Current**: `graph_ui/mod.rs` defines `GraphUi` with eight fields and the top-level `render`. `graph_ui/connections.rs` adds `impl GraphUi` with `process_connections`, `handle_drag_result`, `apply_breaker_results`, `handle_background_click`, plus free helpers like `handle_idle`, `build_data_connection_action`, `build_event_connection_action`. `graph_ui/overlays.rs` adds `impl GraphUi` with button rendering + new-node popup. `graph_ui/pan_zoom.rs` is similar. Result: `GraphUi` is one ~50-method god struct sliced across four files only for line-count reasons. Every method still has access to all eight fields; the file boundary doesn't enforce anything.
- **Problem**: The split is "this file got long," not "this is a separable concern." A genuine separation would let each submodule own its slice of state (e.g. the connection/break interaction owns `gesture` mutations + the action-builders; pan/zoom owns nothing — it's already pure free functions). Right now any helper in `connections.rs` can reach into `self.new_node_ui` if it wants to, and nothing stops it.
- **Alternative**: Pull the free-function helpers out of the `impl GraphUi` blocks (most are already free — `handle_idle`, `build_data_connection_action`, `build_event_connection_action`). The remaining `impl GraphUi` methods that take `&mut self` for a reason — `process_connections`, `apply_breaker_results`, `handle_drag_result`, `render_buttons`, `handle_new_node_popup`, `update_zoom_and_pan` — stay on `GraphUi`, but the *file* layout follows the submodule layout proposed in F1: connection logic in `connections/actions.rs`, overlays in `nodes/details.rs`/`nodes/new_node.rs`/`overlays.rs`, pan/zoom in `pan_zoom.rs`. Tests in `graph_ui/mod.rs` (currently lines 288–575) move to live next to the code under test (`build_data_connection_action` tests → `connections/actions.rs`; `handle_idle` tests → wherever `handle_idle` lands).
- **Recommendation**: Depends on F1. If you do F1, this falls out for free as a side effect. Don't do this in isolation — the value is mostly bundled with the larger restructure.

### [F6] `node_layout.rs` co-locates a cache with a pure value

- **Category**: File structure / Mixed concerns
- **Impact**: 1/5 — file is only 272 LOC; both types are clearly named
- **Effort**: 1/5 — split into two files
- **Current**: `NodeGalleys` (mutable, holds `Arc<Galley>` + scale; constructed via `gui.painter().layout_no_wrap(...)`) and `NodeLayout` (pure data, computed via `compute(...)` and never stored) live in the same file (`node_layout.rs:25-100` and `:109-272`).
- **Problem**: Minor. A reader looking for "where do node geometry rects come from" gets a file that opens with text-shaping caching code. `NodeLayout::compute` is testable as a pure function without `Gui`/`Painter`; testing it directly is awkward when the file's first half drags in `Gui` machinery.
- **Alternative**: `graph_ui/layout/galleys.rs` (NodeGalleys) and `graph_ui/layout/node_layout.rs` (NodeLayout). Or merge into the broader `graph_ui/layout.rs` proposed in F1.
- **Recommendation**: Bundle with F1. Don't do as a standalone change.

## Considered and rejected

- **Splitting `GraphUi` into "interaction-state" + "render-cache" structs.** Tempting (eight fields suggests two roles), but the per-frame `render` ordering already correctly threads `&mut gesture` through the interaction phase and `&gesture` through render — the *call sites* already enforce the split. Forcing the struct itself to split would just add a holder struct without removing any coupling.
- **Replacing `Gesture::DraggingNode { released: bool }` with two variants.** The "released this frame" flag looks like a smell (a flag inside a state-machine variant), but the workaround comment (`gesture.rs:27-32`) explains a real ordering constraint that the alternative — a `JustReleasedNode` transient variant — would just rename rather than fix. Out of scope for a structural review anyway.
- **Moving `argument_values_cache` from `GraphUi` to `GraphContext`.** Looked at this because `node_details_ui` takes it as an extra `&mut` parameter. Rejected: the cache is *UI-owned* (it survives across `Session` resets, lives where the renderer can drain `Session::take_cache_events`); putting it in the read-only `GraphContext` would invert that ownership for cosmetic reasons.
- **Promoting `widgets/` out of `gui/`.** Some widgets (`PopupMenu`, `Frame`) are reused outside the graph editor (status bar, file menu) but never outside `gui/`. No callers outside the GUI crate depend on them. Leave it.

---

# Design review: prism/src/gui/  (2026-04-25, follow-up)

Scope (per user): data flow, dependencies, isolation of responsibility, testability. Structural placement (file moves, type relocations) is out of scope — see the prior section (F1–F6).

## Current design

After the F1–F5 restructure, the data contract is clean on paper: `Session::frame(output, |s, o| ...)` enforces `drain_inbound → render → handle_output`; the render closure reads through `GraphContext<'_>` (immutable bundle) and writes through `FrameOutput` (action queue + side-channel signals: errors, run/editor/app commands, an argument-values request `NodeId`). `MainWindow::render` and `GraphUi::render` are the two render entry points; everything below them is supposed to be "context in, output out."

In practice the renderer breaks that boundary in two places. First, `GraphUi::render` takes `&mut Session` and calls `session.take_cache_events()` on entry (`graph_ui/mod.rs:95`) — a render-time mutation of session state that exists only because `ArgumentValuesCache` lives on `GraphUi`, not on `Session`. Second, `&mut self.argument_values_cache` is threaded as a fourth parameter into `node_details_ui.show()` (`graph_ui/mod.rs:239`), parallel to `FrameOutput`. So there are effectively two action channels: `FrameOutput` for graph mutations, `ArgumentValuesCache` for the per-node texture/value cache. Render-time emission is also scattered: `render_remove_btn`, `render_cache_btn`, and `handle_node_interactions` all push `GraphUiAction`s mid-paint (`nodes/mod.rs:155, 224, 355`), so "what actions does this interaction produce?" cannot be answered without running paint.

The shared infrastructure type `Gui<'_>` is a god-handle: it bundles `&mut Ui`, `Rc<Style>`, `scale`, and `rect`. Most pure-geometry code (`NodeLayout::compute`, `pan_zoom::compute_scroll_zoom`, `view_selected_node_target`, `fit_all_nodes_target`) only needs `(style, scale, rect)` but is forced to take `&Gui<'_>` because `GraphLayout::node_layout` does. That coupling is the single biggest reason `graph_ui` tests are sparse outside `connections::handlers`.

## Overall take

The action-buffer architecture is right; the leaks above are localized. The biggest payoff is closing the cache-event side channel (F1 below) — that's what unlocks "render is a pure function of `(GraphContext, mut FrameOutput)`" and, consequently, headless tests of every render submodule. The `Gui<'_>` split (F2) is a smaller win but it's what makes layout/zoom maths actually unit-testable. F3–F5 are smaller cleanups; F4 is just finishing F4 from the prior review.

## Findings

### [F1] `ArgumentValuesCache` is a second action channel parallel to `FrameOutput`

- **Category**: Contract / Responsibility
- **Impact**: 4/5 — collapses a hidden side-channel and removes the only `&mut Session` reason from render
- **Effort**: 3/5 — moves one field, redirects two call sites, changes `Session::take_cache_events` into an `apply` method
- **Current**:
  - `GraphUi` owns `argument_values_cache: ArgumentValuesCache` (`graph_ui/mod.rs:76`).
  - `GraphUi::render` takes `session: &mut Session` purely so it can call `for event in session.take_cache_events()` at the top of the frame (`graph_ui/mod.rs:91, 95`).
  - The cache is then threaded as `&mut self.argument_values_cache` into `node_details_ui.show()` (`graph_ui/mod.rs:239`) — a fourth `&mut` parameter alongside `gui`, `ctx`, `output`. `NodeDetailsUi::show` re-passes it to `show_content` (`nodes/details.rs:31, 52`).
  - `request_argument_values` flows the *opposite* way through `FrameOutput::set_request_argument_values` (`frame_output.rs:107`), which `Session::handle_output` translates into a worker request — so the cache has *one* exit lane via `FrameOutput` and *one entirely separate* entry lane via `take_cache_events`.
  - The previous review explicitly considered and rejected moving the cache to `GraphContext`. That reasoning still holds — the cache is UI-owned and survives `Session` graph resets — but it doesn't justify the current shape, where session and renderer share mutable cache state across a render-time mutation.
- **Problem**:
  - `GraphUi::render` cannot be called with a borrowed-only graph context. The whole point of `GraphContext` ("read-only bundle of frame-level dependencies") is undermined: the actual render API is `(read-only ctx, AND a mut Session, AND a mut FrameOutput)`.
  - The cache is written by *render* (via `node_details_ui` setting `request_argument_values` then `Session` reposting an `ArgumentValues` event that lands in `cache_events`) but also *drained* by render. That's a feedback loop the renderer can't close on its own.
  - Tests of `GraphUi::render` (none exist) would need a real `Session`, which itself needs `EguiUiHost`, a worker, a tokio runtime — none of it stubbed.
- **Alternative**:
  Treat the cache like the action buffer: it's render-owned, but inputs land via a frame-input bundle and outputs land via `FrameOutput`.
  ```rust
  pub struct GraphFrameInputs<'a> {
      pub ctx: GraphContext<'a>,
      pub cache_events: Vec<CacheEvent>,   // moved out of Session this frame
  }
  ```
  Or, simpler: extend `GraphContext` with a `cache_events: &'a mut Vec<CacheEvent>` (drained by GraphUi during render — borrow ends with the context). `Session::frame`'s closure builds the bundle:
  ```rust
  session.frame(output, |s, output| {
      let mut cache_events = s.take_cache_events();
      let ctx = s.graph_context_with_events(&mut cache_events);
      graph_ui.render(gui, &ctx, input, output);  // no &mut Session
      ...
  });
  ```
  Either way `GraphUi::render` loses `&mut Session`, and "what does render mutate?" answers exactly: `&mut self` (renderer state), `&mut FrameOutput` (egress).
- **Recommendation**: Do it. The `&mut Session` parameter is the single biggest reason graph_ui can't be tested headlessly today.

### [F2] `Gui<'_>` is the unit of dependency for code that only needs read-only style/scale/rect

- **Category**: Abstraction / Testability
- **Impact**: 4/5 — unblocks unit tests for layout maths and zoom-target maths, both of which are pure today but trapped behind `&Gui`
- **Effort**: 3/5 — touches every signature in `layout.rs`, `pan_zoom.rs`, parts of `nodes/mod.rs`; mechanical
- **Current**:
  - `NodeLayout::compute` is pure (`layout.rs:132`) — takes `&NodeGalleys`, `&Func`, `&Style`, scale, origin, pos. No `Gui`. Good.
  - But `GraphLayout::node_layout` (`layout.rs:318`) takes `&Gui<'_>` and uses it only for `gui.scale()`, `&gui.style`, and the `origin(gui, ctx)` helper which itself only needs `gui.rect.min` (`layout.rs:294`).
  - `pan_zoom::compute_scroll_zoom`, `view_selected_node_target`, `fit_all_nodes_target` (`pan_zoom.rs:91, 126, 152`) are documented as "pure target-computing functions" but each takes `&Gui<'_>` — for `gui.rect` only (and indirectly `gui.scale()` via `node_layout`).
  - Result: not one of these can be tested without an `egui::Ui`, despite being pure maths. The existing tests for `build_data_connection_action` / `handle_idle` (`connections/handlers.rs:365–644`) only succeed because those helpers were carefully kept `Gui`-free.
- **Problem**: `Gui<'_>` couples the `&mut Ui` (paint sink, mutable) to `Style`/`scale`/`rect` (frame-constant, read-only) and to `painter()`/`fonts()` queries. Code that only consumes the read-only half is forced to also accept the mutable half, foreclosing tests.
- **Alternative**: Introduce `ViewParams<'a> { style: &'a Style, scale: f32, rect: Rect }` — produced by `Gui::view_params(&self) -> ViewParams<'_>` — and rewrite the signatures of pure functions to take `&ViewParams`:
  ```rust
  fn node_layout(&self, vp: &ViewParams<'_>, ctx: &GraphContext<'_>, id: &NodeId, drag: Vec2) -> NodeLayout
  fn compute_scroll_zoom(vp: &ViewParams<'_>, input: &InputSnapshot, ...) -> (f32, Vec2)
  fn fit_all_nodes_target(vp: &ViewParams<'_>, ctx: &GraphContext<'_>, gl: &GraphLayout) -> (f32, Vec2)
  ```
  `Gui::scale`/`Gui::style`/`Gui::rect` already exist — `view_params()` is a one-line constructor. Painting code keeps taking `&mut Gui<'_>`.
- **Recommendation**: Do it, and write the unit tests that fall out (zoom-pinning maths, fit-all bounds, `NodeLayout` boundary cases). These are exactly the kinds of computations regressions slip into.

### [F3] Render functions emit `GraphUiAction`s mid-paint

- **Category**: Responsibility / Control flow
- **Impact**: 3/5 — separates "what did the user do?" from "what got drawn"; enables interaction-only tests
- **Effort**: 4/5 — touches `render_nodes`, `render_remove_btn`, `render_cache_btn`, `render_buttons`
- **Current**:
  - The interaction/render split was already done for *node-body drag*: `handle_node_interactions` runs first and fills the gesture, `render_nodes` then paints with the up-to-date offset (`graph_ui/mod.rs:171, 188`).
  - But `render_nodes` still emits `NodeRemoved` (via `render_remove_btn` returning a bool that the caller pushes — `nodes/mod.rs:223`), `CacheToggled` (`nodes/mod.rs:355`), and `PortInteractCommand` is bubbled out of `render_ports` (`nodes/mod.rs:447`) so connection-drag start lives downstream of paint.
  - `render_buttons` emits `RunCommand` and a `ViewButtonAction` enum mid-paint (`overlays.rs:97, 106`). Same for `handle_new_node_popup`.
  - Net effect: an integration test like "double-clicking a remove button emits one `NodeRemoved`" requires `egui::Ui` plumbing because the remove button is a render widget.
- **Problem**: Action emission is functionally a side-effect of widget interaction events, but those events are also load-bearing for paint (hover state, pressed state). Coupling them means there's no way to drive the action layer in isolation. This also makes the "actions are immediate" invariant (`frame_output.rs:38`) harder to reason about — multiple sites push during render in an order that depends on iteration order over `view_nodes`.
- **Alternative**: Two passes per node, mirroring what already exists for body drag:
  1. **Interact pass** — emits actions and `PortInteractCommand`, returns a `NodeRenderState` (selected, breaker_hit, port_layouts) that's all the render needs.
  2. **Render pass** — pure function of `NodeRenderState + NodeLayout + NodeGalleys`.

  Two-stage approach for buttons mirrors what `pan_zoom`'s pure helpers already do for view targets — `render_buttons` returns `ButtonResult { action }`, then the caller emits the action. Apply the same shape uniformly: render returns intent, caller emits.
- **Recommendation**: Bundle with F2. The pure render half becomes testable as a snapshot ("given this state, what shapes were drawn?") even via `egui::Painter` capture.

### [F4] `NodeUi` and `NodeDetailsUi` are vestigial namespaces

- **Category**: State that shouldn't exist
- **Impact**: 2/5 — removes two indirections, no behavioral change
- **Effort**: 1/5
- **Current**:
  - `NodeUi { const_bind_ui: ConstBindUi }` (`nodes/mod.rs:113`) — single field, two methods that take `&mut self` only to reach `self.const_bind_ui.start()`.
  - `NodeDetailsUi;` is a unit struct (`nodes/details.rs:23`); `show()` is `&self` and uses no fields. It's a namespace.
  - The previous review's F4 already named `NodeUi` and was not yet executed.
- **Problem**: Both types add a layer to function signatures and field names without modeling any state. Readers searching for "node interaction state" find an empty struct.
- **Alternative**: Hoist `const_bind_ui` to a direct field on `GraphUi`. Demote both types' methods to free functions in `nodes/mod.rs` and `nodes/details.rs`. Drop the structs.
- **Recommendation**: Do it. Pure cleanup; precondition for future work that wants the const-bind cache visible to other render submodules without going through a wrapper.

### [F5] `process_connections` and `update_zoom_and_pan` are free functions wearing `&mut self` costumes

- **Category**: Abstraction / Responsibility
- **Impact**: 2/5 — minor — clarifies which fields actually couple to which call site
- **Effort**: 2/5 — re-thread fields into params; signatures already use `#[allow(clippy::too_many_arguments)]`
- **Current**:
  - `process_connections` takes 7 args and uses `self.gesture`, `self.connections`, `self.node_ui.const_bind_ui`, `self.new_node_ui` (`connections/handlers.rs:47`).
  - `update_zoom_and_pan` takes 6 args and uses only `self.gesture` (`pan_zoom.rs:23`).
  - `render_buttons` uses no fields beyond `self`'s implicit access — same for `handle_new_node_popup` which uses `self.new_node_ui`, `self.gesture`.
  - The pattern: methods on `GraphUi` because they need 1–2 fields, but the signature already lists every other dependency explicitly. The `impl GraphUi` block isn't carrying its weight.
- **Problem**: Signatures already document the dependencies; the `&mut self` is doing nothing the explicit params don't. Reviewers can't tell which subsystem owns which state.
- **Alternative**: Convert to free functions whose first parameter is the specific state slice they touch:
  ```rust
  fn process_connections(
      gesture: &mut Gesture,
      connections: &ConnectionUi,
      const_bind: &ConstBindUi,
      new_node_ui: &mut NewNodeUi,
      input: &InputSnapshot,
      ctx: &GraphContext<'_>,
      ...
  )
  ```
  `update_zoom_and_pan` then just takes `&mut Gesture`. Each function's signature becomes a contract that lists what it touches.
- **Recommendation**: Depends on F3. If F3 lands, this falls out as the natural shape; in isolation it's noise.

## Considered and rejected

- **Splitting `Gesture` mutation into one owner per gesture variant.** Pan-zoom touches `Gesture::Idle ↔ Panning`, connections touch `Idle ↔ Dragging/Breaking`, node body touches `Idle ↔ DraggingNode`. Tempting to give each subsystem its own mutator, but the variants are *exclusive* — exactly one transition is legal per frame — and `Gesture::cancel()` is a load-bearing escape hatch from any state. A shared `&mut Gesture` is correct; the smell is the call-site count, not the type.
- **Replacing `Session::frame` with an explicit context object.** Considered making `Session::frame` return a `FrameGuard<'_>` that exposes only what render is allowed to do (read graph, mutate output) and Drop-applies on scope exit. Rejected: `MainWindow` legitimately needs `&mut Session` *outside* the closure (`handle_app_command`, `session.empty_graph()` in `main_window.rs:80`). The closure-based `frame` is the correct shape; the leak is F1, not the API itself.
- **Moving `request_argument_values` from `FrameOutput` to a callback.** It's currently `Option<NodeId>` on `FrameOutput`, and `Session::handle_output` spawns an async request. Looked clean as a callback (`request_argument_values: &dyn Fn(NodeId)`), but the deferred-via-output shape is consistent with everything else in `FrameOutput` — and `NodeDetailsUi` already gates duplicate requests via `cache.mark_pending(...)` before setting the field. Leave it.

