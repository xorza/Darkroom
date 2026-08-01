# Review — `darkroom/src`

Scope: simplification, consolidation, code reduction, unnecessary complexity.

**Delete each item once it is addressed.** This file lists open findings only —
no "done" markers, no resolved section.

Supersedes the previous pass. Findings from it that are still open are carried
below; ones the code has since answered are gone.

---

## Record functions grow to 100+ lines because the widget tree is written inline

Twelve functions exceed 100 lines. Some are legitimately long dispatches
(`build_step`'s per-variant match, the theme builders); the rest are recording
passes whose length comes from nesting closures rather than from doing more.

- [ ] `gui/pane/graph/mod.rs::record_canvas` — 138 lines, three closure levels
      (outer canvas → backdrop → inner canvas under the transform), with the
      cull, the effective selection, the `DrawCtx`, the wire pass, the node
      bodies, the panels and the in-flight previews all inside the innermost.
- [ ] `gui/pane/graph/node/mod.rs::draw_one` — 130 lines.
- [ ] `gui/pane/graph/node/header.rs::header` — 138 lines.
- [ ] `gui/pane/graph/paint/inspector.rs::draw_one` — 118 lines.
- [ ] `gui/widgets/inline_rename.rs::show` — 143 lines, and the idle and active
      branches barely share anything but the id.
- [ ] `gui/pane/preferences/mod.rs::model_row` — 128 lines for one settings
      row, most of it the path field's mirror/commit bookkeeping.
- [ ] `gui/window/mod.rs::frame` — 102 lines, the per-tab-kind dispatch buried
      inside the dock's content closure.

## `Theme` stores one sub-theme twice

- [ ] `static_value_editor_revealed` is a precomputed derived copy of
      `static_value_editor` stored beside its base, so the struct holds the
      same bundle twice with a comment asking that the pair not drift.

## `Badge` and `Chip` are two visual families, not one duplicated widget

Re-read side by side, the earlier claim that they differ only in constants
does not hold: `Badge` is a hollow bordered chip whose colour the caller owns,
tinted through an alpha ramp; `Chip` is a solid button whose colours come from
the theme and *invert* when toggled. Only the six-line record-and-tooltip tail
is common, and factoring that out would re-derive the widget. What remains is
narrower.

- [ ] `BadgeKind::Control { filled }` and `Chip::toggled` name the same
      two-state concept, and each still carries its own fill table — the
      policies differ on purpose, the naming does not have to.

## The removed `Scene` projection's vocabulary outlived it

The projection was replaced by the `GraphCtx` scope chain. The type references
are gone; the field paths and the noun are not.

- [ ] `gui/pane/graph/mod.rs:561` says node bodies paint in `scene.z_order` —
      the order is `main_view.item_placements`, and no `z_order` exists.
- [ ] `gui/pane/graph/gesture/selection/mod.rs:55` names `scene.selected`; the
      committed set is `GraphView::selected`, read via `GraphCtx::selected`.
- [ ] "the scene" is used across ~8 files to mean *what is currently drawn*.
      That reading is still coherent English, but with no `Scene` type left it
      no longer names anything, and several docs lean on it for a precise
      claim ("absence from the scene is not grounds for eviction").

## Widget-id minting has two idioms and two names

- [ ] 33 helpers end `_wid`, 3 end `_widget_id`. `node_wid` and
      `node_widget_id` sit in the same file, the latter a one-line alias for
      `node_wid("body", …)`.
- [ ] Most surfaces mint through a parameterized helper (`node_wid(tag, id)`,
      `port_wid(tag, port)`, `control_wid(node, key)`); `gui/pane/graph/toolbar.rs`
      instead declares five zero-argument functions, each hashing its own
      string literal with a hand-repeated `darkroom.graph.` prefix.

## Overlapping enums and one-line delegating wrappers

- [ ] `Document::apply_dock_op` (`core/document/mod.rs:266`) is a one-line
      delegate to `self.layout.apply(op)`.
- [ ] Graph visibility has three spellings: `Document::shows_graph`, the
      `GraphCtx::is_visible` field, and `GraphUI::visible` (a cached copy kept
      only for edge detection).

## `GraphView::item_placements` carries two jobs

- [ ] It is both the position map and the paint-stack order, which is why
      `GraphView` needs a hand-written `PartialEq` comparing it as a sequence
      and why `Raise` is expressed as `move_item_to_index`.
- [ ] The same field duplicates the graph's node set; `Document::validate`
      exists partly to enforce the 1:1 correspondence, and
      `Document::remove_node` exists because the invariant spans two fields.

## Known gaps, stated rather than fixed

- [ ] Nothing enforces membership of the once-a-frame cache sweep
      (`App::reconcile_derived_state`). A fourth `NodeId`-keyed cache that
      outlives the scene compiles, runs, and leaks entries for deleted nodes
      with no diagnostic — the table on `Document::holds_node` is the only
      signal, and only to a reader who goes looking.
- [ ] `CanvasCtx::without_gesture` derives a whole context to suppress one flag
      for one reader, adding a fourth way the same frame can be described.
