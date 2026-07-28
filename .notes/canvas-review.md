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
