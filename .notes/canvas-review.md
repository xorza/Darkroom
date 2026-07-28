# `darkroom/src/gui/canvas` — review findings

Scope: every file under `darkroom/src/gui/canvas/` (production code only; test
files and the APIs tests reach for were not reviewed).

**When you address an item, delete it from this file.** These are observations,
not designs — each says what is wrong, not how to fix it.

Grouped into batches, highest impact first. A batch is items that want doing
**together** because they touch the same files or the same invariant — splitting
one means walking the same code two or three times. Items under *Independent*
have no such coupling and can go in any order, alone.

Line anchors were refreshed after the whole-scene-sweep work landed.

---

## Batch 1 — Gesture lifecycle and pane ownership

Five files, one missing concept. `classify_canvas_gesture` centralizes which
gesture *starts*; nothing centralizes which pane owns it or when it ends, and
each controller re-derives both. The four items land on the same lines in the
same controllers — `selection_ui.rs:97` and `:127` are adjacent, as are
`breaker.rs:282` and `:299` — so doing them apart means three passes over
`selection_ui`, `breaker`, `connection_ui` and `subscription_ui`.

- [ ] **"Is this my pane?" is spelled four different ways.**
      `selection_ui.rs:97` (`band.is_some_and(|b| b.graph != target)` → early
      return), `breaker.rs:282` (same shape, different field),
      `anchored_menu.rs:56` (`self.graph != Some(graph)`), and the wire
      gestures' shared `state.filter(|s| graph.contains(s.node()))`
      (`connection_ui/mod.rs:287`, `subscription_ui.rs:186`). A fifth,
      `ConnectionUI::take_pending_connection_in` (`connection_ui/mod.rs:140`),
      folds the test into the take. Same invariant, re-derived per controller.

- [ ] **Cancellation is not part of the gesture classification, so four
      controllers each poll `ui.escape_pressed()` with three different guards.**
      `connection_ui/mod.rs:106`, `subscription_ui.rs:93` (both return
      unconditionally, even with no gesture in flight, so an Escape frame also
      skips their latch), `breaker.rs:298` (gated on `state.is_some()`),
      `selection_ui.rs:127` (after the band check). The divergent guards are the
      visible cost.

- [ ] **`SelectionUI` keeps three coupled fields whose validity relationship is
      documented rather than typed** (`selection_ui.rs:24-43`): `band:
      Option<RubberBand>` (which itself carries `graph`), `preview:
      Option<GraphRef>` (duplicating `band.graph` except on the release frame,
      where the handoff is explained in a five-line comment at
      `selection_ui.rs:169-173`), and `base`, which is left populated and stale
      after every commit.

- [ ] **`GraphUI::bake_snap_hovers` runs once per visible pane (`mod.rs:277`)
      though it mutates document-unique, pane-agnostic state** (`mod.rs:355`).
      Both controllers' snap targets are resolved back in `prepass`, so with N
      panes open the same idempotent writes happen N times, and the frame's
      "geometry is now final" point is smeared across the per-pane draws instead
      of sitting at the end of the once-per-frame pass.

## Batch 2 — Stale documentation

Cheap, zero code risk, and actively misleading as it stands — the chord is
documented as the wrong key and the counts don't match the code. Three of the
four are fallout from the same removed pinned-output feature, so they read as
one edit.

- [ ] **The breaker chord is documented as two different keys.** The code checks
      `ui.modifiers().ctrl` (`mod.rs:574`), and `mod.rs:541` documents it as
      Ctrl+LMB, while `breaker.rs:115`, `breaker.rs:252` and `selection_ui.rs:19`
      call the same chord Cmd+LMB.

- [ ] **Doc comments still describe a removed pinned-output ("pin") feature.**
      `breaker.rs:55-57` and `breaker.rs:97-100` say "the four `mark_broken_*`
      siblings" / "four `broken_*` collections" — there are three of each, and
      `breaker.rs:142` repeats it. `breaker.rs:267` says `apply` drains a
      `SetOutputPinned { pinned: false }` intent that no longer exists.

- [ ] **`drag_anchor.rs:37` links `NodeId::owner`, which does not exist** (and
      `drag_anchor.rs:230` names it again in a comment), while
      `drag_anchor.rs:46` reads "Every member moving with this drag \n mixed —
      and its position at drag start", a sentence left broken when the pin
      variant was removed. `selection_ui.rs:16-17` has the same kind of
      breakage: "intersecting nodes \n widgets highlight live".

- [ ] **Method and module names in docs no longer match the code.**
      `mod.rs:191` and `geometry.rs:21` say `CanvasGeometry` is "reused by
      `frame`"; the method is `GraphUI::draw`. `pan_zoom/mod.rs:2` says it was
      "Split out of `graph_ui`"; that module is now `canvas`.

## Batch 3 — `new_node_ui`

One file, three items, all in `apply` / `palette_body` / the `PaletteEntry`
match. Two of them are work done per frame per pane whether or not the palette
is open.

- [ ] **`NewNodeUi::apply` computes the popup's sizing every frame for every
      pane whether or not the palette is open** (`new_node_ui.rs:150`:
      `ui.display().logical_rect()`, `max_height`, `scroll_cap`), while the
      genuinely expensive `local_def_rows` was correctly deferred into the body.

- [ ] **`local_def_rows` (`new_node_ui.rs:77`) allocates two `String`s per
      local definition, every frame the palette is up**, purely to escape
      `InternedStr` borrow guards; `palette_body` then allocates a third with
      `query.to_lowercase()` (`new_node_ui.rs:278`) each frame.

- [ ] **`PaletteEntry::Func` and `PaletteEntry::Special` produce the same intent
      from the same `Func`** — identical `node_id`, `pos`, and
      `default_bindings` construction — differing only in the `Node` value.
      `name()`/`category()` (`new_node_ui.rs:41-51`) already collapse the two
      through `SpecialNode::func()`.

## Batch 4 — Per-frame allocation in the record

Same class, different files: heap traffic on every frame a panel or menu is
open. Independent edits, but one sitting and one measurement.

- [ ] **`inspector.rs` rebuilds owned `String`s per panel line per frame** —
      `status_text` (`:395`), `value_str` (`:384`) per input, and
      `port_label_text`'s `format!` (`:360`) per port row.

- [ ] **`AnchoredMenu::show` clones the context-menu `Background` on every frame
      the menu is open** (`anchored_menu.rs:68`) to satisfy the builder.

## Batch 5 — `GraphUI` / `Gestures` shape

Coupled only because the first changes one of the bindings the second lists.

- [ ] **`PreviewDrag` (`preview_drag.rs:27-91`) is a single-field newtype around
      `GroupDrag` with one method.** The type, the file's struct/impl scaffolding,
      and the extra indirection buy nothing over the `GroupDrag` living directly
      in `Gestures` beside the one function that drives it — especially given
      `NodeUI` already stores its own bare `GroupDrag`.

- [ ] **`GraphUI::record_canvas` destructures `self` into 15 bindings, 8 of them
      `_`** (`mod.rs:368-376`). The `Gestures` grouping was introduced to make
      tab-switch resets a single assignment, and this is the cost: every field
      of both structs restated in a draw function that uses seven of them.

## Independent

The only user-visible item is first; the rest are local and unordered.

- [ ] **`background::build_tile` (`background.rs:127-146`) thresholds the dot
      hard** (`dx*dx + dy*dy <= r2`, no coverage falloff), leaving the tile
      aliased and relying on linear filtering to hide it. Every canvas frame
      shows these dots.

- [ ] **`ConnectionUI::apply` assigns `self.state` from three places on one
      path** (`connection_ui/mod.rs:91`, `:101`, `:127`).

- [ ] **`drag_candidates` (`connection_ui/mod.rs:345`) expresses "skip the
      output column under the modifier" by branching between two `&[PortKind]`
      slice literals and nested `flat_map`s**, rather than as the filter it is.

- [ ] **`selected_group_positions` (`drag_anchor.rs:147`) binds `positions`
      only to return it on the next line, and aliases `selection_holds` behind a
      single-use `holds` closure.**

- [ ] **`PortLayer::record` (`geometry.rs:140`) is a two-line wrapper that
      forwards to the free `snapshot` (`geometry.rs:383`), passing
      `&mut self.offsets` and the key back in.** The split serves no borrow
      constraint — `record` already owns `&mut self`.
