# Dockable tabs — implementation plan

> **Status:** Phases 0–1 shipped. Implementation deviations from the plan
> below: the aperture `Splitter` takes one `FnMut(&mut Ui, SplitHalf)` body
> instead of two closures (a recursive walk can't capture `&mut self`
> twice); `SetRatio` addresses splits by `DockPath` (turns packed into one
> byte) instead of `Vec<bool>`; the tree is stored as a flat, canonically
> pre-ordered `Vec<DockNode>` instead of boxed children; split depth is
> capped at 4. Phase 3's non-DnD half also shipped: aperture grew
> `Ui::set_cursor(CursorIcon)` (per-record-pass reset, host applies on
> change) and the `Splitter` requests resize cursors over its divider.
> Remaining: phase 2 (drag & drop + drop-zone highlight + ghost chip),
> the DnD-dependent phase-3 polish (grabbing cursor mid-drag, strip
> insertion caret, highlight fade), phase 4 (multi-canvas).

Drag a tab chip (image viewer, preferences) out of its strip and drop it onto
another pane to split that pane horizontally or vertically — with a live
translucent highlight of the region the drop would create — or drop it onto
another pane's tab strip to move it there. IDE-style docking, scoped to
darkroom's existing tab kinds.

## Target UX

- **Drag source**: any closable tab chip. Press + ≥4 px travel starts the drag
  (aperture's built-in threshold keeps plain clicks working unchanged). A ghost
  chip follows the cursor.
- **Drop zones** per visible pane: **center** (join that pane's tab group) and
  **four edges** (split the pane, the new half shows the dragged tab). Hovering
  a pane's tab strip targets an insertion slot instead (move/reorder).
- **Highlight**: while hovering a zone, a translucent accent rect + border
  covers exactly the region the tab would occupy (half the pane for an edge,
  the full pane for center, an insertion caret in a strip).
- **Release** commits; **Esc** or releasing outside any zone cancels.
- Panes are separated by **draggable dividers** (resize the split ratio).
- Each pane has its **own tab strip**; closing a pane's last tab collapses the
  split. The layout persists with the document and is undoable, like all other
  view state.

## Current state

### darkroom

- `Document` (`darkroom/src/core/document/mod.rs`) owns `tabs: Vec<TabRef>` +
  `active: usize` — a single flat strip, persisted + undoable. `TabRef` is
  `Graph(GraphRef) | Preferences | ImageViewer(PortRef)`, deduped on open, so
  a `TabRef` value *is* a stable tab identity.
- `MainWindow::frame` (`darkroom/src/gui/main_window.rs`) renders one chrome
  row (menu bar + the single strip) and one content pane dispatched on
  `doc.active_tab()`.
- Tab mutations flow through the intent/undo layer: `Intent::SwitchTab` /
  `Intent::CloseTab` → `DocStep::SwitchTab { from, to }` /
  `DocStep::CloseTab { index, target, from_active, to_active }`
  (`darkroom/src/core/edit/intent.rs`). Opens (`OpenGraph`, `OpenImageViewer`,
  Preferences) mutate the tab list directly (not undoable) but focus through a
  recorded `SwitchTab` — established precedent that pure focus changes may
  bypass the record.
- The edit pipeline assumes **one active graph**: a single `Scene`, `GraphUI`,
  `CanvasGeometry`, and gesture state, rebuilt for `active_target()` each
  frame. This is the hard constraint on showing two graph canvases at once.
- Image viewers are already multi-instance-ready: `MainWindow.image_viewers`
  is a per-port map, each viewer keys its widget ids by port, and
  `Editor::sync_image_viewers` registers **every** viewer tab (not just the
  visible one) in the run state's watch registry. Data-wise, several visible
  viewers already work.

### aperture (investigated 2026-07)

Available today — the drag-drop core is buildable without aperture changes:

| Need | Aperture primitive |
|---|---|
| Drag begin/track/end on a chip | `ResponseState::drag` (`DragState { delta, started }`), `drag_started()` / `drag_delta()` / `drag_stopped()`; 4 px threshold built in; delta keeps tracking after the pointer leaves the widget (`aperture/src/input/response.rs`) |
| Pointer position during drag | `Ui::pointer_pos()` (auto-subscribes move repaints) |
| Which pane is under the pointer mid-drag | `Ui::hover_within(id)` — occlusion- and layer-aware, and **not** suppressed by the active capture (`aperture/src/ui/mod.rs:1339`) |
| Pane rects for zone math | `Ui::response_for(id).rect` (one frame stale — fine) + `Rect::contains` etc. |
| Highlight on top of everything | `Ui::layer(Layer::Tooltip, …)` — floats above `Main`/`Popup` **without** a click-eater; body nodes `Sense::NONE`; paint via `add_shape(Shape::RoundedRect { local_rect: Some(zone), fill: translucent, stroke })` |
| Ratio-sized split children | `Sizing::Fill(weight)` — weighted leftover distribution |
| Per-gesture cross-frame state | `Ui::state_mut::<T>(id)` |
| Continuous repaint while dragging | automatic: any pointer move with a live capture requests an immediate repaint |

**Load-bearing gotcha**: `ResponseState::hovered` is suppressed on every
widget that isn't the drag owner while a drag is captured
(`aperture/src/input/mod.rs:929`). Drop-target detection must use
`hover_within` + rect math, never `.hovered`.

Genuinely missing in aperture:

1. **A `Splitter` widget** — `Separator` is decorative (`Sense::NONE`). A
   draggable divider is trivially composable (thin `Frame`, `Sense::DRAG`,
   read `drag_delta`), but it's generic enough to live in aperture as a
   packaged widget with hover/active styling.
2. **Mouse cursor control** — no API at all; nothing reaches winit's
   `Window::set_cursor`. Needed for resize cursors over dividers and a
   grabbing cursor mid-drag. Requires a cursor-request slot on `Ui` (reset
   each frame, last writer wins) that `WinitHost` applies after the frame.
   Pure polish — the feature works without it.
3. *(Optional)* a public point→widget hit-test (`Cascades::hit_test` exists
   but is `pub(crate)`). Not required: `hover_within` covers us.

No drag-payload/DnD abstraction exists or is needed — the payload (which tab
is dragged) is darkroom state.

## Design

### 1. Dock layout model — new `darkroom/src/core/document/dock.rs`

Pure data + pure ops, exhaustively unit-testable without any GUI:

```rust
id_type!(pub TabGroupId);   // runtime-unique per split; persisted

pub enum DockNode {
    Split(DockSplit),
    Group(TabGroup),
}
pub struct DockSplit {
    pub dir: SplitDir,        // Row = children side-by-side, Column = stacked
    pub ratio: f32,           // first child's share, clamped to [0.1, 0.9]
    pub first: Box<DockNode>,
    pub second: Box<DockNode>,
}
pub struct TabGroup {
    pub id: TabGroupId,
    pub tabs: Vec<TabRef>,    // non-empty
    pub active: usize,
}
pub struct DockLayout {
    pub root: DockNode,
    pub focused: TabGroupId,  // keyboard-shortcut routing; not undo-recorded on bare clicks
}
```

`Document` replaces `tabs` + `active` with `layout: DockLayout`
(`#[serde(default)]`; derives mirror `TabRef`'s, including whatever the
bitcode-packed `ActionStack` requires).

Ops (all free functions or `DockLayout` methods, each returning whether they
changed anything):

- `group(id)` / `group_mut(id)` / `groups()` (iterate leaves, stable order)
- `primary()` — the group containing `TabRef::Graph(GraphRef::Main)`; the
  invariant successor of "tabs[0] is Main"
- `activate(group, index)`, `insert_tab(group, index, tab)`,
  `remove_tab(tab)`
- `split(group, side, tab)` — replace the group's node with a `Split`; the
  new half is a fresh single-tab group (fresh `TabGroupId::unique()`)
- `move_tab(tab, drop: DockDrop)` where
  `DockDrop = Into { group, index } | Split { group, side }` — remove +
  insert/split + collapse, one atomic op; a no-op when it would recreate the
  same shape (e.g. splitting a single-tab group off itself)
- `collapse()` — a `Split` whose child group emptied is replaced by the other
  child (recursive)
- `set_ratio(path, ratio)` — splits addressed by tree path (`Vec<bool>`,
  first/second per level); paths are computed fresh during each render walk,
  so no stable split ids are needed
- `all_tabs()` — iterator over every group's tabs (replaces `doc.tabs` scans:
  pruning, `sync_image_viewers`, dedup-on-open)

Invariants, asserted in `Document::validate` (extending the existing one):
exactly one group holds `Main`; no group is empty; no `TabRef` appears twice;
`active < tabs.len()` per group; `focused` names an existing group; ratios in
bounds; **phase-1 rule: `TabRef::Graph` tabs appear only in the primary
group**.

### 2. Undo integration — rework in `darkroom/src/core/edit/intent.rs`

Replace the two index-based tab steps with **one whole-tree snapshot step**:

```rust
DocStep::Dock { from: DockLayout, to: DockLayout }
```

The tree is tiny (a handful of nodes, ~tens of bytes serialized), so
snapshotting both halves per step costs nothing and makes every layout
mutation — switch, close, move, split, resize — a single uniform, trivially
reversible step. `is_noop` = `from == to`; apply/revert assign the snapshot;
`requires_relayout` = true. Intents stay semantic:

```rust
Intent::Dock(DockIntent)
enum DockIntent {
    ActivateTab { group: TabGroupId, index: usize },  // also focuses the group
    CloseTab    { group: TabGroupId, index: usize },  // Main tab: dropped in build_step, as today
    MoveTab     { tab: TabRef, to: DockDrop },
    SetRatio    { path: Vec<bool>, ratio: f32 },
}
```

`build_step` clones the layout, applies the op to the clone, emits
`Dock { from, to }` (or `None` when unchanged). `SetRatio` steps coalesce
under a new `GestureKey::DockResize` (same-key in-place coalescing already
exists for node drags); `ActivateTab` keeps `GestureKey::TabSwitch`
semantics. The navigation-phase "undo may replay a tab switch" special case
carries over unchanged — applying a `Dock` step is just a layout assignment,
already legal in the navigate phase.

Not recorded (existing precedent — opens mutate directly): opening a
tab into the focused group (`tab_index_or_push` successor), and **bare focus
changes from clicking a pane** — focus rides along inside recorded steps'
snapshots but clicking around never pollutes history.

### 3. Rendering — new `darkroom/src/gui/dock.rs`, rework of `main_window.rs`

`MainWindow::frame` becomes: menu-bar row (menu only — the strip moves into
the panes), then a recursive walk of `layout.root`, then the status bar.

- `Split` → `Panel::hstack`/`vstack` with children sized
  `Fill(ratio)` / `Fill(1 − ratio)` and a divider between them (aperture
  `Splitter` once it exists; an inline 5 px `Sense::DRAG` frame until then).
  Divider drags emit `DockIntent::SetRatio` with delta-derived ratios.
- `Group` → `Panel::vstack` with id `pane_wid(group.id)`: the group's tab
  strip on top, the active tab's content filling the rest.

`tab_bar.rs` generalizes: widget ids become `(group_id, index)`-keyed
(`tab_chip_wid(group, i)` etc.), `show`/`emit_tab_actions` take the group,
and `UiAction::ActivateTab`/`CloseTab` gain the `TabGroupId`. Content
dispatch per group:

- `TabRef::Graph(_)` — the canvas + toolbar overlay, **only ever reached in
  the primary group** (phase-1 invariant), so the single
  `GraphUI`/`Scene` stays untouched.
- `TabRef::ImageViewer(port)` — `viewer_mut(port).show(...)`; already
  per-port keyed, works in any pane.
- `TabRef::Preferences` — `preferences_view::show(...)`, stateless per-pane.

Focused group: clicking anywhere in a pane (poll the pane's response in the
navigation scan) sets `layout.focused` directly. The focused pane's strip
gets the accent treatment; canvas shortcuts stay gated on "focused group is
primary and its active tab is a graph" (today's `active_target()` gate,
re-sourced). Global chords (save/undo/quit) unchanged.

`Document::active_tab()` splits into two explicit reads:
`layout.focused_group().active_tab()` (shortcut routing, close-tab command)
and `layout.primary().active_graph()` (drives `Scene`/`sync_target`/compile —
the successor of `active_target()`).

### 4. Drag & drop — new gesture in `gui/dock.rs`

State machine, stored as a plain field on `MainWindow` (cleared on cancel /
drop, like canvas gestures):

```rust
struct TabDrag {
    tab: TabRef,
    source: (TabGroupId, usize),
}
```

Per frame while a drag is live (all in the prepass/navigation scan, reading
last frame's responses — same pattern as `emit_tab_actions`):

1. **Begin**: chips get `Sense::CLICK | Sense::DRAG`; `drag_started()` on a
   closable chip arms `TabDrag`. Clicks still work (threshold). Phase 1
   refuses graph tabs (no `Sense::DRAG` on them; the subgraph chips' inline-
   rename capture also stays out of the picture that way).
2. **Target**: find the pane under the pointer with
   `hover_within(pane_wid(group))` (capture-ungated — the one correct API,
   see gotcha above), get its rect via `response_for(pane_wid).rect`, and
   classify with a **pure function**
   `drop_zone(pane: Rect, strip: Rect, p: Vec2) -> Option<DockDrop>`:
   pointer in the strip rect → `Into { group, index }` (insertion slot by
   chip midpoints); inner 50 % box → `Into` at end; else nearest edge →
   `Split { group, side }`. Unit-test this exhaustively — it's the whole UX
   contract.
3. **Highlight**: `ui.layer(Layer::Tooltip, …)` with a `Sense::NONE` body;
   `add_shape(Shape::RoundedRect { local_rect: Some(zone_rect),
   fill: accent-at-~25 %-alpha, stroke: 1.5 px accent })` where `zone_rect`
   is the half-pane / full-pane / caret rect. Ghost chip: small rounded rect
   + tab title at `pointer_pos() + offset` on the same layer. Colors from the
   existing palette (`selection_rect` accent family) — no new hex values.
4. **Drop**: `drag_stopped()` → if a zone is live, push
   `UiAction::DockDrop { tab, to }` → `Intent::Dock(MoveTab { .. })`.
   Esc or no zone → clear `TabDrag`, nothing recorded.

### 5. Ripple sites (mechanical)

- `Editor::tab_index_or_push` / `open_graph` / `open_image_viewer` /
  `open_preferences` → dedupe via `all_tabs()`, insert into the focused
  group (graph opens: into primary).
- `Editor::sync_image_viewers`, `Document::ensure_valid_active` pruning →
  iterate `all_tabs()` / prune per group + `collapse()`.
- `MainWindow::reset_transient` — still keyed on the *graph* target changing;
  unchanged since graphs live only in the primary group.
- `tab_labels` — per group.
- Old documents: `Document` has no `deny_unknown_fields`, so files carrying
  the retired `tabs`/`active` fields load fine; `layout` defaults to a single
  main group. Open tabs from old saves are lost — acceptable pre-1.0.

## Phasing

**Phase 0 — aperture (parallel, small):**
`Splitter` widget (drag + hover/active styling + emits delta);
`Ui::set_cursor(CursorIcon)` request slot wired through `WinitHost` →
`window.set_cursor`. Neither blocks phase 1–2; cursor work can trail.

**Phase 1 — dock model + rendering (no drag yet):**
`dock.rs` model + ops + tests; `Document`/`intent.rs` rework; per-group
strips + recursive pane rendering; focused group; divider resize (inline
frame if the aperture `Splitter` isn't in yet). Drive splits via a temporary
tab-chip context-menu ("Split right / down") so everything is exercisable
and testable before DnD exists. Ship: identical look for a single group.

**Phase 2 — drag & drop:**
`TabDrag` gesture, `drop_zone` classification + tests, highlight overlay,
ghost chip, drop → `MoveTab`, Esc cancel. Remove the temporary context menu
(or keep it — cheap and discoverable).

**Phase 3 — polish:**
Cursor icons (grab while dragging, resize over dividers); strip insertion
caret + within-strip reordering; focused-pane affordance tuning; highlight
fade-in via `request_repaint_after` if wanted.

**Phase 4 — out of scope for now, noted for honesty:**
Dragging *graph* tabs / multiple visible canvases. Requires per-pane `Scene`,
`GraphUI`, `CanvasGeometry`, gesture state, and focused-pane edit-target
routing — a structural Editor change an order of magnitude larger than
phases 0–3. The phase-1 invariant (graph tabs pinned to the primary group)
is what keeps it cleanly deferred.

## Testing

- **`dock.rs` ops** (pure): table-driven sweeps — split/move/close/collapse
  sequences asserting the exact resulting tree, plus invariant checks after
  every op (single Main, no empty groups, no duplicate tabs, focused valid).
  Edge cases: move a group's last tab (collapse), move onto its own group
  (no-op step), close the focused group's tab, ratio clamping.
- **`drop_zone`** (pure): hand-computed rect/pointer cases for all five
  zones, strip insertion indices, and zone boundaries (exact 50 %-box edges).
- **Undo**: `Dock` step apply/revert round-trips; `from == to` is a no-op;
  `SetRatio` coalescing; replaying a tab activation in the navigate phase.
- **Serde**: `DockLayout` round-trip through the document formats; an old
  document (with legacy `tabs` fields) loads to the default layout.
- **Editor**: adapt existing tab tests (`open_image_viewer` dedupe/focus,
  viewer pruning) to group-based assertions.

GUI-only pieces (highlight painting, ghost chip) are exempt per house rules;
everything decision-making lives in pure functions precisely so it isn't.

## Open questions

1. **Per-document vs. global layout.** Tabs are document state today, so the
   plan keeps the dock tree in `Document` (persisted + undoable, consistent
   with camera/selection). The IDE convention is window-global layout — if
   that's preferred, `DockLayout` moves to `Preferences` and the undo story
   shrinks to nothing. Recommend: stay in `Document`.
2. **Is layout undo wanted at all?** The plan says yes (consistency with
   `SwitchTab` today, and the snapshot step makes it nearly free).
3. **Splitter in aperture vs. darkroom first.** Plan says aperture (it's
   generic), but an inline darkroom frame is a fine stopgap if aperture
   sequencing is inconvenient.
