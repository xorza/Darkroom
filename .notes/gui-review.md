# `darkroom/src/gui` — review findings

Scope: every file under `darkroom/src/gui/` (production code only; test files
and the APIs tests reach for were not reviewed). Supersedes the old
`canvas-review.md`, whose surviving items are folded in below with refreshed
line anchors.

**When you address an item, delete it from this file.** These are observations,
not designs — each says what is wrong, not how to fix it.

Grouped by shared root cause, highest impact first. Items under *Independent*
have no coupling and can go in any order, alone.

---

## The single `AppCommand` slot, arbitrated four different ways

One command leaves a frame, but every level that funnels into that slot picks
a different winner rule, and no two agree. This is the widest cross-cutting
shape in the module.

- [ ] **`main_window::claim` (`main_window.rs:47`) is first-claim-wins, while
      every producer it arbitrates over is last-write-wins internally.**
      `menu_bar::show` (`menu_bar.rs:20-27`), `menu_bar::file_menu`
      (`:59-82`), `preferences_view::show` (`preferences_view.rs:43-129`) and
      `graph_toolbar::show` (`graph_toolbar.rs:64-156`) all overwrite
      `command` on each hit. `Editor::frame` then adds a third rule,
      `.or(command_from_shortcut)` (`app/editor/mod.rs:412`), and
      `resolve_gestures` a fourth, `.or(...).or_else(...)`
      (`canvas/mod.rs:364-369`).

- [ ] **`preferences_view::show` can drop a `PickMlModel` behind a later
      `Changed`.** Five separate `command = Some(...)` assignments
      (`preferences_view.rs:78`, `:89`, `:97`, `:105`, `:118`); the second
      model row's `Changed` overwrites the first row's Browse request in the
      same frame.

- [ ] **Eleven sites plumb `Option<AppCommand>` by hand** — `menu_bar::show`,
      `menu_bar::dropdown`, both menu builders, `preferences_view::show`,
      `graph_toolbar::show`, `GraphUI::draw`, `GraphUI::resolve_gestures`,
      `emit_chip_command`, `NodeMenuUi::apply`, `GraphMenuUi::apply`,
      `MainWindow::frame`, `Editor::frame`, `App::record` — each with its own
      local, its own assignment style, and its own precedence comment.

## `GraphUI` / `Gestures` shape

The controller roster is restated in four places, and the record function pays
for it.

- [ ] **`GraphUI::record_canvas` destructures `self` into 15 bindings, 7 of
      them `_`** (`canvas/mod.rs:401-421`). The `Gestures` grouping was
      introduced to make tab-switch resets a single assignment, and this is
      the cost: every field of both structs restated in a draw function that
      uses eight of them.

- [ ] **Adding one gesture controller touches four enumerations** — the
      `Gestures` struct (`canvas/mod.rs:137-154`), `prepass`
      (`:234-267`), `resolve_gestures` (`:332-369`), and the
      `record_canvas` destructure (`:401-421`) — with nothing tying them
      together.

- [ ] **`PreviewDrag` (`canvas/preview_drag.rs:26-88`) is a single-field
      newtype around `GroupDrag` with one method.** The type, the file's
      struct/impl scaffolding, and the extra indirection buy nothing over the
      `GroupDrag` living directly in `Gestures` beside the one function that
      drives it — especially given `NodeUI` already stores its own bare
      `GroupDrag`.

## Near-identical bodies written twice

Each pair below differs in one axis and is otherwise a copy, so a change to
the shared part has to be made in both.

- [ ] **`RunState::apply_node_patch` (`run_state.rs:191-217`) and
      `replace_results` (`:220-266`) carry the same
      `NodeExecutionStatus → ExecStatus` match**, ~14 lines apiece, differing
      only in the `Running` arm (mapped vs. `panic!`).

- [ ] **`input_label_cell` (`node/port_row/mod.rs:270-361`) and `output_cell`
      (`:412-463`) repeat the same menu plumbing** — read
      `(cell.response.id, cell.response.right.clicked())`, call
      `open_port_context_menu`, open a hug-sized `ContextMenu::for_id`, end
      with `remove_port_item`. Their `CellOpts` construction
      (`:167-170`, `:187-190`) is likewise mirrored.

- [ ] **`CanvasGeometry::rebuild` (`canvas/geometry.rs:305-356`) and
      `replay_cached` (`:363-375`) walk the same three glyph domains in the
      same order** with the same `n.sink` guard, one calling `record` and the
      other `replay`.

- [ ] **Palantir's four `WidgetLook` states are hand-listed in five places** —
      `theme.rs:449-456`, `theme.rs:499-509`, `theme.rs:579-588`,
      `widgets/inline_rename.rs:246-252`, `preferences_view.rs:267-271`
      (which lists only three of the four, silently). Each is the same
      "restyle every state" loop over a literal array.

## Per-frame allocation in the record path

Heap traffic on every frame something is on screen. Independent edits, but
one sitting and one measurement.

- [ ] **`inspector.rs` rebuilds owned `String`s per panel line per frame** —
      `status_text` (`:395`), `value_str` (`:384`) per input, and
      `port_label_text`'s `format!` (`:360`) per port row.

- [ ] **`AnchoredMenu::show` clones the context-menu `Background` on every
      frame the menu is open** (`canvas/anchored_menu.rs:79`) to satisfy the
      builder.

- [ ] **`status_bar::memory_label` formats twice per frame**
      (`status_bar.rs:69`, and `push_str(&format!(...))` at `:72` allocating a
      throwaway `String`), for a readout that only changes on a
      once-per-second sample.

- [ ] **The node body's footers rebuild their labels every frame** —
      `fmt_elapsed` in `header::status_row` (`node/header.rs:230`) per running
      node, `fmt_bytes` twice in `memory_row` (`node/memory_row.rs:70`), and
      three `format!`s in `preview_row::info_row`
      (`node/preview_row.rs:106`, `:109`, `:116`).

- [ ] **`image_viewer::header` builds its readout `String` per frame**
      (`image_viewer.rs:277-293`), including a `write!` for the zoom clause.

- [ ] **`value_editor::show` collects a `Vec<&str>` of option names per frame**
      for every visible dropdown — `:66-69` for value variants, `:146` for
      enums — to hand `ComboBox` a slice.

- [ ] **The dock rebuilds label state per frame**: `MainWindow::frame` collects
      a `HashMap<NodeId, String>` of viewer labels (`main_window.rs:134-137`),
      `tab_labels` a `Vec<TabLabel>` per group (`dock/mod.rs:371-384`), and
      `drop_target` a `Vec<Rect>` of chip rects per frame while a tab drag is
      live (`dock/mod.rs:310-314`).

## State reached through its owner instead of asked for

Three layers each mutate a neighbour's fields directly, so the invariants
those owners document have no enforcement point.

- [ ] **`Editor` reaches two levels into the UI tree.**
      `sync_image_viewers` retains on `self.main_window.image_viewers`
      (`app/editor/mod.rs:618-623`), and `frame` calls
      `self.main_window.graph_ui.take_node_menu_action()` (`:418`) — both
      through `pub(crate)` fields on `MainWindow` (`main_window.rs:62-67`)
      that exist only for those reach-throughs.

- [ ] **`RunState` clears `PreviewStore::entries` directly without raising
      the store's reconcile flag** — `clear` (`run_state.rs:365`) and
      `clear_cache_projections` (`:286`). The field is `pub(crate)`
      (`preview_store.rs:26`) precisely so this can happen, which defeats the
      `needs_reconcile` gate the store documents as its own
      (`preview_store.rs:28-34`).

- [ ] **`RunState::drop_empty_nodes` (`run_state.rs:272-275`) claims to be
      "the one definition of empty" but tests three of `NodeRunState`'s four
      payload fields** — `errors` is not considered.

## Independent

The only user-visible item is first; the rest are local and unordered.

- [ ] **`background::build_tile` (`canvas/background.rs:127-146`) thresholds
      the dot hard** (`dx*dx + dy*dy <= r2`, no coverage falloff), leaving the
      tile aliased and relying on linear filtering to hide it. Every canvas
      frame shows these dots.

- [ ] **`image_viewer::show` derives `(Option<ShownImage>, Option<&str>)` from
      a four-level nested match** (`image_viewer.rs:134-176`) whose two arms
      are documented as complementary but are spelled as an unconstrained
      tuple.

- [ ] **`value_editor::show` uses `read_only_label` as an `else`-escape five
      times** (`node/value_editor.rs:91`, `:97`, `:103`, `:111`, `:123`), plus
      two direct arms — a function that exists only to return `None` after
      drawing.

- [ ] **`ConnectionUI::apply` writes `self.state` from six places on one
      path** (`canvas/connection_ui/mod.rs:98`, `:112`, `:121`, `:146`,
      `:200`, `:234`/`:238`).

- [ ] **`drag_candidates` (`canvas/connection_ui/mod.rs:359`) expresses "skip
      the output column under the modifier" by branching between two
      `&[PortKind]` slice literals and nested `flat_map`s**, rather than as
      the filter it is.

- [ ] **`selected_group_positions` (`canvas/drag_anchor.rs:147`) binds
      `positions` only to return it on the next line, and aliases
      `selection_holds` behind a single-use `holds` closure.**

- [ ] **`PortLayer::record` (`canvas/geometry.rs:140`) is a two-line wrapper
      that forwards to the free `snapshot` (`canvas/geometry.rs:383`), passing
      `&mut self.offsets` and the key back in.** The split serves no borrow
      constraint — `record` already owns `&mut self`.

- [ ] **`CardBorder` (`theme.rs:690-693`) is a named struct around one
      `Color` field with one caller**, which immediately projects it
      (`node/mod.rs:214`).

- [ ] **`palette_struct!` (`theme.rs:258-274`) is a macro with exactly one
      instantiation**, and the two roster fields it cannot cover
      (`TYPE_COLORS`, the `PAL_*` swatches) are kept in step by separate
      hand-written consts (`theme.rs:522-545`).

- [ ] **`dock/strip.rs:127` hard-codes palantir's default
      `TextStyle::line_height_mult` as a local const** so `NEW_TAB_CHIP_SIDE`
      can be a `const`; nothing links the copy to the original.

- [ ] **`dialogs.rs` honours a starting directory for project and graph
      pickers but not for node inputs.** `file_dialog` (`:19-25`) takes
      `start`; `filtered_file_dialog` (`:56-62`) does not, so
      `pick_existing_file` / `pick_existing_files` / `pick_new_file` /
      `pick_directory` always open wherever the OS last left them.

- [ ] **`Badge` (`widgets/badge.rs`) and `Chip` (`widgets/toolbar.rs`) are two
      chip widgets with the same job** — fixed square, hover-lifted fill,
      optional toggled state, centered glyph, hover tooltip via
      `tooltip_after` — differing in size constant and fill policy.

- [ ] **35 `*_wid` free functions wrap 33 `WidgetId::from_hash` calls** across
      the module, each restating its own string tag; the tag and the key tuple
      are the only thing that varies.

- [ ] **`StatusInputs` (`app/mod.rs:48-52`) exists only to keep
      `Editor::frame` under clippy's argument limit**, as its own doc states.

- [ ] **`CanvasHits` uses `Option::get_or_insert` for its side effect eight
      times** (`canvas/hits.rs:180`, `:192`, `:200`, `:235`, `:248`, `:267`,
      `:274`, `:278`), discarding the `&mut` it returns.

- [ ] **`preview_drag_modifier` (`canvas/mod.rs:549`) takes `&mut Ui` to read
      `ui.modifiers()`**, forcing every caller to hold a mutable borrow for a
      pure read.

- [ ] **Three record functions run past 100 lines with several unrelated
      concerns each**: `header::status_row` (`node/header.rs:200-331` — time
      label, spinner, spacer, four chip families),
      `preferences_view::model_row` (`preferences_view.rs:203-324` — state
      mirroring, error styling, three widgets, command production), and
      `Inspectors::draw_one` (`canvas/inspector.rs:147-281`).

- [ ] **`app/mod.rs:140` is a commented-out debug line**
      (`// ui.debug_overlay.damage_rect = true;`).
