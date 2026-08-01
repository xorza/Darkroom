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

## The context chain's forwarding is a phase ladder, not redundancy

Corrected from the first pass, which read the five levels as mostly-forwarding
nesting. They are not interchangeable: each gates on what the frame has
resolved by the point it can be built, and the forwarding is what lets a
late-phase reader ask an early-phase question through one handle.

```
navigate   scan_hits           GraphCtx     (geometry stale, no gesture yet)
prepass    geometry.rebuild()  CanvasCtx    (+ geometry, hits, gesture, cancelled)
record     cull + selection    DrawCtx      (+ selection, inspectors, cull)
```

`DrawCtx::new` has exactly one call site, inside `record_canvas`. Hoisting it
would hand the hit sweep a cull region for an unfolded viewport and a gesture
that has not been classified — so the levels cannot collapse without moving
behaviour. Only one thing in the group survives:

- [ ] `CanvasCtx::without_gesture` (`gui/pane/graph/ctx.rs:95`) constructs a
      derived context to suppress one flag for one reader, adding a fourth way
      the same frame can be described — unrelated to the phase ladder above.

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
