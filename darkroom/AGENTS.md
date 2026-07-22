# AGENTS.md

This file provides guidance to AI coding agents when working with code in this repository.

`darkroom` is the node-graph editor, built on Aperture (the in-tree
immediate-mode GUI lib at `../aperture`). Root `../AGENTS.md` holds the
workspace-wide rules (workflow, Rust style, tooling); Aperture's own
widget/id conventions live in `../aperture/AGENTS.md`. This file covers only
what's specific to darkroom.

## Commands

```
cargo run -p darkroom                      # launch the editor (opens last doc)
cargo test -p darkroom              # tests (mostly pure: zoom math, breaker geometry, serde round-trips)
cargo test -p darkroom <substr>     # single test by name substring
cargo clippy -p darkroom --all-targets -- -D warnings
cargo run -p darkroom --features profile-with-tracy   # tracy zones across darkroom + aperture
```

The `ayu_graphite_asset_in_sync` test regenerates `assets/ayu-graphite.toml`
from the code-defined theme consts on every test run, so after changing the
default look just run the tests and commit the asset diff (see `theme.rs`).

## Dependencies / boundaries

- **`scenarium`** — headless core. Owns `Graph`, `Node`, `Binding`, `Library`,
  `StaticValue`, the headless `Worker` evaluator, serde formats.
  darkroom never reimplements graph semantics; it edits a `scenarium::Graph`,
  resolves nodes against a `Library`, and runs the graph through `Worker`.
- **`aperture`** — the GUI runtime. `App` implements once-only
  `aperture::App::update` plus replayable `aperture::App::record`; `WinitHost`
  (in `main.rs`) drives both. All widgets, input, layout, theming, texture
  upload come from here. Pre-1.0, breaks freely — coordinate changes with
  aperture.
- **`common`** — `SerdeFormat`, `serialize`/`deserialize`, and shared utilities.
- **`lens`** — application node libraries: filesystem watching, random values,
  image operations, and astro processing.
- **`tokio`** — multi-thread runtime backing the execution worker (graph runs
  off the UI thread; results drain back on-frame).

## Module layout (`src/`)

Root holds the entry point; implementation is grouped by responsibility:

- **`main.rs`** — module decls + `WinitHost` bootstrap.
- **`gui/scene.rs`** — the render projection (see below).
- **`gui/theme.rs`** — the visual/layout `Theme` bundle, code-defined (see below).
- **`gui/run_state.rs`** — centralized runtime state: per-node execution
  status/logs and the latest worker-pushed pinned-output values.
- **`gui/image_viewer.rs`** — render-side full-image conversion, textures,
  and pan/zoom state; source values are borrowed from `RunState` each frame.
- **`gui/app/`** — `mod.rs` (the `App`: GUI policy + per-frame entry +
  `AppContext`), `editor/` (the `Editor`: undo + scene + UI tree +
  the edit pipeline; `shortcuts.rs` maps chords → intents/commands),
  `commands/`
  (`AppCommand` side effects, run *outside* the record — grouped into nested
  sub-enums with one handler submodule each: `file` / `graph` / `run` /
  `prefs` / `edit` / `shell`; `mod.rs` is the dispatcher).
- **`core/worker.rs`** — `WorkerBridge`: tokio worker + result channel.
- **`core/workspace/`** — `Workspace`: the shared `OpenDocument` +
  `RuntimeHost` pairing and run/save/cache coordination.
- **`core/runtime_host.rs`** — `RuntimeHost`: runtime library, compiler,
  worker, scripting host, and status log; no document or frontend policy.
- **`core/graph_library.rs`** — `GraphLibrary`: the user-owned reusable graph
  definitions held privately by `RuntimeLibrary`.
- **`core/runtime_library/`** — `RuntimeLibrary`: the ephemeral merged
  Scenarium registry (built-ins, configured ML defaults, and its graph-library
  definitions), graph-library persistence API, and published snapshots for
  scripts and workers.
- **`core/terminal_session/`** — terminal/headless event interpretation and
  shutdown state over a `Workspace`.
- **`core/document/`** — `mod.rs` (the `Document` model + `GraphRef` / `GraphView` /
  `EditScope`), `open_document.rs` (`OpenDocument`: startup loading, active
  path, and pending normalization), `serde.rs` (custom ordered paint-stack
  wire format), and `validate.rs` (document/view structural validation).
- **`core/edit/`** — the mutation machinery: `intent/` (intents + undo steps),
  `action_stack/` (packed undo history), and `publish.rs` (local/shared graph
  publication).
- **`core/io/`** — `document.rs` (`.darkroom` ZIP containing `document.json`),
  `graph_template/` (reusable graph-template serde I/O), `graph_library/`
  (`darkroom.graph-library.json` loading, quarantine, and durable writes),
  `preferences.rs` (`Preferences` session state), and `cache.rs`
  (per-document disk-cache root: `<stem>.darkroom-cache/` beside the file,
  with a self-ignoring `.gitignore`).
- **`gui/`** — the UI tree: `canvas/` (the graph canvas + its gestures/
  overlays/inspectors), `node/` (the node-body widget cluster), `dock/`
  (the dock's whole GUI half behind the two-call `DockUi` — pane-tree
  rendering, per-group strips, divider resize, drag-docking), `widgets/`
  (reusable widgets like inline-rename), plus `main_window`, `menu_bar`
  chrome.

## Architecture: Workspace, App, and Editor

`Workspace` is the shared document/runtime owner used by every frontend.
`App` (`src/gui/app/mod.rs`) adds GUI lifecycle policy; `Editor`
(`src/gui/app/editor/mod.rs`) borrows `Workspace.open` for GUI editing. `App` holds:

- `workspace: Workspace` — the open document plus runtime services.
- `editor: Editor` — undo, scene/run projections, gestures, and the GUI tree.
- `theme: Theme`, `preferences: Preferences`.
- `host_handle: HostHandle` — winit integration for file dialogs + repaints.

`App::update` runs once before recording and wires runtime effects to the
editor:

1. **drain worker events** (`drain_worker_events`) — fold progress/stats and
   pinned-output pushes into `editor.run_state`.
2. **handle script inbound** (`handle_script_inbound`) — apply queued graph
   edits and run/quit requests before rebuilding the scene.
3. **handle close request** (`handle_close_request`) — persist window state and
   raise the unsaved-changes prompt before the replayable phase.

`App::record` may replay, but Aperture exposes action input only to the first
real pass. It runs `Editor::frame`, handles its action-derived `AppCommand`
after authoring has released application borrows, submits dirty caches, and
renders and resolves the exit dialog. Unconditional work stays in `update`.

`AppContext<'a>` (`gui/app/mod.rs`) threads `&Theme`, `&Library`, and `&RunState`
down the UI tree so child widgets don't grow a parameter fan-out.

## Architecture: the per-frame edit pipeline (`Editor::frame`)

darkroom is immediate-mode but routes **all** graph mutations through an
intent/undo layer rather than mutating the document inline. The frame splits
into a **navigation phase** (settle *which* graph is active) and an **edit
phase** (mutate that graph), because input that switches tabs/opens graphs
comes from *last* frame's click responses and must resolve before anything
edits or records. `Editor` owns the GUI pipeline state while borrowing the
workspace's `OpenDocument`:

- `action_stack: ActionStack`, `scene: Scene`,
  `main_window: MainWindow`, `run_state: RunState`.
- `scene_target: Option<GraphRef>` (detects tab change), `scene_dirty`, and
  `needs_relayout` flags. Structural edits set
  `OpenDocument.normalization_pending`.
- `intents: Vec<Intent>` and `actions: Vec<UiAction>` — reused scratch buffers,
  cleared each record pass (no cross-frame state).

One record pass:

1. **clear scratch** — `intents` + `actions`.
2. **navigate** — apply keyboard undo/redo (which can replay a dock-layout
   change), then surface tab activate/close + graph-open/new clicks off
   last frame's responses as `UiAction`s. Open mutates the layout directly;
   activate/close queue undoable `Intent::Dock` ops. After this the active
   target is fixed for the rest of the frame.
3. **sync_target** — if the active graph changed since last frame, drop
   transient gesture state and flag a relayout. Does not rebuild.
4. **rebuild #1 (pre-prepass)** — `rebuild_scene(ui, target)`,
   **unconditional**, because `Scene` re-interns names into aperture's active
   record-pass text arena and refreshes the graph projection. Clears
   `scene_dirty`.
5. **edit prepass** — read aperture's *current* input state (drag deltas,
   pan/zoom, connection release) and push `Intent`s. No drawing.
   Layout-changing edits (node drag, connection commit) are emitted here so
   they apply *before* the record.
6. **drain (pre-record)** + canvas shortcuts + file-op chords. The next scene
   rebuild normalizes the open document when structural edits made it pending;
   the drain sets `scene_dirty` if it applied anything.
7. **rebuild #2 (pre-record)** — `rebuild_scene` again **only if `scene_dirty`**.
   An idle frame or bare tab switch skips it.
8. **record** — draw the tree; widgets push more intents (clicks, edit commits,
   inspector chip toggles) and a `MenuCommand` may surface.
9. **drain (post-record)** + relayout request as needed.

### Source of truth: `Document` (`src/core/document/mod.rs`)
The serialized, undoable unit. The graph *data* is one `scenarium::Graph`
(`graph`), which already nests local graphs recursively.
Everything else is editor view-state, split per graph:

- **`GraphRef`** — `Main` (root graph) or `Local(GraphId)` (a local graph).
  The active-graph handle threaded through the whole edit pipeline.
- **`GraphView`** — per-graph view metadata: `item_placements`
  (`IndexMap<ItemRef, Vec2>` of node-body and pinned-output preview positions
  whose *order* is the shared paint stack — later items
  draw in front, `Intent::Raise` lifts either kind; `check()` asserts one
  `Node` item per graph node and one `Pin` item per pinned output),
  `viewport`, and `selected` (a `BTreeSet` so equality/serde are
  order-independent). Root lives in `main_view`; each opened graph in
  `local_views` (lazily seeded + auto-laid-out on first open). **All of this
  is persisted and undoable by design** — reopening restores camera +
  selection, and Ctrl+Z walks them alongside structural edits.
- **`layout: DockLayout`** (`src/document/dock.rs`) — the pane arrangement:
  a binary split tree stored as a flat, canonically pre-ordered
  `Vec<DockNode>` whose leaves are `TabGroup`s (tabs + per-group active),
  plus the focused group. The *primary* group holds the `Main` graph tab
  (successor of the old "tabs[0] is Main"); graph tabs are pinned there —
  one canvas — while viewer/preferences tabs split into their own panes
  (right-click a chip → "Split right/down", capped at 4 nested splits).
  Splits are addressed by `DockPath` (turns from the root packed into one
  byte). Also persisted; every layout mutation is an undoable
  `Intent::Dock`.
- **`EditScope` / `EditScopeRef`** — graph+view borrowed *together* for a
  target, so an edit touches both atomically. Get them via
  `Document::scope_mut(target)` / `scope(target)`.

`Library` is *not* here — `RuntimeLibrary` privately owns the persistent
`GraphLibrary` plus the current and published merged snapshots; `RuntimeHost`
coordinates that single library entity with the worker and script host.
Startup seeds an empty
graph (`auto_layout_default`); there is no checked-in sample graph.

### Intent / undo layer (`src/edit/intent.rs`, `src/edit/action_stack/`)
- Every mutation is scoped to a `GraphRef` target. `Intent` = forward-only
  "set X to Y". `build_step(intent, &doc, target)` reads the pre-mutation
  snapshot and folds both halves into one self-contained `UndoStep`;
  `apply_step`/`revert_step` write the "to"/"from" halves against `target`.
  `Intent::Dock` is graph-agnostic and special-cased ahead of the scope
  lookup: `build_step` snapshots the whole `DockLayout` before/after the op
  into one `DocStep::Dock { from, to }` (the tree is tiny), so every layout
  mutation — activate, close, move/split, divider resize — is uniformly
  reversible by assignment, and refused/degenerate ops fall out as `from ==
  to` no-ops. Adding a variant touches ~6 spots — the doc comment lists them.
- Variants: `AddNode`, `DuplicateNodes`, `RemoveNode`, `MoveSelection`,
  `RenameNode`, `SetInput`, `SetSelection`, `Raise` (either kind of
  paint-stack item — node body or pin preview), `SetNodeProperty`
  (a `NodeProperty::Disabled`/`Cache` — one intent backs both scalar toggles),
  `SetSubscription` (`subscribe: bool` — one intent backs subscribe + unsubscribe),
  `DetachGraph`, `SetViewport`, `RenameBoundaryPort`, `RenameGraph`,
  plus document-global `Dock(DockOp)` (`ActivateTab` / `CloseTab` /
  `MoveTab` / `SetRatio` — activations coalesce as a switch burst, one
  divider's drag coalesces per `DockPath`).
- `DuplicateNodes` is assembled from the current selection by
  `intent::build_duplicate_intent` (free fn next to `build_step`, not a
  `Document` method — `Document` is the persisted model, intent
  construction lives in `edit/`).
- `ActionStack` packs history into two flat byte buffers (bitcode), not a
  `Vec<Vec<UndoStep>>`. Each batch records its `target` so undo/redo re-resolve
  the right graph+view. Consecutive same-`GestureKey` *and* same-target steps
  coalesce in place (a node drag = many `MoveNodes` intents → one undo entry).
- **`UiAction`** (`gui/mod.rs`) is the navigation-request transport from the
  UI layer to `Editor`: `ActivateTab`/`CloseTab` (group-keyed) become
  undoable `Intent::Dock` ops. `OpenGraph`/`NewGraph`/`OpenImageViewer`
  add the tab via `DockLayout::find_or_insert` (graph tabs into the primary
  group, others into the focused one) — that part isn't undoable — but
  focus the tab through the same recorded activation, so undo faithfully
  reverses focus while leaving the opened tab in place.
- Per-step properties (`is_noop`, `requires_relayout`, `requires_reconcile`,
  `gesture_key`, `coalesce`) are methods on `UndoStep`, exhaustive over the
  variants so a new one won't compile until it declares its behavior.
- Every variant is emitted by some UI (node-title rename →
  `RenameNode`, tab-strip rename → `RenameGraph`, etc.). Promote/publish/
  export resolution (`graph_template_to_export` / `promote_to_graph_library` /
  `publish_local_graph`) is pure document↔library logic in
  `core/edit/publish.rs` (unit-tested against bare types, `&mut GraphLibrary`
  in / `bool` changed out), not on `Document`; `app/commands/graph.rs` is the
  thin GUI orchestration (dialogs + dirty flag), running the mutators through
  `RuntimeHost::edit_graph_library`. `RuntimeLibrary` persists the edit,
  recomposes, and publishes the merged registry; `RuntimeHost`
  reports the persistence outcome and re-pushes the worker's `DiskStore` (its
  codec table rides a library snapshot).

### Graph normalization (`scenarium::Graph::normalize`)
Derived graph state lives in Scenarium. `OpenDocument::normalize` gates the
operation on its pending flag before scene rebuild, execution, cache save, or
document save. `Graph::normalize` recursively synchronizes each local graph's
interface against its interior wiring, compacts unused boundary slots, remaps
the owning graph's instance bindings, then prunes bindings and subscriptions
left dangling against the resulting interfaces and current library. It is
idempotent on an already-canonical graph tree.

### Render projection: `Scene` (`src/scene.rs`)
A flat, per-record snapshot rebuilt from the *active* graph+view
(`Scene::rebuild(ui, graph, view, library, run_state)` — see
`Editor::rebuild_scene`). Names are `InternedStr` handles into aperture's
active text arena, so the rebuild both refreshes the projection and allows the
previous arena to recycle. Port names, types, and input-binding snapshots are
flattened into pooled `Vec`s sliced per node (zero per-node allocation in
steady state). Each `SceneNode` carries its
`exec_status` (copied from `run_state`) to drive the status glow + run-time
label. Scene is read-only mirror state — viewport/selection are copied *from*
the active `GraphView` each rebuild; the gesture writes back via intents.

### Graph execution: worker + run state (`core/worker.rs`, `gui/run_state.rs`)
Execution is **decoupled from the UI thread**. `WorkerBridge` owns a tokio
multi-thread `Runtime`, scenarium's headless `Worker`, and an mpsc channel:

- `App::run_graph` asks `Workspace::run_once` to normalize and compile the
  active graph against the library **on the UI thread**
  (`RuntimeHost::run_once` → the host-owned long-lived
  `scenarium::execution::compile::Compiler`) and sends the
  `Arc<CompiledGraph>` to the worker, followed by a separate
  `Run { RunSeeds::sinks() }` command. A compile error surfaces synchronously —
  no run starts, `begin_run` is skipped, and the worker's prior program is
  untouched. The FIFO worker first reports `WorkerReport::Installed` with that
  exact shared compile, then streams its progress, pinned outputs, and final
  result in order.
  `RunState` retains the acknowledged compile and uses it to project flat result
  ids onto authoring nodes. `WorkerBridge::deliver` forwards reports to its
  channel and pokes `host.request_repaint()`.
- **Run to a node** (`App::run_node`, `RunCommand::Node`): after installation,
  the host resolves the root node's `NodeAddress` through that compile, then a
  separate `RunSeeds::nodes` command seeds the exact `ExecutionNodeId`, with its
  outputs delivered for the preview fetch. Local definition tabs cannot supply
  an enclosing instance path, so they expose no Run Node action. Two
  triggers, both gated on `SceneNode::runnable` (instance/boundary/missing nodes
  don't resolve as seeds; local tabs lack an execution address). The disable
  toggle remains available on executable sink kinds independently, and
  Scenarium treats an explicitly seeded disabled sink as enabled for that run:
  the header's play chip left of the
  title (drawn in `gui/node/header.rs`, click scanned by `emit_play_clicks` and
  translated at canvas level) and the node context menu's "Run to this node".
- **Per-document disk cache.** `Workspace` binds `RuntimeHost` to the
  `OpenDocument` path during construction, replacement, and save. The host
  sends `WorkerMessage::SetDiskCache` pointing it at `io::cache`'s
  `<stem>.darkroom-cache/` store. An unsaved doc stays
  memory-only. So a node toggled to `CachePersistence::Disk` (header `C` chip)
  reloads its output across sessions from a store beside the project file.
- On-thread, `App::update` drains the channel (`worker.drain()`, non-blocking).
  `WorkerReport::Progress` → `RunState::apply_progress` marks the active node
  `ExecStatus::Running(Instant)` (purple glow) live — carrying the start instant
  so the node header shows a `aperture::Spinner` + live elapsed-so-far
  (`App::record` repaints ~20fps while `run_state.is_running()`);
  `WorkerReport::Finished` → `set_results`
  folds the final `ExecutionStats` (including nested-graph attribution) onto
  authoring nodes: per-node `ExecStatus`
  (`None`/`Cached`/`Executed(secs)`/`Running`/`MissingInputs`/`Errored`) + logs.
- **Cancel** (coarse): **Run ▸ Cancel Run** shows while `run_state.is_running()`
  (a `running` flag, set by `begin_run`, cleared by `set_results`); it routes
  `MenuCommand::CancelRun` → `RuntimeHost::cancel_run` → `WorkerBridge::cancel_run` →
  `Worker::request_cancel` (a shared `common::CancelToken` the executor polls
  between nodes). The
  in-flight node still finishes (its blocking work isn't interrupted — that's
  P3), but nothing further runs and the run reports `stats.cancelled`.
- **Status, logs, and pinned values persist across re-runs** so the UI doesn't
  blank during compute; fresh stats and pushes replace them as they arrive.
- **Pinned outputs are centralized**: `WorkerEvent::PinnedOutputs` is stored
  directly in `RunState`, with an eagerly prepared thumbnail and a monotonic
  revision. Pin previews and image viewers read it during rendering and cache
  only derived textures/navigation state. `WorkerBridge::deliver` requests the
  frame that drains each report, so the editor never notifies individual views.

### GUI tree (`src/gui/`)
Top level is the chrome: `MainWindow` (menu-bar band + status bar around
the dock; its one real job is the `content` closure saying what each tab
kind looks like) and `menu_bar` (returns `MenuCommand`s). Everything
pane-shaped lives in `gui/dock/` behind `DockUi`, integrated in exactly
two calls: `scan` in the navigation phase (tab activate/close clicks +
the drag-docking lifecycle, off last frame's responses) and `render` in
the record (the recursive dock-tree walk — splits as aperture `Splitter`s
whose ratio drags surface as `DockOp::SetRatio`, groups as
strip-over-content panes — plus the drag's drop-zone highlight, ghost
chip, and grabbing cursor). `dock/strip.rs` is the chip row (close
buttons, inline graph rename, the right-click split menu; the focused
group's active tab wears the full accent cap); `dock/drag.rs` is the
gesture state + the pure pointer→drop-zone classification. The rest:

- **`gui/canvas/`** — `mod.rs` is `GraphUI`, the canvas scope. It separates
  **persistent** state (`background` dotted backdrop, the `CanvasGeometry`
  cache, `inspectors` open-panel set — survive tab switches) from a resettable
  `Gestures` bundle: `NodeUI` (node bodies + drag), `ConnectionUI` (wires +
  in-flight drag + snap), `BreakerUI` (RMB/Cmd+LMB scribble that severs wires /
  deletes nodes), `NewNodeUi` (right-click spawn popup), `GraphMenuUi`
  (RMB context menu on graph-instance nodes), `SelectionUI` (rubber-band),
  and the pan anchor. `pan_zoom` holds the viewport gesture + zoom math
  (unit-tested). `cull.rs` is record-time viewport culling (unit-tested): only
  nodes and wires intersecting the visible world rect are recorded (off-screen
  ones cost no measure/paint). Safe because every node-subtree widget id
  derives from the `NodeId`; a node whose subtree holds keyboard focus
  (aperture's `Ui::focus_within`) is exempt so aperture's state sweep can't
  drop a mid-edit draft. Culled nodes keep their
  world extents via `CanvasGeometry::node_world_rect` (current position + a
  cross-frame size cache beside the port-offset cache), which also feeds the
  rubber-band hit test and inspector-panel anchoring.
- **`gui/canvas/inspector.rs`** — floating per-node inspection panels.
  `Inspectors` keeps a `NodeId → InspectMode` map (`Open` = transient, closes
  on any outside action; `Pinned` = sticky). The header `i` chip cycles
  `Closed → Open → Pinned → Closed`. Panels render as children of the inner
  (transformed) canvas so they pan/zoom with the graph, only for nodes present
  in the current scene. Sections: identity, status (outcome + elapsed),
  inputs (live values when fetched, else static bindings), outputs, log tail.
- **`gui/node/`** — the node-body widget: `mod.rs` is `NodeUI` (node bodies +
  drag; emits `MoveNodes`, graph-open requests, port-disconnect
  double-clicks), with sub-widgets `header` (play chip + title +
  `G`/`■`/`D`/`R`/`↓`/`i` badges: run-to-node / graph / sink / sink-disable /
  RAM-cache / disk-cache / inspect; the `D` control appears only on runnable
  sinks, and the
  `R` and `↓` chips flip the two bits of `Node::cache` (`CacheMode`
  `None`/`Ram`/`Disk`/`Both`) via `SetCacheMode`), `port_row` (the two port
  columns + circles + binding menu; a required input's port paints in the
  missing/warning color only once a run flagged its node `MissingInputs` —
  `SceneInput::required` + `node.exec_status` + `exec_missing_glow` — so the
  port keeps its data-type color while editing), `port_rename` (inline
  boundary-port rename in graph
  interiors), and `value_editor` (inline `Const` editing; an input with
  `value_variants` renders a preset dropdown over them regardless of type — carried
  on the flat `Scene::value_variants_pool` sliced per input by
  `SceneInput::value_variants`).
- **`gui/widgets/`** — reusable widgets. `inline_rename.rs` is a label that
  swaps to a `TextEdit` on double-click (used by the node title and
  boundary-port names).

Key cross-cutting mechanisms:
- **Deterministic widget ids.** Node/port widgets derive their `WidgetId` from
  domain coords (`node_widget_id(id)`, `port_circle_wid(PortRef)`) so any pass
  can `ui.response_for(id)` without threading a cache. The canvas uses
  `WidgetId::auto_stable()`.
- **`CanvasGeometry`** (`gui/canvas/geometry.rs`) — the canvas's
  response-derived geometry: per-frame snapshots of each port glyph's
  geometry/hover/drag state plus the cross-frame node-size cache
  (`node_sizes`), polled from last frame's responses in one pass.
  **Rebuilt in `prepass` and reused in `frame`** (the connection commit reads
  it); the order is an invariant enforced by call sequence. Port centers are
  recomputed from this frame's `node.pos` + cached intra-node offset so a
  just-dragged node's wires anchor correctly without a stale frame.
- **`BreakerProbe`** threads the active breaker state into node/connection
  draws so intersection tests run inline; hits drain into intents on release.

### Persistence + libraries (`src/core/io/`, `src/core/graph_library.rs`)
`document.rs` is pure path⇄document I/O, no `App`/undo/preferences coupling —
`commands/file.rs` orchestrates. A `.darkroom` project is a ZIP archive with one
pretty-printed `document.json` entry. `io/graph_template/` separately handles
multi-format graph-template import/export. `RuntimeLibrary` privately owns the
reusable `GraphLibrary` definitions persisted by `io/graph_library/` in
`darkroom.graph-library.json`; promote, publish, and explicit template import
use its graph-library edit API. It rebuilds the merged Scenarium library from
built-ins, ML model defaults, and those definitions after every change. Local
graphs track lineage via an `origin` field — "Publish" updates the linked
graph-library entry in place, else creates a new one.

`Preferences` (`darkroom.preferences.toml` in cwd) persists last-theme-name +
last-document so the next launch reopens where you left off. Failures degrade
rather than crash and report through `core/status.rs`'s `StatusLog`, the
user-facing outcome log owned by `RuntimeHost` and shared by every frontend (no
bare `eprintln!` anywhere in darkroom; every entry also goes to `tracing`).
It keeps a capped rolling history — the TUI `status` command renders it —
plus a sticky `error` slot holding the last failure
(`StatusLog::report_error`), shown error-colored in the GUI's bottom status
bar until a subsequent success of the same family (a run kick, a finished
run, a file op) assigns `None`. Compile failures report themselves from
`RuntimeHost::compile`; frontends report their own outcomes (run results, file
ops, graph ops).

### Theme (`src/theme.rs`)
The look is **code-defined**, not embedded TOML. Module consts hold every
color (`CANVAS_BG`, `NODE_FILL`, `BADGE_GRAPH`, `EXEC_EXECUTED_GLOW`,
`INPUT_PORT`, …) and layout dimension (`NODE_MIN_WIDTH`, `PORT_SIZE`,
`CANVAS_DOT_SPACING`, …). `Theme::default()` assembles them. `Theme` bundles
darkroom's own fields *and* the nested `aperture::Theme` (scalar fields first,
the aperture table last — a TOML serialization ordering requirement), so it's a
complete bundle serialized as TOML. The checked-in `assets/ayu-graphite.toml`
is a reference/round-trip fixture kept in sync by a test, **not** a
parallel source of truth.

**Colors come from the palette.** Values trace back to the semantic Ayu Mirage
High Contrast palette (backgrounds, borders, text, accent/status, syntax). When
adding or restyling a field, reuse an existing palette swatch rather than
inventing a hex value, so a palette re-seed propagates cleanly. The
`MenuCommand::InvertColors` command is a reversible light-theme stub that flips
every color in place.
</content>
