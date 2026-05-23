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
mutating the document inline. One frame, in order:

1. **clear `intents`** — `App::intents` is a per-frame scratch buffer (private,
   reused only for its allocation; carries no cross-frame state).
2. **prepass** (`MainWindow::prepass` → `GraphUI::prepass`) — read palantir's
   *current* input state (drag deltas, pan/zoom, connection release) and push
   `Intent`s. No drawing. Crucially, layout-changing edits (node drag,
   connection commit) are emitted here so they apply *before* the record —
   Pass A then arranges the settled layout and no extra relayout frame is
   needed. See the long comment on `GraphUI::prepass` for why connection
   commit specifically must be pre-record.
3. **drain (pre-record)** — `App::drain_intents`: build an `UndoStep` per
   intent, drop no-ops, apply to `Document`, push the whole batch as **one**
   undo entry.
4. **shortcuts** — undo/redo (applied directly via `ActionStack`), Esc-deselect
   and Ctrl+0 reset-zoom (pushed as intents), plus file-op chords → `MenuCommand`.
5. **`Scene::rebuild`** — rebuild the derived render projection from `Document`.
6. **record** (`MainWindow::frame` → `GraphUI::frame`) — draw the tree; widgets
   push more intents (clicks, edit commits) and a `MenuCommand` may surface.
7. **drain (post-record)** + relayout request as needed.
8. **menu side effects** — file/theme dialogs run *last*, outside the record,
   so the blocking dialog holds no frame borrows.

### Source of truth: `Document` (`src/document.rs`)
The serialized, undoable unit: `graph` (scenarium), `view_nodes` (per-node
positions, a `KeyIndexVec<NodeId, ViewNode>` side-table kept in sync with
`graph` — `validate()` asserts the node sets match), plus `pan`/`scale` and
`selected_node_id`. **Viewport and selection are deliberately persisted and
undoable** (reopen restores camera + selection; navigation undo is intentional;
selection-in-Document keeps it coherent across undo/redo of node removal).
`FuncLib` is *not* here — it's runtime-owned on `App` (built from builtins at
startup, shared across documents).

### Intent / undo layer (`src/intent.rs`, `src/action_stack.rs`)
- `Intent` = forward-only "set X to Y". `build_step(intent, &doc)` reads the
  pre-mutation snapshot and folds both halves into one self-contained
  `UndoStep`. `apply_step`/`revert_step` write the "to"/"from" halves. Adding
  a variant touches ~6 spots — the doc comment on `Intent` lists them.
- `ActionStack` packs history into two flat byte buffers (bitcode), not a
  `Vec<Vec<UndoStep>>`. Consecutive same-`GestureKey` steps coalesce in place
  (a node drag = many `MoveNode` intents → one undo entry; same for viewport).
- Note: `RenameNode`, `SetCacheBehavior`, `SetEventConnection` variants exist
  with full handling but are **not yet emitted by any UI** — staged ports from
  the deprecated editor. That's why `mod intent` carries `#[allow(dead_code)]`.

### Render projection: `Scene` (`src/scene.rs`)
A flat, per-frame snapshot rebuilt from `Document` every frame (the names live
in palantir's per-frame text arena, so it *must* be rebuilt before any widget
reads it). Port names and input-binding snapshots are flattened into pooled
`Vec`s sliced by `PortSpan` (zero per-node allocation in steady state). Scene
is read-only mirror state — viewport/selection are copied *from* Document each
rebuild; the gesture writes back via intents, never directly.

### GUI tree (`src/gui/`)
`MainWindow` (zstack: graph behind a floating menu bar) → `GraphUI` (the
canvas) owns the sub-controllers: `NodeUI` (node bodies + ports + drag),
`ConnectionUI` (wires + in-flight drag + snap), `BreakerUI` (RMB/Cmd+LMB
scribble that severs wires / deletes nodes), `NewNodeUi` (right-click spawn
popup), `value_editor` (inline `Const` editing). `menu_bar` returns
`MenuCommand`s; `App` performs the side effects.

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
node chrome uses `backgrounds.*`, selection halo uses `elem_selected`
(`#4b4b4b`), ports use `success`/`syn_keyword`, broken state uses `error`.
Keeps darkroom on-palette and lets a palette re-seed propagate cleanly.
`AppConfig` (`darkroom.config.rhai` in cwd) persists last-theme-name +
last-document so the next launch reopens where you left off. I/O failures log
to stderr and degrade — there is no user-facing error surface yet.
</content>
