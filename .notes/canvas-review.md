# `darkroom/src/gui/canvas` — review findings

Scope: every file under `darkroom/src/gui/canvas/` (production code only; test
files and the APIs tests reach for were not reviewed).

**When you address an item, delete it from this file.** These are observations,
not designs — each says what is wrong, not how to fix it.

Sorted by severity × benefit, highest first.

---

## Structural / design

- [ ] **Five independent full-scene node sweeps run every frame, none of them
      culled.** `Inspectors::apply` (`inspector.rs:98-113`), `outside_action`
      (`inspector.rs:314-317`), `NodeContextMenu::latch` (`anchored_menu.rs:128-131`,
      once per menu per pane, so twice), and the chip scans behind
      `emit_chip_command` (`mod.rs:591-604`) each iterate every node and call
      `ui.response_for(...)` per node, per pane. `CanvasGeometry::rebuild` adds a
      sixth sweep at port granularity. All of these read last frame's responses
      off deterministic ids, so they are structurally one pass split six ways.

- [ ] **Cancellation is not part of the gesture classification, so four
      controllers each poll `ui.escape_pressed()` with three different guards.**
      `connection_ui/mod.rs:106`, `subscription_ui.rs:93` (both return
      unconditionally, even with no gesture in flight), `breaker.rs:308` (gated
      on `state.is_some()`), `selection_ui.rs:127` (after the band check).
      `classify_canvas_gesture` centralizes which gesture *starts*; nothing
      centralizes which one ends, and the divergent guards are the visible cost.

- [ ] **"Is this my pane?" is spelled four different ways.**
      `selection_ui.rs:97` (`band.is_some_and(|b| b.graph != target)` → early
      return), `breaker.rs:292` (same shape, different field),
      `anchored_menu.rs:54` (`self.graph != Some(graph)`), and the wire
      gestures' shared `state.filter(|s| graph.contains(s.node()))`
      (`connection_ui/mod.rs:287`, `subscription_ui.rs:186`). A fifth,
      `ConnectionUI::take_pending_connection_in` (`connection_ui/mod.rs:140-146`),
      folds the test into the take. Same invariant, re-derived per controller.

- [ ] **`GraphUI::bake_snap_hovers` runs once per visible pane
      (`mod.rs:251`) though it mutates document-unique, pane-agnostic state**
      (`mod.rs:336-344`). Both controllers' snap targets are resolved back in
      `prepass`, so with N panes open the same idempotent writes happen N times,
      and the frame's "geometry is now final" point is smeared across the
      per-pane draws instead of sitting at the end of the once-per-frame pass.

- [ ] **`PreviewDrag` (`preview_drag.rs:26-88`) is a single-field newtype around
      `GroupDrag` with one method.** The type, the file's struct/impl scaffolding,
      and the extra indirection buy nothing over the `GroupDrag` living directly
      in `Gestures` beside the one function that drives it — especially given
      `NodeUI` already stores its own bare `GroupDrag`.

- [ ] **`GraphUI::record_canvas` destructures `self` into 15 bindings, 8 of them
      `_`** (`mod.rs:366-384`). The `Gestures` grouping was introduced to make
      tab-switch resets a single assignment, and this is the cost: every field
      of both structs restated in a draw function that uses seven of them.

- [ ] **`SelectionUI` keeps three coupled fields whose validity relationship is
      documented rather than typed** (`selection_ui.rs:24-44`): `band:
      Option<RubberBand>` (which itself carries `graph`), `preview:
      Option<GraphRef>` (duplicating `band.graph` except on the release frame,
      where the handoff is explained in a five-line comment at
      `selection_ui.rs:169-173`), and `base`, which is left populated and stale
      after every commit.

## Local simplifications

- [ ] **`ConnectionUI::apply` assigns `self.state` from three places on one
      path** (`connection_ui/mod.rs:91`, `:101`, `:127`).

- [ ] **`drag_candidates` (`connection_ui/mod.rs:374-382`) expresses "skip the
      output column under the modifier" by branching between two `&[PortKind]`
      slice literals and nested `flat_map`s**, rather than as the filter it is.

- [ ] **`selected_group_positions` (`drag_anchor.rs:147-158`) binds `positions`
      only to return it on the next line, and aliases `selection_holds` behind a
      single-use `holds` closure.**

- [ ] **`PortLayer::record` (`geometry.rs:82-85`) is a two-line wrapper that
      forwards to the free `snapshot` (`geometry.rs:227-251`), passing
      `&mut self.offsets` and the key back in.** The split serves no borrow
      constraint — `record` already owns `&mut self`.

- [ ] **`NewNodeUi::apply` computes the popup's sizing every frame for every
      pane whether or not the palette is open** (`new_node_ui.rs:150-157`:
      `ui.display().logical_rect()`, `max_height`, `scroll_cap`), while the
      genuinely expensive `local_def_rows` was correctly deferred into the body.

- [ ] **`PaletteEntry::Func` and `PaletteEntry::Special` produce the same intent
      from the same `Func`** (`new_node_ui.rs:421-446`) — identical `node_id`,
      `pos`, and `default_bindings` construction — differing only in the `Node`
      value. `name()`/`category()` (`new_node_ui.rs:39-55`) already collapse the
      two through `SpecialNode::func()`.

- [ ] **`local_def_rows` (`new_node_ui.rs:77-88`) allocates two `String`s per
      local definition, every frame the palette is up**, purely to escape
      `InternedStr` borrow guards; `palette_body` then allocates a third with
      `query.to_lowercase()` (`new_node_ui.rs:237`) each frame.

- [ ] **`AnchoredMenu::show` clones the context-menu `Background` on every frame
      the menu is open** (`anchored_menu.rs:67`) to satisfy the builder.

- [ ] **`inspector.rs` rebuilds owned `String`s per panel line per frame** —
      `status_text` (`:398-407`), `value_str` (`:387-396`) per input, and
      `port_label_text`'s `format!` (`:363-370`) per port row.

- [ ] **`background::build_tile` (`background.rs:127-146`) thresholds the dot
      hard** (`dx*dx + dy*dy <= r2`, no coverage falloff), leaving the tile
      aliased and relying on linear filtering to hide it.

## Stale documentation

- [ ] **Doc comments still describe a removed pinned-output ("pin") feature.**
      `breaker.rs:106` and `breaker.rs:65-67`
      say "four `broken_*` collections" / "the four `mark_broken_*` siblings"
      — there are three of each. `breaker.rs:277` says `apply` drains a
      `SetOutputPinned { pinned: false }` intent that no longer exists.
      `breaker.rs:155-156` explains a bug that "had been forgotten for two of
      the four".

- [ ] **`drag_anchor.rs:37` links `NodeId::owner`, which does not exist**, and
      `drag_anchor.rs:45-46` reads "Every member moving with this drag \n mixed —
      and its position at drag start", a sentence left broken when the pin
      variant was removed. `selection_ui.rs:16-17` has the same kind of
      breakage: "intersecting nodes \n widgets highlight live".

- [ ] **Method names in docs no longer match the code.** `mod.rs:172` and
      `geometry.rs:19` say `CanvasGeometry` is "reused by `frame`"; the method
      is `GraphUI::draw`. `pan_zoom/mod.rs:2` says it was "Split out of
      `graph_ui`"; that module is now `canvas`.

- [ ] **The breaker chord is documented as two different keys.** The code checks
      `ui.modifiers().ctrl` (`mod.rs:562`), and `mod.rs:529` documents it as
      Ctrl+LMB, while `breaker.rs:130`, `breaker.rs:262` and `selection_ui.rs:19`
      call the same chord Cmd+LMB.
