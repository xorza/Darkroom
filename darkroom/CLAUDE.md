# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`darkroom` is the **new** node-graph editor, built on Palantir (the in-tree
immediate-mode GUI lib at `../palantir`). The frozen egui editor lives in
`../darkroom-egui-deprecared` and is reference-only. Root `../CLAUDE.md`
holds the workspace-wide rules (workflow, Rust style, tooling); Palantir's
own widget/id conventions live in `../palantir/CLAUDE.md`. This file covers
only what's specific to darkroom.

## Commands

```
cargo run -p darkroom                      # launch the editor (opens last doc)
cargo nextest run -p darkroom              # tests (mostly pure: zoom math, breaker geometry, serde round-trips)
cargo nextest run -p darkroom <substr>     # single test by name substring
cargo clippy -p darkroom --all-targets -- -D warnings
cargo run -p darkroom --features profile-with-tracy   # tracy zones across darkroom + palantir
```

Run the ignored one-shot asset generator after changing the default look:
`cargo test -p darkroom generate_toml_asset -- --ignored` (regenerates
`assets/ayu-graphite.toml`, the checked-in default theme).

## Dependencies / boundaries

- **`scenarium`** — headless core. Owns `Graph`, `Node`, `Binding`, `FuncLib`,
  `StaticValue`, serde formats. darkroom never reimplements graph semantics;
  it edits a `scenarium::Graph` and resolves nodes against a `FuncLib`.
- **`palantir`** — the GUI runtime. `App` implements `palantir::App::frame`;
  `WinitHost` (in `main.rs`) drives it. All widgets, input, layout, theming
  come from here. Pre-1.0, breaks freely — coordinate changes with palantir.
- **`common`** — `SerdeFormat`, `serialize`/`deserialize`, `KeyIndexVec`.
- **`lens`** — `ImageFuncLib` (image-processing node library).

## Architecture: the per-frame pipeline

Everything hangs off `App::frame` (`src/app.rs`). darkroom is immediate-mode
but routes **all** graph mutations through an intent/undo layer rather than
mutating the document inline. The frame splits into a **navigation phase**
(settle *which* graph is active) and an **edit phase** (mutate that graph),
because input that switches tabs/opens subgraphs comes from *last* frame's
click responses and must resolve before anything edits or records. One frame:

1. **clear scratch** — `intents` (pending mutations) and `actions` (view-state
   requests). Both are reused-allocation buffers; no cross-frame state.
2. **navigate** (`App::navigate`) — apply keyboard undo/redo (which can replay
   a `SwitchTab`), then `MainWindow::scan_navigation` surfaces tab
   activate/close + subgraph-open clicks off last frame's responses as
   `UiAction`s. Open mutates the tab list directly; activate/close queue
   undoable `SwitchTab` / `CloseTab` intents. After this
   `target = document.active_target()` is fixed for the rest of the frame.
3. **sync_scene** — rebuild `Scene` (and drop transient gesture state) only if
   the active graph changed since last frame, tracked by `App::scene_target`.
   So a switched-to graph records in Pass A and draws its connections in Pass B
   with no first-frame gap.
4. **edit prepass** (`MainWindow::prepass` → `GraphUI::prepass`) — read
   palantir's *current* input state (drag deltas, pan/zoom, connection release)
   and push `Intent`s. No drawing. Layout-changing edits (node drag, connection
   commit) are emitted here so they apply *before* the record. See the long
   comment on `GraphUI::prepass` for why connection commit must be pre-record.
5. **drain (pre-record)** + canvas shortcuts (Esc-deselect, Ctrl+0 reset-zoom
   → intents) + file-op chords (`menu_shortcut` → `MenuCommand`).
6. **`rebuild_scene`** — fold the pre-record drain into the render projection.
7. **record** (`MainWindow::frame` → `GraphUI::frame`) — draw the tree; widgets
   push more intents (clicks, edit commits) and a `MenuCommand` may surface.
8. **drain (post-record)** + relayout request as needed.
9. **menu side effects** — file/theme dialogs run *last*, outside the record,
   so the blocking dialog holds no frame borrows.

### Source of truth: `Document` (`src/document.rs`)
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
- **`tabs` / `active`** — open editor tabs (`tabs[0]` is always `Main`) and the
  visible index. Also persisted; switching tabs is an undoable `SwitchTab`.
- **`EditScope` / `EditScopeRef`** — graph+view borrowed *together* for a
  target, so an edit touches both atomically (the borrow checker can't prove
  `graph` and a `sub_views` entry are disjoint across separate accessors).
  Get them via `Document::scope_mut(target)` / `scope(target)`.

`FuncLib` is *not* here — it's runtime-owned on `App` (built from builtins at
startup, shared across documents).

### Intent / undo layer (`src/intent.rs`, `src/action_stack.rs`)
- Every mutation is scoped to a `GraphRef` target. `Intent` = forward-only
  "set X to Y". `build_step(intent, &doc, target)` reads the pre-mutation
  snapshot (via `scope`) and folds both halves into one self-contained
  `UndoStep`; `apply_step`/`revert_step` write the "to"/"from" halves against
  `target`. `SwitchTab` is graph-agnostic and special-cased ahead of the
  scope lookup. Adding a variant touches ~6 spots — the doc comment lists them.
- `ActionStack` packs history into two flat byte buffers (bitcode), not a
  `Vec<Vec<UndoStep>>`. Each batch records its `target` so undo/redo re-resolve
  the right graph+view. Consecutive same-`GestureKey` *and* same-target steps
  coalesce in place (a node drag = many `MoveNode` intents → one undo entry).
- **`UiAction`** (`gui/mod.rs`) is the navigation-request transport from the
  UI layer to `App`: `App` turns `ActivateTab`/`CloseTab` into undoable
  `SwitchTab`/`CloseTab` intents, while `OpenGraph` mutates the tab list
  directly (opening isn't undoable; switching and closing are).
- Note: `RenameNode` and `SetEventConnection` variants exist with full handling
  but are **not yet emitted by any UI** — staged ports from the deprecated
  editor. They don't trip dead-code lints because the serde derives reference
  every variant. (`SetCacheBehavior` *is* live — the node header's `C` chip.)

### Render projection: `Scene` (`src/scene.rs`)
A flat, per-frame snapshot rebuilt from the *active* graph+view every frame
(`Scene::rebuild(graph, view, func_lib)` — see `App::rebuild_scene`). The names
live in palantir's per-frame text arena, so it *must* be rebuilt before any
widget reads it. Port names and input-binding snapshots are flattened into
pooled `Vec`s sliced by `PortSpan` (zero per-node allocation in steady state).
Scene is read-only mirror state — viewport/selection are copied *from* the
active `GraphView` each rebuild; the gesture writes back via intents, never
directly.

### GUI tree (`src/gui/`)
`MainWindow` (zstack: graph behind a floating menu bar + tab strip) → `GraphUI`
(the canvas) owns the sub-controllers: `NodeUI` (node bodies + ports + drag;
also emits subgraph-open requests via `emit_subgraph_opens`), `ConnectionUI`
(wires + in-flight drag + snap), `BreakerUI` (RMB/Cmd+LMB scribble that severs
wires / deletes nodes), `NewNodeUi` (right-click spawn popup), `value_editor`
(inline `Const` editing). `tab_bar` renders the open-tab strip and emits
`UiAction`s; `menu_bar` returns `MenuCommand`s; `App` performs the side effects.
`AppContext` (in `app.rs`) threads the active `Theme` + `FuncLib` down the tree.

Key cross-cutting mechanisms:
- **Deterministic widget ids.** Node/port widgets derive their `WidgetId` from
  domain coords (`node_widget_id(id)`, `port_circle_wid(PortRef)`) so any pass
  can `ui.response_for(id)` without threading a cache. The two canvases use
  `WidgetId::auto_stable()`.
- **`PortFrame`** (`graph_ui.rs`) — per-frame snapshot of each port's
  geometry/hover/drag state, polled from last frame's responses. **Rebuilt in
  `prepass` and reused in `frame`** (the connection commit reads it); the order
  is an invariant enforced by call sequence, not types. Port centers are
  recomputed from this frame's `node.pos` + cached intra-node offset so a
  just-dragged node's wires anchor correctly without a stale frame.
- **`BreakerProbe`** threads the active breaker state into node/connection
  draws so intersection tests run inline; hits drain into intents on release.

### Persistence (`src/persistence.rs`, `src/config.rs`, `src/theme.rs`)
`persistence` is pure path⇄type I/O (dialogs + serde), no `App`/undo/config
coupling — `App` orchestrates. Documents round-trip through any `SerdeFormat`
(Rhai is canonical). `Theme` bundles darkroom colors/layout *and* the nested
`palantir::Theme`, serialized as TOML; `Theme::default()` deserializes the
embedded `assets/ayu-graphite.toml` (single source of truth for the look).

**Colors come from the palette.** `assets/ayu-graphite-palette.toml` is the
semantic Ayu Mirage High Contrast palette (backgrounds, borders, text,
accent/status, syntax). When adding or restyling a theme field, pick an
existing swatch from that palette rather than inventing a hex value — e.g.
node chrome uses `backgrounds.*`, selection halo uses `text_muted`
(`#aaaaa8`), ports use `success`/`syn_keyword`, broken state uses `error`.
Keeps darkroom on-palette and lets a palette re-seed propagate cleanly.
`AppConfig` (`darkroom.config.rhai` in cwd) persists last-theme-name +
last-document so the next launch reopens where you left off. I/O failures log
to stderr and degrade — there is no user-facing error surface yet.
</content>
