# Scenarium Implementation Notes (AI)

This file captures implementation details and internal structure for AI agents.

## Project Overview

Scenarium collects the tooling required to build node based applications. The repository is a Rust workspace containing
the core graph implementation and an editor front end.
Editor node UI styling now pulls widget/selection colors from `Style`, with `Style` initialized from egui visuals to
centralize theme values.
`Style::new` now takes a `scale` argument and scales font sizes, padding, radii, stroke widths, and other size
constants for UI sizing control.
Node rendering constants now live in `Style.node` (`NodeStyle`) instead of standalone `Style` fields (including node status dots, port colors, and port sizing).
Node header layout now accounts for the remove button width plus padding when computing header width.
Node remove button sizing now comes from `Style.node.remove_btn_size`.
Node corner radius now lives on `Style` instead of `Style.node`.
Node body rendering now reuses the shared `Style` background fill/stroke colors instead of dedicated node body fields.
Selected node outlines now use `Style.active_bg_stroke`.
Graph background dotted grid styling now lives in `Style.background` (`GraphBackgroundStyle`).
Connection stroke styling now lives in `Style.connections` (`ConnectionStyle`).
`NodeLayout` and its layout computation now live in `editor/src/gui/node_layout.rs`.
Node layout now caches text galleys for titles and port labels to reuse during rendering.
Cache button text now uses a cached galley in `NodeLayout` for rendering.
Connection breaker now reserves segment capacity based on `MAX_BREAKER_LENGTH` and `MIN_POINT_DISTANCE`.
GraphLayout now stores node layouts directly in `KeyIndexVec` and compacts entries during updates instead of clearing.
Graph UI context now carries the current graph scale for shared access in render helpers.
Graph UI context now owns `&mut ViewGraph` and optional execution stats, so graph UI helpers read shared state from
`GraphContext` instead of passing view/exec data through every call.
Graph UI context now asserts valid scale inputs and exposes a `button_with` helper to render custom-shaped buttons with
shared widget styling.
`button_with` now accepts any shape iterator to avoid intermediate allocations at call sites.
Editor node layout constants (width, header/cache/row heights, padding, corner radius) now live in `Style`, and
`NodeLayout` only keeps computed geometry.
Node port interaction and rendering now happen in a single pass that both draws ports and returns drag/hover state.
Node cache/remove button interactions and their side effects now run in `NodeUi::process_input`, leaving render logic
focused on drawing.
Port drag/hover interaction now happens in `NodeUi::process_input`, with port rendering split into a draw-only pass.
Connection drag handling is now folded into `GraphUi::process_connections` to keep connection input logic in one place.
`GraphUi::process_connections` now uses simpler primary-state flags and consolidates drag/breaker updates for clearer
flow.
`GraphUi::process_connections` now requires an in-bounds `Pos2` and is only called when the pointer is inside the graph
rect.
Connection UI now owns the shared bezier control-offset helper used for connection drawing and drag previews.
Temp connection rendering now draws the full sampled bezier range including the endpoint.
Const input badges now clamp and validate scale to keep layout stable at extreme zooms.
Const input badge labels now right-align to the badge edge and remain vertically centered.
Const int input badges now use a custom editor `DragValue` widget that renders text-only and handles drag updates
without egui background styling.
Const int drag updates now use the total drag delta to keep values stable across frames while dragging.
Const int badge widths now derive from label symbol count times a monospaced font size, and static value editors use a
dedicated monospaced font from Style.
`DragValue` now accepts position/alignment only, deriving its interaction rect from the value text galley size.
`DragValue` can render its own background fill/stroke and is used for const int badges.
`DragValue` now supports configurable padding around the text for sizing and alignment.
`DragValue` now supports click-to-edit inline text input, committing on Enter or focus loss and canceling with Escape.
`DragValue` now sets a horizontal resize cursor on hover in drag mode.
`DragValue` now configures font and color via builder methods instead of constructor args.
GUI helpers now include a `Gui` wrapper that pairs `Style` with `egui::Ui`, and `DragValue` uses it to pull default
font/color/background/padding when not explicitly set.
`Gui` now owns a cloned `Style` to avoid borrowing it from callers.
`Gui` now exposes `set_scale` to update scale and recreate `Style` when zoom changes.
`Style.node` now carries `const_badge_offset` for const input badge positioning.
Const int drag widgets now render directly on the main UI instead of using a child text UI scope.
Const input badge link now renders as a bezier polyline mesh instead of a straight line.
Const input badge rendering is owned by `ConstBindUi` on `NodeUi`.
Custom-shaped buttons now use the `common::button::Button` helper instead of `GraphContext`.
Toggle buttons now use the `common::toggle_button::ToggleButton` helper instead of `GraphContext`.
`ToggleButton` now supports optional background styling with default theme values.
Node cache toggle updates now happen directly inside the cache button renderer.
Port drag selection now uses a helper to prefer the highest-priority drag state.
Graph UI `apply_connection` now documents its return value and panic conditions via rustdoc comments.
Fit-all now computes bounds in graph space to keep repeated fit operations stable across zoom levels.
Fit-all now derives bounds from view-node positions and layout sizes instead of screen-space rects to prevent repeated
fit clicks from shifting zoom/pan.
View-selected now centers based on graph-space node position/size to avoid repeated clicks shifting the camera.
Graph background rendering now lives in `gui/background.rs`, caches its mesh, and only rebuilds on pan/scale/rect size
changes.
Graph rendering now uses a fixed-rect child `Ui` with a clip rect to prevent CentralPanel borders from shifting when
offscreen widgets (like const bindings) extend beyond the graph area.
The editor now overrides `eframe::App::clear_color` to use `visuals.panel_fill`, so the window background behind the
CentralPanel outer margin matches the top/bottom panel and menu background.
Graph UI interactions now collect connection errors (like cycle detection) and surface the latest one via the status
bar in `AppData::handle_graph_ui_actions`.
`GraphUi` now owns its `GraphUiInteraction` state and `GraphUi::render` returns a reference to the interaction payload
instead of storing it on `MainUi`.
`Style::apply` now updates egui visuals to match the editor theme (panel/window fills, widget colors, selection, and
corner radii).
`common::button::ButtonBackground` now tracks a separate `hovered_stroke`, and buttons render that stroke on hover.
`Button::background` now accepts a `ButtonBackground` struct instead of individual color/stroke arguments.
`ToggleButton::background` now accepts a `ToggleButtonBackground` struct instead of individual fill/stroke arguments.
`ToggleButtonBackground` now includes a `hovered_stroke`, and toggle buttons render that stroke on hover.
Node header fills now switch to the connection breaker color when the breaker path intersects the node body.
Node title rendering now uses `galley_with_override_text_color` so breaker-hit overrides can change the title color.
`MainUi::handle_undo_shortcut` now stubs out Cmd/Ctrl+Z handling and returns early from render when not pressed.
`AppData::undo` is stubbed for future undo stack integration.
Undo/redo now keeps the current snapshot at the top of the undo stack, pops it onto the redo stack on undo, and
applies the new top snapshot without re-pushing, avoiding the extra duplicate state.
`GraphUiInteraction.actions` now stores `GraphUiAction` values directly; actions carry their `node_id` (with
`NodeSelected` using an `Option<NodeId>`), and a `ZoomPanChanged` action is emitted when pan or zoom changes.
Zoom/pan change detection now uses the `UiEquals` UI tolerance thresholds for scale and pan comparisons.
`GraphUiInteraction` now lives in `editor/src/gui/graph_ui_interaction.rs` with helper methods for actions/errors,
and call sites use `add_action`/`add_error`/`add_node_selected` instead of pushing directly.
`GraphUiInteraction::add_pending_action` now flushes pending actions when mixing action kinds and keeps only the most
recent pending action per kind.
Undo/redo snapshot storage now lives in `editor/src/undo_stack.rs`, with `AppData` delegating stack operations to the
`UndoStack` helper.
Undo snapshots are now LZ4-compressed (`lz4_flex`), storing compressed bytes instead of raw serialized strings.
Undo/redo stacks now store ranges into backing byte buffers (`undo_bytes`/`redo_bytes`) to reduce per-snapshot
allocations.
Undo/redo now truncates backing buffers when popping the most recent snapshot to avoid unbounded growth.
`UndoStack` is now generic over serde types and constructed with a `FileFormat`.
Undo stack tests now validate buffer growth/shrink behavior and redo buffer clearing.
`MainUi::handle_undo_shortcut` now handles redo on Cmd/Ctrl+Shift+Z.
Graph UI `update_zoom_and_pan` now lives on `GraphUi` as a private method.
Graph UI scroll handling now folds smooth scroll + wheel line/page deltas via `collect_scroll_mouse_wheel_deltas`.
Connection drag state now lives inside `ConnectionUi` instead of `GraphUi`.
Connection drag lifecycle now goes through `ConnectionUi` helpers (`start_drag`, `update_drag`, `stop_drag`), and
`ConnectionUi::render` draws the active drag path.
Temp connection endpoints now live on `ConnectionDrag`.
Double-clicking a connection now clears its input binding.
`KeyIndexVec` now implements order-independent `Eq`/`PartialEq` by comparing values by key.
KeyIndexVec equality now has a unit test covering order independence and value changes.
`editor::common::ui_equals` now defines a `UiEquals` trait with `ui_equals` for `f32`, `Vec2`, and `Pos2` using UI tolerance thresholds.
Undo history now uses a `UndoStack` trait with a `FullSerdeUndoStack` implementation that stores full serialized snapshots.
Connection curves now render with a gradient from output port color to input port color, using per-segment strokes.
Connection curves now use sampled polylines instead of `CubicBezierShape`.
Bezier math and intersection helpers now live in `common::bezier::Bezier`, reused by connection rendering.
Connection style now includes a hover stroke for hovered connections.
Connection style now includes `hover_brighten` and `style::brighten` for hover color tuning.
Connection bezier sampling now eases parameterization to add detail near endpoints while reducing mid-curve density.
Connection curve mesh allocation now reserves vertex/index capacity based on bezier point count.
Polyline mesh construction helpers now live in `editor/src/gui/polyline_mesh.rs` with a `PolylineMesh` wrapper used by
connection rendering.
Bezier rendering now uses `common::bezier::Bezier`, which wraps `PolylineMesh` and owns bezier sampling.
`Bezier::show` now returns an egui response so callers can detect hover/click.
`Bezier` now caches its endpoints and scale to skip redundant point rebuilds.
`common::BumpVecDeque` is a bump-backed deque built on `bumpalo::collections::Vec<MaybeUninit<T>>` for arena-friendly
push/pop without std allocator usage.

## Repository Layout

- **common** – shared utilities and helper macros
- **graph** – Rust library defining graphs, nodes and the execution
- **test_resources** – sample graphs and media used by tests
- **test_output** – output folder created by unit tests
- **deprecated_code** – older .NET and QML editors and experimental code

Each directory may contain its own `Cargo.toml` or build scripts.
The root `Cargo.toml` defines the workspace and shared dependencies.
The graph crate exports a `prelude` module that re-exports common graph, data, function, and execution graph types for
easier imports.
The editor now imports `Graph` via `graph::prelude::Graph`.
The editor model exposes `Graph::from_graph` to convert a core graph into the editor view with inferred output counts
and a simple grid layout.
GraphView conversion now calls `Graph::validate()` directly after building the graph.
Execution graph tests now use descriptive names for removed-node scenarios.
Editor view model types are split into `editor/src/model/graph_view.rs` and `editor/src/model/node_view.rs`.
Editor graph serialization now uses `common::FileFormat` (JSON/YAML).
The editor loads `test_func_lib` to name view-node inputs/outputs based on core function definitions.
Editor view tests now derive their graph view from `test_graph` + `test_func_lib` instead of a standalone view fixture.
Execution graph tests now verify that impure functions execute without input changes, pure functions do not, and Output
behaviors panic unless nodes are Terminal.
Execution graph tests include coverage for toggling a Once node back to AsFunction to refresh upstream execution.

Commit messages are often prompts sent to an AI agent to request a change.

## Dependency Versions

As of 2025-12-24, workspace dependencies are set to the latest minor versions with patch versions left open.
The only version bump needed in this update was `wgpu` to `28.0`; other workspace dependencies were already at their
latest minor versions.
The editor crate now relies on `workspace = true` for shared dependencies, and the workspace dependency list includes
`toml`, `rayon`, `dotenv`, and `tracing-rolling-file`.

## Subprojects

### common crate

Provides small utilities such as logging helpers, macros for unique identifier types, and miscellaneous functions.
See `common/src` for implementation details.

Recent adjustments:

- `common/src/scoped_ref.rs` uses generic drop callbacks (no boxing), derives `Debug` on scoped refs, and uses `expect`
  in `Drop` for invariant enforcement.
- `common/src/shared.rs` defines `Shared<T>` as a `Debug` newtype wrapper around `Arc<Mutex<T>>` with convenience
  `lock`, `lock_owned`, `try_lock`, `get_mut`, and `arc` helpers plus `Deref` to the inner `Arc`; it now supports
  `T: ?Sized` and reserves `new`/`Default` for sized types.
- `common/src/lib.rs` now re-exports helpers from module files (`constants`, `debug`, `file_format`, `serde_format`,
  `shared`) to keep the crate root minimal.
- `common/src/bool_ext.rs` adds `BoolExt` with `then_else` and `then_else_with` helpers for conditional value selection.
- Worker execution now skips `execute()` when `ExecutionGraph::update` fails, forwarding the update error directly to
  the compute callback.
- Worker compute callbacks are stored as boxed trait objects to satisfy `Shared<T: Sized>` bounds.
- Function lambdas are async: `FuncLambda` stores `Arc<AsyncLambda>` (a boxed-future closure type alias), and built-in
  invokers use async closures wrapped in `Box::pin(async move { ... })`.
- `DynamicValue::Custom` now requires `Any + Send + Sync` to keep async lambda futures `Send` when borrowing inputs.
- `TestFuncHooks` uses `Arc<dyn Fn...>` so async lambdas can clone hooks into futures without moving them out of the
  closure.
- `graph/src/macros.rs` adds `async_lambda!` to reduce boilerplate for async lambdas, including a binding-list form for
  per-call setup.
- `common/src/key_index_vec.rs` supports key-based index syntax with invariant assertions for missing keys.
- `common/src/key_index_vec.rs` includes reusable compaction helpers driven by a `KeyIndexKey` trait to keep the index
  map in sync.
- `common/src/key_index_vec.rs` skips serializing `idx_by_key` and rebuilds it from `items` during deserialization,
  rejecting duplicate keys.
- `common/src/key_index_vec.rs` includes YAML/JSON/Lua roundtrip tests to ensure deserialization rebuilds index maps
  correctly.
- `common/src/key_index_vec.rs` compact insert tests now assert via returned indices instead of item references.
- `common/src/key_index_vec.rs` keeps `idx_by_key` in sync when removing items, updating shifted indices after removal.
- `common/src/key_index_vec.rs` tests cover compact insert and finish behavior for swap, append, and already-compacted
  paths.
- `common/src/key_index_vec.rs` compaction now uses a constructor callback for new entries instead of default-only
  insertion.
- `common/src/key_index_vec.rs` serializes as a flat array (no wrapper object) and rebuilds the index map from that
  array on load.
- `common/src/macros.rs` `id_type!` now supports `From<&str>`/`From<String>` for UUID IDs, panicking with a clear
  message on invalid strings.
- Removed `common/src/apply.rs` and replaced its usages with standard `Option` methods.
- Switched logging to tracing; `common/src/log_setup.rs` wires console + rolling file output via `tracing-subscriber`
  and `tracing-appender`.
- Execution graph construction now uses a DFS order with cycle detection, input/output binding validation, and
  ID-to-index maps for faster lookups.
- Added `*_ref` accessors on `Graph`/`FuncLib`/`ExecutionGraph` for invariant-driven access (panic on missing IDs).
- Replaced many unwraps with explicit `expect` messages in core execution paths.
- Invoke tests now use Tokio async tests with blocking sections around sync compute execution.
- Removed `*_ref` accessors and standardized on `Option` lookups with explicit `expect` messages at call sites.
- Replaced parking_lot usage with Tokio mutexes and async/try_lock access patterns (no blocking_lock).
- OutputStream now uses async drain via Tokio mpsc; sync writers send without blocking.
- Compute is async; worker and tests await compute instead of blocking.

### graph crate

Implements the data structures for graphs and nodes. Nodes are created from functions defined in a function library.
Connections between nodes are represented by `Binding::Output` values.
Data structures like graphs, function libraries, and execution graphs can be serialized to YAML or JSON.
`StaticValue` implements `Eq` with a manual `PartialEq` that compares float variants by `f64::to_bits` for deterministic
equality.

Execution execution is handled by the `execution_graph` module which determines which nodes should run each tick.
Additional modules drive execution and integration:

- `compute` runs active nodes through `FuncLib` lambdas and converts values between data types.
- Function libraries now own lambda invocations directly; `invoke.rs` and the `Invoker`/`UberInvoker` layer were
  removed.
- Lua function loading now assigns `Func.lambda` directly via `FuncLambda` and asserts input/output counts during
  invocation.
- `Func.lambda` now uses a `FuncLambda` enum with explicit `None`/`Lambda` states instead of `Option`.
- Lua print output now joins multiple arguments with tab separators while preserving type-specific stringification.
- Lua function loading now holds the function map lock once and iterates inputs/outputs with zip-based loops.
- Lua value conversion now panics with the unsupported Lua value details for easier debugging.
- Lua function parsing now caches input/output counts and includes function+index context in data type errors.
- Lua connection collection now takes and drops the mutex guard in a tighter scope.
- Lua graph wiring now preallocates node storage and validates the final graph with an explicit expect.
- Lua tests now assert ordered multi-value returns and reuse a helper for bound-node names.
- Lua tests now cover invoking the Lua `mult` function directly via the function library.
- ComputeError now stores invocation error text as a `String` for clone-friendly errors.
- Compute invocation now records node errors and returns immediately without cloning the full result.
- Execution graph terminal-node discovery now uses a direct loop instead of iterator chaining.
- Execution graph validation now enforces processing/execution order membership and forbids Processing states.
- Execution node input resets now preallocate and extend from function inputs with a compact iterator map.
- Execution graph input-state propagation now caches output-availability once per edge for clarity.
- `worker` spawns a Tokio thread that executes the graph either once or in a loop and processes events.
- Graph file loading now uses `?` to lift serde-format errors into anyhow results.
- `worker` must be shut down via `Worker::exit()`; dropping a running worker triggers a panic to surface logic errors.
- `worker` event loops return `None` when the message channel closes to signal termination.
- `event` manages asynchronous event loops that send event IDs back to the worker.

Benchmarks:

- `graph/benches/b1.rs` resolves `test_resources` via `CARGO_MANIFEST_DIR`, disables caching on the `sum` node, and
  benchmarks `ExecutionGraph::new` with Criterion.

Execution graph construction now uses an explicit stack for active-node ordering to avoid deep recursion limits.
Execution graph creation and scheduling are split into clearer phases (node build, propagation, scheduling).
Execution graph propagation asserts nodes were processed before input state evaluation.
Execution graph input-state propagation now inlines the wants-execute check when storing input state.
Execution graph input-state propagation now uses `BoolExt::then_else` for the wants-execute branch.
Execution graph output usage now uses `BoolExt::then_else` for Skip/Needed selection.
Graph UI uses `BoolExt::then_else` for pointer checks, zoom/pan defaults, and selection strokes.
Timers invoker uses `BoolExt::then_else_with` for frame-event frequency selection.
Execution graph propagation expects function library entries to exist and asserts input index bounds.
Execution graph build validates graph+func-lib alignment once up front and no longer repeats validation checks in each
phase.
Execution graph node collection uses a helper to reuse cached state (invoke cache, output values, binding counts) from
the previous execution.
Execution graph update now uses helpers to reset or build execution nodes without duplicating state initialization
logic.
Execution node reset logic now lives on `ExecutionNode` to keep update behavior self-contained.
Execution node port resets are centralized in `ExecutionNode::reset_ports_from_func`.
Execution graph node cache now creates missing execution nodes when new graph nodes appear, clears cached outputs if
function output arity changes, and rebuilds output binding counts each pass.
Execution graph node cache lookup uses explicit matches to keep the insert path obvious.
Execution graph scheduling asserts when execution node indices are missing.
Execution graph propagation asserts when output bindings reference missing execution nodes.
Execution graph propagation panics with function and node IDs on missing functions.
Execution graph debug assertions include node indices and IDs to speed up diagnosis.
Execution graph visit/output assertions now include index mismatch context for faster debugging.
Execution graph invariant lookups and tests now use `expect` instead of `unwrap_or_else`.
Execution graph binding access now uses a shared expect helper in forward/validation paths.
Execution graph invalidation now walks downstream execution-node bindings to clear init state.
Execution graph invalidation now has unit coverage for dependent node invalidation.
Execution graph invalidation now accepts any iterator of node IDs for batching.
Execution graph invalidation is now exposed as `invalidate_recursively` (typo fixed), with a deprecated alias kept for
callers.
Execution graph invalidation now uses a `Vec<bool>` visited map instead of a `HashSet` for faster dense traversals.
Execution graph execution collects stats via `ExecutionGraph::collect_execution_stats`.
Node UI derives per-node execution info and uses node-style colors for executed/cached/missing-input shadows.
Node UI now renders per-node execution time labels beneath executed nodes.
Node UI shadow styling is defined per state using `egui::Shadow` in node style.
Polyline mesh now owns its connection points buffer as a `Vec<Pos2>`.
Connection UI uses a reusable temporary `PolylineMesh` for drag previews.
Connection UI only rebuilds the drag preview mesh when the drag endpoints change.
Connection UI stores connection endpoints in a shared `ConnectionEndpoints` struct for curves and drag preview.
Connection breaker now reuses `PolylineMesh` for its point storage.
Connection breaker no longer tracks a separate last-point field.
Connection breaker incrementally appends mesh segments as new points arrive.
Polyline mesh no longer exposes `add_curve_to_mesh`; segment appends handle full rebuilds too.
Const input badges are now editable and update their bindings on commit.
Const input edits now render inside a clipped child UI to avoid overflow.
Const input edits now emit `GraphUiAction::InputChanged` when the value changes.
Graph UI top panel renders its buttons in a fixed-position area.
Graph top button row now renders inside a semi-transparent black frame for contrast.
Graph top button row now stretches to the full graph width.
Graph top button row now uses a fixed-width UI scope to enforce full-width layout inside the area.
Graph top buttons now render as fixed-size square buttons using the monospaced font.
Undo stack now stores a per-instance byte limit set via the constructor and drops the oldest entries (plus their
backing bytes) when the limit is exceeded.
Undo stack tests now cover byte-based eviction behavior and assert the limit must be positive.
Undo stack tests now include a two-snapshot budget case.
ConnectionBezier now supports a per-instance style override used during rendering.
Connection UI now rebuilds and renders curves in a single pass instead of a separate rebuild step.
Execution graph debug validation (`validate_with`) now lives in `graph/src/graph.rs` (still on `ExecutionGraph`) to keep
graph-centric validation together.
Graph now owns the execution-input validation helper (`validate_execution_inputs`) used by execution-graph updates.
Execution graph debug validation now checks process/invoke order bounds/uniqueness and uses position maps for dependency
ordering assertions.
Graph now exposes a `dependent_nodes` traversal to gather downstream nodes from a starting node id.
Graph tests now cover `dependent_nodes` traversal ordering and reachability.
Compute now sorts invocations by `ExecutionNode::invocation_order`, which resets to `u64::MAX` each pass and is set
during scheduling.
Compute input value conversion now uses a single loop instead of iterator chains.
Execution graph update/backward/forward comments now describe traversal intent and invariants.
Execution graph update/forward/backward comments now clarify rebuild, binding refresh, and cycle-detection intent.
Execution graph now rebuilds the `e_node_idx_by_id` cache each update and uses it for node lookups.
Execution node cache rebuild now drops execution nodes missing from the current graph.
Execution node cache update compacts in-place with swaps and truncation to minimize allocations.
Execution node cache compaction includes inline comments describing the swap-and-truncate flow.
Graph input binding assignment now uses direct `is_some` checks on `const_value` for clarity.
Execution graph input binding traversal now uses a `let-else` early-continue to reduce nesting in backward1.
Execution graph backward1 now increments output usage counts in one place per visit, while still handling
already-processed nodes.
Execution graph backward2 traversal uses early-continue and `let-else` to flatten execute-path scheduling.
Execution graph forward now expects bound nodes to exist with a clearer message and asserts bound output indices are in
range.
Execution graph forward simplifies bind-update logic by comparing existing bindings against the new execution binding
before assignment.
Function validation now asserts that no-output functions are impure.
`graph::test_graph()` constructs the standard sample graph (fixed IDs, bindings, const inputs) and validates it; tests
now use it directly instead of deserializing a YAML fixture.
`graph::function::test_func_lib()` constructs the standard sample function library in code; tests and benchmarks use it
instead of a YAML fixture.
`graph::common::FileFormat` provides YAML/JSON selection and auto-detects file formats by extension for graph/function
library loading.
Execution graphs now expose YAML/JSON serialization helpers for roundtrip testing.
Zed debug config adds a CodeLLDB launch task that sets an LLDB breakpoint on `rust_panic`.

## Common Terms

Add shared terminology here as the project evolves.

## Editor Workflow

Add editor workflow details here as they are defined.
The node header cache button is disabled for terminal nodes to prevent toggling cache behavior.
Node ports are rendered immediately after each node body to preserve proper z-ordering when nodes overlap.
Node label rendering now happens inside the node body render pass to keep labels ordered with their nodes.
Selecting a node reorders it to the end of the node list so it renders on top of others.
Port hit-testing iterates nodes in reverse draw order and uses strict distance checks to prefer front-node ports on
overlaps.
Temporary connection dragging now renders after nodes so it stays above all nodes and connections.
Lua serialization/deserialization now uses `common::serde_lua` and `FileFormat::Lua` for graph, function library,
execution graph, and editor graph view formats.
File-format serialization and deserialization are centralized in `common::serialize` and `common::deserialize`.
Serialization now uses `common::SerdeFormatError` and `SerdeFormatResult` for format-specific errors.
The editor includes a Run button beneath the graph view that executes compute on the current graph.
Sample test hooks in the editor now populate compute status output via the `print` hook.
The editor now uses a Tokio async main and `std::sync::Mutex` for compute status updates.
Editor UI edits no longer invalidate execution nodes immediately; execution now relies on the worker's fresh
graph/func-lib run.
Editor print output now uses `arc-swap` to store the latest string without locking.
Editor worker construction now lives in `AppData::create_worker` (invoked by `AppData::new`) to keep status update
wiring in one place.
Worker completion/error callbacks now request an egui repaint via a stored `egui::Context` to refresh status output
promptly.
Editor main-window UI rendering now lives in `editor/src/main_window.rs` via `MainWindow::render`, which
`ScenariumApp::update` delegates to.
Editor runtime state for worker/graphs now lives in `AppData` inside `ScenariumApp` to group core app data.
Editor UI state is split: `MainWindow` holds graph UI/interaction context, while `AppData` (in
`editor/src/model/app_data.rs`) holds status text, current path, and other core runtime data.
Main-window construction now creates a `UiRefresh` helper passed into `AppData::new` so worker callbacks can request
egui repaints without holding raw context.
Graph save/load, test-graph construction, graph-UI action handling, and run-graph status handling now live in `AppData`,
with `MainUi` delegating to those helpers and `ViewGraph` focusing on in-memory serialization only.
Node cache toggles now emit `GraphUiAction::CacheToggled` so the UI action stream captures that event.
Editor graph views now store a core `graph::Graph` alongside per-node positions; GUI rendering and edits read/write node
data and bindings directly on the core graph.
Graph connection rendering (curve generation, hit detection, and temporary-connection drawing) now lives in
`editor/src/gui/connection_render.rs` with `graph_ui.rs` focusing on interaction flow.
`GraphLayout::build` now encapsulates layout/width/port collection for the graph UI render pass.
Graph view validation now panics on invalid zoom, pan, node positions, or mismatched node lists instead of returning
errors.
Editor execution invalidation now batches affected node IDs before recursive invalidation.
Editor run status now appends execution stats (node count + elapsed seconds).
Graph UI interaction state now records per-node action types (cache toggle, input change, node removal).
Node headers now show an impure-function status dot for impure funcs.
Graph UI helpers now pass `ViewGraph` explicitly instead of storing it on `GraphContext`.
Node layout sizing now computes row widths inline and reuses cached cache-button metrics to avoid extra allocations.
Node layout translation now happens before struct construction to avoid mutable layout patch-up.
Execution graph tests now assert per-node output usage counts for simple, missing-input, and graph-change scenarios.
Execution graph tests include `none_binding_execute_is_stable` to exercise repeated execution with an unset input
binding.
Execution graph tests include a helper that returns invocation-order node names for assertions.
DynamicValue now owns the type-conversion helper previously in execution_graph.
Execution graph validation now asserts output value cache length and ensures all bound output nodes exist in the
execution node set.
Function async lambda signatures are centralized via `AsyncLambdaFn` to avoid duplication in `graph/src/function.rs`.
Invocation inputs now use `InvokeInput { state, value }`, and output usage is tracked via `OutputUsage` instead of
`OutputRequest`.
`async_lambda!` now only supports the 3-arg and 4-arg forms used in the codebase, dropping the generic fallback arms.
Invoke cache `get_or_default` now resets the stored value to the requested default type if a downcast fails.
`graph/src/context.rs` defines `ContextKind` (built-ins plus `Custom(TypeId)`), `Context`/`ContextFactory`, a
`ContextRegistry` for builders, and `InvokeContext` storage; `Func` now records `required_contexts` (runtime-only,
skipped in serialization).
Context meta constructors now share a `ContextCtor` type alias to avoid repeating the boxed Any factory signature.
`graph/src/context.rs` includes a test that verifies custom contexts are created once and reused via
`ContextManager::get`.
`ContextMeta::new_default` creates a custom context meta for `T: Default` without needing an explicit constructor.
`FuncLambda::invoke` now takes a `ContextManager` so lambdas can access shared invocation contexts; async lambda macros
insert the extra parameter.
`common::is_false` is a shared serde helper for `skip_serializing_if` bool flags (used by `graph::Node.terminal`).
`graph::Input.binding` now skips serialization when `Binding::None`, and `Input.default_value` skips when `None`.
`graph::Node.behavior` now skips serialization when `NodeBehavior::AsFunction` via `NodeBehavior::is_default`.
Editor node rendering now shows const input bindings as small badges to the left of input ports.
Const-binding badges now use a dedicated stroke color in the graph style.
Graph UI style defaults in `editor/src/gui/style.rs` now inline their literal values instead of named constants.
Graph render context now pulls fonts, text color, and port radius from the shared GUI style.
GraphLayout now exposes helpers to detect hovered ports and pointer-over-node states for graph UI interactions.
GraphLayout and port query helpers now live in `editor/src/gui/graph_layout.rs` to keep graph UI rendering focused.
`render_node_bodies` now mutates `GraphUiInteraction` directly and applies selection/removal internally instead of
returning a NodeInteraction bundle.
Graph UI now uses an `InteractionState` enum to represent idle vs. connection drag/breaker modes, and passes
`ConnectionBreaker` as `Option<&ConnectionBreaker>` when active while reusing breaker/drag allocations.
Graph UI now stores a persistent `ConnectionRenderer` whose `render` method rebuilds curves/highlights before drawing.
`ConnectionBreaker` now has a `render` helper to draw its breaker path when active.
`ConnectionBreaker` now lives in `editor/src/gui/connection_breaker.rs` to keep connection rendering helpers separate.
`ConnectionRenderer` now owns its connection drawing logic via a `draw` helper instead of a free function.
`ConnectionRenderer` now collects curves into its reusable `curves` buffer via a method instead of returning a new
vector.
`ConnectionRenderer` now clears and refills its `highlighted` set in-place when evaluating breaker hits.
`KeyIndexVec::remove_by_index` now asserts index/key validity and returns `V` directly to surface logic errors.
`KeyIndexVec` now supports `retain` and `IntoIterator` for borrowed forms, rebuilding its key map after retention.
`KeyIndexVec::push` was renamed to `add`; it now asserts key/index map sync and overwrites existing items when a key
already exists.
`KeyIndexVec` tests cover overwrite behavior when calling `add` with an existing key.
Graph UI now uses a `PrimaryState` enum to model pressed/down/released pointer state instead of separate booleans.
Connection drag drawing now lives on `ConnectionDrag::render` instead of an inline call.
`ConnectionDrag` now lives in `editor/src/gui/connection_drag.rs` to keep graph UI focused on state handling.
Graph UI now stores `ConnectionDrag` as an `Option` and drops it when the drag ends.
Graph layout now preallocates port capacity, centralizes port collection on `GraphLayout`, and uses explicit expects for
missing graph/func data.
GraphLayout now collects ports via a `&mut self` method that uses stored layout fields instead of threading parameters.
Graph UI now keeps a persistent `GraphLayout` and updates it in-place each frame.
Node UI now splits caption-drag handling from node rendering and updates `GraphLayout.node_rects` when dragging.
Mouse wheel zoom now uses signed vertical scroll delta with a small exponential factor for smoother zoom direction.
Graph UI now uses `ui.interact` on node rects to determine pointer-over-node state instead of manual rect contains
checks.
Connection breaker now encapsulates point addition/length limiting logic in `ConnectionBreaker::add_point`.
Breaker max length is now owned by `connection_breaker.rs` rather than `graph_ui.rs`.
Connection hit-testing now precomputes breaker segments and avoids repeated iterator cloning.
Connection breaker now stores segments directly, avoiding point-window allocations for hit testing.
Connection breaker rendering now builds a single polyline for `painter.line` instead of per-segment draws.
Graph UI connection application now returns a `Result` and rejects connections that would introduce cycles.
Execution graph now reuses a cached `processing_stack` for cycle detection instead of allocating per pass.
Graph UI input handling now lives in `GraphUi::process_input`, with rendering split into `GraphUi::render`.
Graph UI error type now implements `Display` so cycle errors surface in the status panel.
Editor visuals now explicitly disable egui debug-on-hover overlays.
`GraphLayout` now stores per-node `NodeLayout` values built by `compute_node_layouts` (no shared base layout), and
layout geometry is derived from local constants inside `compute_node_layout`.

- Added KeyIndexVec::compact_insert_start() with CompactInsert guard that auto-calls compact_finish on drop.

- Added tests for compact_insert_start drop/finish behavior in common/src/key_index_vec.rs.

- ConstBindUi now returns a ConstBindFrame from start(), which owns a CompactInsert guard and handles hover + auto-compact on drop; NodeUi uses the frame for rendering.

- CompactInsert now exposes item_mut() and finish(), and tests cover finish() behavior.

- CompactInsert now implements Index/IndexMut for direct access to items during compaction.

- Added CompactInsert index/index_mut test coverage in common/src/key_index_vec.rs.

- CompactInsert::insert_with now returns (index, &mut item); updated const bind and tests.

- Added CompactInsert index panic test when accessing beyond write_idx.

- CompactInsert now centralizes index validation in validate_index().

- GraphLayout now uses compact_insert_start guard for node_layouts compaction.

- ConnectionUi rebuild now uses compact_insert_start guard for curves.

- ConstBindUi now accepts an optional ConnectionBreaker and highlights const links when breaker segments intersect.

- Bezier now exposes intersects_breaker() and const link break detection uses it.

- Bezier::intersects_breaker now accepts Option<&ConnectionBreaker>.

- ConnectionUi break detection now uses Bezier::intersects_breaker.

- ConnectionUi now exposes broke_iter() for broken connections.

- ConnectionUi broke_iter now derives from curves instead of a HashSet.

- ConstBindUi now exposes broke_iter() over ConstLinkKey and ConstLinkKey is pub(crate).

- Const link mesh build now deduces colors/width once before calling build_mesh.

- ConnectionBezier::show now deduces style internally from hover/broke and gui style; callers pass hover/broke only.

- DragValue now has pos()/align() setters and show() uses stored position/alignment.

- DragValue now supports hover(bool) to control hover cursor; const int uses hover(true).

- NodeStyle now includes DragValueStyle with defaults matching inactive bg and hover stroke.

- Style::new now uses shared color constants to avoid duplicated values.

- Style::new now uses CORNER_RADIUS and SMALL_CORNER_RADIUS constants for corner sizes.

- Style::new now uses DEFAULT_BG_STROKE_WIDTH for background/hover strokes.

- DragValue now uses pointer hover to pick hover stroke when hover is enabled.

- Const bind DragValue now checks breaker intersection with its rect and marks curve as broke.

- ConnectionBreaker now exposes intersects_rect(); const bind uses it.

- Simplified DragValue rect expansion using Rect::expand.
