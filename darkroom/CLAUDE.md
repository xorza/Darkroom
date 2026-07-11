# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`darkroom` is the node-graph editor, built on Aperture (the in-tree
immediate-mode GUI lib at `../aperture`). Root `../CLAUDE.md` holds the
workspace-wide rules (workflow, Rust style, tooling); Aperture's own
widget/id conventions live in `../aperture/CLAUDE.md`. This file covers only
what's specific to darkroom.

## Commands

```
cargo run -p darkroom                      # launch the editor (opens last doc)
cargo test -p darkroom              # tests (mostly pure: zoom math, breaker geometry, serde round-trips)
cargo test -p darkroom <substr>     # single test by name substring
cargo clippy -p darkroom --all-targets -- -D warnings
cargo run -p darkroom --features profile-with-tracy   # tracy zones across darkroom + aperture
```

Run the ignored one-shot asset generator after changing the default look:
`cargo test -p darkroom ayu_graphite_asset_in_sync -- --ignored` (regenerates
`assets/ayu-graphite.toml` from the code-defined theme consts; see `theme.rs`).

## Dependencies / boundaries

- **`scenarium`** — headless core. Owns `Graph`, `Node`, `Binding`, `Library`,
  `SubgraphDef`, `StaticValue`, the headless `Worker` evaluator, serde formats.
  darkroom never reimplements graph semantics; it edits a `scenarium::Graph`,
  resolves nodes against a `Library`, and runs the graph through `Worker`.
- **`aperture`** — the GUI runtime. `App` implements `aperture::App::frame`;
  `WinitHost` (in `main.rs`) drives it. All widgets, input, layout, theming,
  texture upload come from here. Pre-1.0, breaks freely — coordinate changes
  with aperture.
- **`common`** — `SerdeFormat`, `serialize`/`deserialize`, `KeyIndexVec`.
- **`lens`** — `image_library()` / `astro_library()` (image + astro node libraries).
- **`tokio`** — multi-thread runtime backing the execution worker (graph runs
  off the UI thread; results drain back on-frame).

## Module layout (`src/`)

Root holds the entry point plus the central projection/theme/run-state types;
everything else is grouped by responsibility:

- **`main.rs`** — module decls + `WinitHost` bootstrap.
- **`scene.rs`** — the render projection (see below).
- **`theme.rs`** — the visual/layout `Theme` bundle, code-defined (see below).
- **`run_state.rs`** — per-node execution status + logs + on-demand runtime
  values (`RunState`, `NodeRunState`, `ExecStatus`, `RunId`).
- **`node_values.rs`** — render-side value views: formats worker-returned
  values to text and uploads image previews as aperture textures
  (`NodeValueView`, `PortValueView`).
- **`app/`** — `mod.rs` (the `App`: runtime owner + per-frame entry +
  `AppContext`), `editor/` (the `Editor`: document + undo + scene + UI tree +
  the edit pipeline; `shortcuts.rs` maps chords → intents/commands),
  `worker.rs` (`WorkerBridge`: tokio worker + result channel), `commands/`
  (`AppCommand` side effects, run *outside* the record — grouped into nested
  sub-enums with one handler submodule each: `file` / `subgraph` / `run` /
  `prefs` / `edit` / `shell`; `mod.rs` is the dispatcher).
- **`document/`** — `mod.rs` (the `Document` model + `GraphRef` / `GraphView` /
  `EditScope`), `view_node.rs` (per-node position record).
- **`edit/`** — the mutation machinery: `intent.rs` (intents + undo steps),
  `action_stack/` (packed undo history), `reconcile/` (derived subgraph-
  interface reconciliation).
- **`io/`** — `persistence.rs` (file-dialog + serde I/O), `preferences.rs`
  (`Preferences` session state), `library.rs` (shared subgraph library file),
  `cache.rs` (per-document disk-cache root: `<stem>.darkroom-cache/` beside the
  file, with a self-ignoring `.gitignore`).
- **`gui/`** — the UI tree: `canvas/` (the graph canvas + its gestures/
  overlays/inspectors), `node/` (the node-body widget cluster), `dock/`
  (the dock's whole GUI half behind the two-call `DockUi` — pane-tree
  rendering, per-group strips, divider resize, drag-docking), `widgets/`
  (reusable widgets like inline-rename), plus `main_window`, `menu_bar`
  chrome.

## Architecture: App vs Editor split

`App` (`src/app/mod.rs`) is the **runtime owner**; `Editor`
(`src/app/editor/mod.rs`) is the **document + editing pipeline**. `App` holds:

- `editor: Editor` — everything document-related and the per-frame pipeline.
- `library: Arc<Library>` — shared runtime library (builtins + loaded library
  subgraph defs), built at startup.
- `theme: Theme`, `preferences: Preferences`, `current_path: Option<PathBuf>`.
- `host_handle: HostHandle` — winit integration for file dialogs + repaints.
- `worker: WorkerBridge` — drives the headless graph-execution worker.

`App::frame` is thin — it wires the runtime to the editor each frame:

1. **drain worker events** (`drain_worker_events`) — pull `ExecutionStats` and
   `ArgumentValues` off the worker channel into `editor.run_state`.
2. **editor frame** (`editor.frame`) — the full edit pipeline; returns an
   optional `MenuCommand`.
3. **request watched values** (`request_watched_values`) — drain the run
   state's per-frame watch registry (open inspector panels + image-viewer
   tabs registered their nodes during the editor frame) into worker value
   requests (deduped per run epoch).
4. **handle menu command** (`handle_menu_command`) — file/theme dialogs +
   `Run` execute *last*, outside the record, so the blocking dialog holds no
   frame borrows. `Run` calls `App::run_graph`.

`AppContext<'a>` (`app/mod.rs`) threads `&Theme`, `&Library`, and `&RunState`
down the UI tree so child widgets don't grow a parameter fan-out.

## Architecture: the per-frame edit pipeline (`Editor::frame`)

darkroom is immediate-mode but routes **all** graph mutations through an
intent/undo layer rather than mutating the document inline. The frame splits
into a **navigation phase** (settle *which* graph is active) and an **edit
phase** (mutate that graph), because input that switches tabs/opens subgraphs
comes from *last* frame's click responses and must resolve before anything
edits or records. `Editor` owns the pipeline state:

- `document: Document`, `action_stack: ActionStack`, `scene: Scene`,
  `main_window: MainWindow`, `run_state: RunState`.
- `scene_target: Option<GraphRef>` (detects tab change), `scene_dirty`,
  `needs_reconcile`, `needs_relayout` flags.
- `intents: Vec<Intent>` and `actions: Vec<UiAction>` — reused scratch buffers,
  cleared each frame (no cross-frame state).

One frame:

1. **clear scratch** — `intents` + `actions`.
2. **navigate** — apply keyboard undo/redo (which can replay a dock-layout
   change), then surface tab activate/close + subgraph-open/new clicks off
   last frame's responses as `UiAction`s. Open mutates the layout directly;
   activate/close queue undoable `Intent::Dock` ops. After this the active
   target is fixed for the rest of the frame.
3. **sync_target** — if the active graph changed since last frame, drop
   transient gesture state and flag a relayout. Does not rebuild.
4. **rebuild #1 (pre-prepass)** — `rebuild_scene(target)`, **unconditional**,
   because `Scene` re-interns port names into aperture's per-frame text arena
   (cleared each `Ui::frame`), so the projection must be regenerated every
   frame regardless. Clears `scene_dirty`.
5. **edit prepass** — read aperture's *current* input state (drag deltas,
   pan/zoom, connection release) and push `Intent`s. No drawing.
   Layout-changing edits (node drag, connection commit) are emitted here so
   they apply *before* the record.
6. **drain (pre-record)** + canvas shortcuts + file-op chords. The drain runs
   reconcile when `needs_reconcile`, and sets `scene_dirty` if it applied
   anything.
7. **rebuild #2 (pre-record)** — `rebuild_scene` again **only if `scene_dirty`**.
   An idle frame or bare tab switch skips it.
8. **record** — draw the tree; widgets push more intents (clicks, edit commits,
   inspector chip toggles) and a `MenuCommand` may surface.
9. **drain (post-record)** + relayout request as needed.

### Source of truth: `Document` (`src/document/mod.rs`)
The serialized, undoable unit. The graph *data* is one `scenarium::Graph`
(`graph`), which already nests local subgraph defs and their interior graphs.
Everything else is editor view-state, split per graph:

- **`GraphRef`** — `Main` (root graph) or `Local(SubgraphId)` (a subgraph
  interior). The active-graph handle threaded through the whole edit pipeline.
- **`GraphView`** — per-graph view metadata: `view_nodes`
  (`KeyIndexVec<NodeId, ViewNode>` of positions, kept in sync with the graph,
  `validate()` asserts node sets match), `pan`, `scale`, and `selected_nodes`
  (a `BTreeSet` so equality/serde are order-independent). Root lives in
  `main_view`; each opened subgraph in `sub_views` (lazily seeded +
  auto-laid-out on first open). **All of this is persisted and undoable by
  design** — reopening restores camera + selection, and Ctrl+Z walks them
  alongside structural edits.
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

`Library` is *not* here — it's runtime-owned on `App` (built from builtins +
the library file at startup, shared across documents). Startup seeds an empty
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
- Variants: `AddNode`, `DuplicateNodes`, `RemoveNode`, `MoveNodes`,
  `RenameNode`, `SetInput`, `SetSelection`, `RaiseNode`, `SetNodeProperty`
  (a `NodeProperty::Disabled`/`Cache` — one intent backs both scalar toggles),
  `SetSubscription` (`subscribe: bool` — one intent backs subscribe + unsubscribe),
  `DetachSubgraph`, `SetViewport`, `RenameBoundaryPort`, `RenameSubgraph`,
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
  undoable `Intent::Dock` ops. `OpenGraph`/`NewSubgraph`/`OpenImageViewer`
  add the tab via `DockLayout::find_or_insert` (graph tabs into the primary
  group, others into the focused one) — that part isn't undoable — but
  focus the tab through the same recorded activation, so undo faithfully
  reverses focus while leaving the opened tab in place.
- Per-step properties (`is_noop`, `requires_relayout`, `requires_reconcile`,
  `gesture_key`, `coalesce`) are methods on `UndoStep`, exhaustive over the
  variants so a new one won't compile until it declares its behavior.
- Every variant is emitted by some UI (node-title rename →
  `RenameNode`, tab-strip rename → `RenameSubgraph`, etc.). Promote/publish/
  export resolution (`subgraph_to_export` / `promote_to_library` /
  `publish_local_def`) is pure document↔library logic in
  `core/edit/publish.rs` (unit-tested against bare types, `&mut Library` in /
  `bool` changed out), not on `Document`; `app/commands/subgraph.rs` is the
  thin GUI orchestration (dialogs + dirty flag), running the mutators through
  `Engine::edit_library` — the **single** library-mutation path, which
  persists the library file and re-pushes the worker's `DiskStore` (its codec
  table rides a library snapshot) on every change. `Engine.library` is
  private; read via `Engine::library()`.

### Reconciliation (`src/edit/reconcile/`)
Derived state, like `Scene`. Runs in the pre-record drain when
`needs_reconcile` is set (any structural edit). Synchronizes each local
subgraph's interface against its interior wiring: compacts unused boundary
slots, remaps indices in the interior graph and across all instance bindings.
Idempotent — a no-op on an already-canonical document.

### Render projection: `Scene` (`src/scene.rs`)
A flat, per-frame snapshot rebuilt from the *active* graph+view every frame
(`Scene::rebuild(graph, view, library, ctx_def, run_state)` — see
`Editor::rebuild_scene`). Port names live in aperture's per-frame text arena,
so it *must* be rebuilt before any widget reads it. Port names, types, and
input-binding snapshots are flattened into pooled `Vec`s sliced per node (zero
per-node allocation in steady state). Each `SceneNode` carries its
`exec_status` (copied from `run_state`) to drive the status glow + run-time
label. Scene is read-only mirror state — viewport/selection are copied *from*
the active `GraphView` each rebuild; the gesture writes back via intents.

### Graph execution: worker + run state (`app/worker.rs`, `run_state.rs`)
Execution is **decoupled from the UI thread**. `WorkerBridge` owns a tokio
multi-thread `Runtime`, scenarium's headless `Worker`, and an mpsc channel:

- `App::run_graph` compiles the active graph against the library **on the UI
  thread** (`Engine::run_once` → the engine-owned long-lived
  `scenarium::execution::compile::Compiler`) and sends the
  `CompiledGraph` in an `[Update, ExecuteTerminals]` batch to the worker. A
  compile error surfaces synchronously — no run starts, `begin_run` is skipped,
  and the worker's prior program is untouched. The worker evaluates on its
  runtime and replies via callback with a `scenarium::WorkerReport`: a live
  `Progress(RunProgress)` per node *as it runs*, then a final `Finished(stats)`.
  `WorkerBridge::deliver` maps these to `WorkerEvent::NodeProgress` /
  `ExecutionFinished` on the channel and pokes `host.request_repaint()`.
- **Run to a node** (`App::run_node`, `RunCommand::Node`): same batch with
  `ExecuteNodes` seeding one node's cone, its outputs pinned resident for the
  preview fetch. Two triggers, both gated on `SceneNode::runnable` (disabled/
  instance/boundary/missing nodes don't resolve as seeds): the header's play
  chip left of the title (drawn in `gui/node/header.rs`, click scanned by
  `emit_play_clicks` and translated at canvas level) and the node context
  menu's "Run to this node".
- **Per-document disk cache.** The worker starts memory-only;
  `Engine::set_document_cache` (called from `set_document_path` — i.e. on
  open/save/new and startup restore) sends `WorkerMessage::SetDiskCache` pointing
  it at `io::cache`'s `<stem>.darkroom-cache/` store. An unsaved doc stays
  memory-only. So a node toggled to `CachePersistence::Disk` (header `C` chip)
  reloads its output across sessions from a store beside the project file.
- On-thread, `App::frame` drains the channel (`worker.drain()`, non-blocking).
  `NodeProgress` → `RunState::apply_progress` marks the active node
  `ExecStatus::Running(Instant)` (purple glow) live — carrying the start instant
  so the node header shows a `aperture::Spinner` + live elapsed-so-far
  (`App::frame` repaints ~20fps while `run_state.is_running()`);
  `ExecutionFinished` → `set_results`
  folds the final `ExecutionStats` (including nested-subgraph attribution) onto
  authoring nodes: per-node `ExecStatus`
  (`None`/`Cached`/`Executed(secs)`/`Running`/`MissingInputs`/`Errored`) + logs.
- **Cancel** (coarse): **Run ▸ Cancel Run** shows while `run_state.is_running()`
  (a `running` flag, set by `begin_run`, cleared by `set_results`); it routes
  `MenuCommand::CancelRun` → `Engine::cancel_run` → `WorkerBridge::cancel_run` →
  `Worker::request_cancel` (a shared `common::CancelToken` the executor polls
  between nodes). The
  in-flight node still finishes (its blocking work isn't interrupted — that's
  P3), but nothing further runs and the run reports `stats.cancelled`.
- **Status + logs persist across re-runs** (the glow doesn't blank during
  compute); runtime *values* invalidate immediately on `begin_run` and are
  fetched on demand.
- **On-demand values**: every surface showing runtime values — open inspector
  panels and image-viewer tabs — registers its nodes per frame in `RunState`'s
  watch registry (`RunState::watch`); `App::request_watched_values` drains it
  into `ValueRequest { node_id, run_id }` sends (the `run_id` epoch drops
  stale replies). The worker spawns a forwarder task; its `ArgumentValues`
  reply (`inputs`/`outputs`) lands on a later frame, where
  `node_values::build_view` formats text + uploads image previews as aperture
  textures into `RunState` (each image port also keeps its full value for the
  viewer tab).

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
buttons, inline subgraph rename, the right-click split menu; the focused
group's active tab wears the full accent cap); `dock/drag.rs` is the
gesture state + the pure pointer→drop-zone classification. The rest:

- **`gui/canvas/`** — `mod.rs` is `GraphUI`, the canvas scope. It separates
  **persistent** state (`background` dotted backdrop, the `CanvasGeometry`
  cache, `inspectors` open-panel set — survive tab switches) from a resettable
  `Gestures` bundle: `NodeUI` (node bodies + drag), `ConnectionUI` (wires +
  in-flight drag + snap), `BreakerUI` (RMB/Cmd+LMB scribble that severs wires /
  deletes nodes), `NewNodeUi` (right-click spawn popup), `SubgraphMenuUi`
  (RMB context menu on subgraph-instance nodes), `SelectionUI` (rubber-band),
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
  drag; emits `MoveNodes`, subgraph-open requests, port-disconnect
  double-clicks), with sub-widgets `header` (play chip + title +
  `S`/`T`/`D`/`R`/`↓`/`i` badges: run-to-node / subgraph / terminal / disable /
  RAM-cache / disk-cache / inspect; the
  `R` and `↓` chips flip the two bits of `Node::cache` (`CacheMode`
  `None`/`Ram`/`Disk`/`Both`) via `SetCacheMode`), `port_row` (the two port
  columns + circles + binding menu; a required input's port paints in the
  missing/warning color only once a run flagged its node `MissingInputs` —
  `SceneInput::required` + `node.exec_status` + `exec_missing_glow` — so the
  port keeps its data-type color while editing), `port_rename` (inline
  boundary-port rename in subgraph
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

### Persistence + library (`src/io/`, `src/theme.rs`)
`persistence.rs` is pure path⇄type I/O (dialogs + serde), no `App`/undo/preferences
coupling — `commands.rs` orchestrates. Documents round-trip through any
`SerdeFormat` (Rhai is canonical). `library.rs` reads/writes the shared
subgraph library (`darkroom.library.rhai`): a set of `SubgraphDef`s loaded into
`Library` at startup and grown by the **promote/publish** menu commands. Local
subgraph defs track lineage via an `origin` field — "Publish" updates the
linked library entry in place, else creates a new one.

`Preferences` (`darkroom.preferences.toml` in cwd) persists last-theme-name +
last-document so the next launch reopens where you left off. Failures degrade
rather than crash and report through `core/status.rs`'s `StatusLog`, the
user-facing outcome log owned by `Engine` and shared by every frontend (no
bare `eprintln!` anywhere in darkroom; every entry also goes to `tracing`).
It keeps a capped rolling history — the TUI `status` command renders it —
plus a sticky `error` slot holding the last failure
(`StatusLog::report_error`), shown error-colored in the GUI's bottom status
bar until a subsequent success of the same family (a run kick, a finished
run, a file op) assigns `None`. Compile failures report themselves from
`Engine::compile`; frontends report their own outcomes (run results, file
ops, subgraph ops).

### Theme (`src/theme.rs`)
The look is **code-defined**, not embedded TOML. Module consts hold every
color (`CANVAS_BG`, `NODE_FILL`, `BADGE_SUBGRAPH`, `EXEC_EXECUTED_GLOW`,
`INPUT_PORT`, …) and layout dimension (`NODE_MIN_WIDTH`, `PORT_SIZE`,
`CANVAS_DOT_SPACING`, …). `Theme::default()` assembles them. `Theme` bundles
darkroom's own fields *and* the nested `aperture::Theme` (scalar fields first,
the aperture table last — a TOML serialization ordering requirement), so it's a
complete bundle serialized as TOML. The checked-in `assets/ayu-graphite.toml`
is a reference/round-trip fixture kept in sync by an ignored test, **not** a
parallel source of truth.

**Colors come from the palette.** Values trace back to the semantic Ayu Mirage
High Contrast palette (backgrounds, borders, text, accent/status, syntax). When
adding or restyling a field, reuse an existing palette swatch rather than
inventing a hex value, so a palette re-seed propagates cleanly. The
`MenuCommand::InvertColors` command is a reversible light-theme stub that flips
every color in place.
</content>
