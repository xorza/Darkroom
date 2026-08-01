# Review — `darkroom/src`

Scope: data-structure canonicality, responsibility isolation, state management.

**Delete each item once it is addressed.** This file lists open findings only —
no "done" markers, no resolved section.

---

## `NodeId`-keyed derived state has no enforced membership

The three caches derived from the open document now share one liveness rule
(`node_alive`) and one sweep (`App::reconcile_derived_state`), listed on
`Document::holds_node`. What is still missing is anything that *makes* a new
one join them.

- [ ] Nothing enforces the list. A fourth `NodeId`-keyed cache that outlives
      the scene compiles, runs, and leaks entries for deleted nodes with no
      diagnostic — the doc on `Document::holds_node` is the only signal, and
      only to a reader who goes looking.

## The read-only context chain is five wrappers deep, mostly forwarding

`AppCtx` → `GraphCtx` → `CanvasCtx` → `DrawCtx` → `NodeCtx`, 52 accessor
methods across the four files, of which the majority exist only to re-expose a
ref held one level up.

- [ ] `theme()` is defined five times — `gui/app/ctx.rs:49`,
      `gui/graph_ctx/mod.rs:144`, `gui/pane/graph/ctx.rs:64`,
      `gui/pane/graph/ctx.rs:155`, `gui/pane/graph/node/ctx.rs:52` — four of
      them a single delegating call. `geometry()`, `hits()` and `graph_ctx()`
      repeat the same pattern across the lower three.
- [ ] Each level's doc justifies itself as "answering everything the level
      below does", which is a statement that the layer adds forwarding. The
      genuinely new field per level is one or two (`CanvasCtx`: gesture +
      cancelled; `DrawCtx`: selection, inspectors, cull; `NodeCtx`: hovered).
- [ ] `CanvasCtx::without_gesture` (`gui/pane/graph/ctx.rs:95`) constructs a
      derived context to suppress one flag for one reader, adding a fourth way
      the same frame can be described.

## `Theme` is one flat struct for four unrelated concerns

`gui/theme/mod.rs:335`, 23 top-level fields in a 1009-line file.

- [ ] Layout metrics (`node_min_width`, `port_gap`, `port_cols_gap`,
      `canvas_dot_spacing`, `new_node_popup_max_height`, …) sit in the same
      struct as the colour palette, as per-widget sub-themes, and as the entire
      embedded `palantir::Theme`. Nothing in the type separates "how big" from
      "what colour" from "which widget".
- [ ] Field *order* is load-bearing for reasons unrelated to meaning — a
      comment at the top of the struct explains that scalars must precede
      tables or TOML serialization errors with `ValueAfterTable`. The
      serialization format constrains the in-memory layout.
- [ ] `static_value_editor_revealed` (`gui/theme/mod.rs:400`) is a precomputed
      derived copy of `static_value_editor` stored beside its base, so the
      struct holds the same data twice with a comment asking that the pair not
      drift.

## Two widget types render the same chip

- [ ] `Badge` (`gui/widgets/badge.rs:97`) and `Chip` (`gui/widgets/toolbar.rs:76`)
      are both "fixed square, `Sense::CLICK`, background fill varying on
      hover/toggled, glyph as either a font char or a `fn(&mut Ui, Color)`,
      `tooltip_after`, returns clicked". They differ in the size constant
      (`BADGE_SIZE` vs `BUTTON_SIZE`), where the colours come from (caller vs
      `Theme`), and nothing else structural.
- [ ] `BadgeKind::Control { filled }` and `Chip::toggled` are the same
      two-state concept under two names, each with its own alpha/fill table.
- [ ] `GoBadge` (`gui/widgets/badge.rs:54`) exists solely because one badge
      resolves its colour from last frame's hover rather than taking it as a
      parameter — a third type for a one-line difference.

## Three mechanisms for "buffered text field that commits on blur or Enter"

- [ ] `EditBuffer` (`gui/widgets/buffered_edit/mod.rs:20`) exists specifically
      to be the shared core, and is used by `inline_rename` (via `RenameState`)
      and `value_editor::buffered_text_edit`.
- [ ] `PathField` (`gui/pane/preferences/mod.rs:201`) reimplements the same job
      with a different shape — `text` / `seen` / `problem`, mirroring on
      external change rather than on unfocused — and does not use `EditBuffer`.
- [ ] Commit detection differs per site: `EditBuffer::blur_edge` in two places,
      raw `resp.submitted || resp.lost_focus` in `model_row`. The `blur_edge`
      doc explains at length why the raw form is wrong for a
      `request_focus`-driven widget; nothing marks which sites are exempt.

## `Viewport`'s doc describes one of its two uses

Not a type-safety hole, contrary to how this was first written: `Viewport` is a
sound shared abstraction — an affine camera satisfying `local = pan + zoom *
content` — and both users satisfy it. The algebra really is shared
(`pan_zoom::zoom_about` and `fold_scroll_zoom` are unit-agnostic, and the
viewer imports both), so splitting the type would duplicate that math or force
a generic for no gain. What is wrong is narrower.

- [ ] `Viewport` (`core/document/mod.rs:108`) documents itself as "a graph's
      camera: pan offset (canvas-local px)". The image viewer's
      `view: Option<Viewport>` (`gui/pane/viewer/mod.rs:65`) holds pan as the
      image's top-left offset in *pane-local* px and zoom as *display px per
      texel*, so a reader arriving from there is told the wrong units for the
      field in front of them.
- [ ] `Viewport::is_valid` validates the *persisted graph* camera — it backs
      `GraphViewValidationError::InvalidViewport` — but sits on the shared type
      with nothing saying so. The viewer never calls it.

## Doc comments describe a projection layer and a module tree that no longer exist

The `Scene` / `SceneNode` projection was replaced by the `GraphCtx` scope
chain, and `gui::canvas` / `gui::node` were replaced by `gui::pane::graph`.
Neither rename reached the prose.

- [ ] `SceneNode::pos` — `gui/pane/graph/frame/geometry/mod.rs:68`,
      `gui/pane/graph/gesture/pan_zoom/mod.rs:210` and `:243`.
- [ ] `SceneNode::runnable` — `gui/app/commands/run.rs:51`.
      `SceneNode::cache_controls` — `gui/pane/graph/node/header.rs:286`.
- [ ] "the per-frame `Scene` projection" — `core/document/mod.rs:105`.
- [ ] `gui::canvas::background` — `gui/theme/mod.rs:348`;
      `gui::canvas::GraphUI` — `core/document/dock/mod.rs:27`.
- [ ] `gui::node::memory_row` / `gui::node::preview_row` —
      `gui/widgets/support.rs:49-50`; `gui::node::port_rename` /
      `gui::node::value_editor` — `gui/widgets/inline_rename.rs:5,7,24` and
      `gui/widgets/buffered_edit/mod.rs:2`.
- [ ] `TerminalSession::tick` named as a host loop — `core/worker.rs:5`. No
      such frontend exists.
- [ ] `DockStep` and `build_doc_step` — `core/edit/intent/mod.rs:6,8` and
      `intent/types.rs:1`. (Already tracked in `ISSUES.md`.)

## State kept but never read

- [ ] `StatusLog::lines` (`core/status/mod.rs:19`) maintains a 200-entry
      rolling `VecDeque` with its own cap logic. Production never reads it —
      the status bar shows the `error` slot alone, and every entry is already
      emitted through `tracing`. The only reader is the `internals` mod.
- [ ] `BackgroundRuntime`'s module doc (`core/background_runtime.rs:7,16`)
      describes the type as capturing a pattern shared by "both owners" and
      tells callers to "declare it after" their inner value. There is one
      owner (`WorkerBridge`), and it is generic machinery built for a second
      that does not exist.

## Overlapping enums and one-line delegating wrappers

- [ ] `Request` and `DocumentRequest` (`gui/requests.rs:27` and `:38`) declare
      the `Graph` and `View` variants twice, with a `map` between them and an
      `unreachable!` arm in each drain.
- [ ] `Document::apply_dock_op` (`core/document/mod.rs:255`) is a one-line
      delegate to `self.layout.apply(op)` carrying a doc comment about
      undo policy that belongs to the caller.
- [ ] `GraphUI::retain_nodes` (`gui/pane/graph/mod.rs:171`) is a one-line
      delegate to `self.geometry.retain_nodes`.
- [ ] Graph visibility has three spellings: `Document::shows_graph`
      (`core/document/mod.rs:249`), the `GraphCtx::is_visible` field, and
      `GraphUI::visible` (`gui/pane/graph/mod.rs:89`, a cached copy kept purely
      for edge detection).

## Dual-purpose containers

- [ ] `GraphView::item_placements` (`core/document/mod.rs:144`) is an
      `IndexMap` doing two jobs at once — the position map *and* the paint
      stack order — which is why `GraphView` needs a hand-written `PartialEq`
      that compares it as a sequence, and why `Raise` is expressed as
      `move_item_to_index`.
- [ ] The same field duplicates the graph's node set. `Document::validate`
      exists partly to enforce the 1:1 correspondence, and `Document::remove_node`
      exists because that invariant spans two fields.

## Visibility inconsistency

- [ ] `Preferences` (`core/io/preferences/mod.rs:23`) is `pub(crate)` but every
      field is declared `pub`, unlike the `pub(crate)` fields used throughout
      the rest of the crate.
