# Wire ties

Wire ties as `.notes/wire-ties.md` decides them. This plan starts after `HARNESS_PLAN.md` lands:
each batch that changes what the canvas shows is checked with the editor harness, by a session on a
project and a `Shot` of the canvas.

Each batch leaves `cargo drive check` and `cargo drive test` green.

## Plan

### 1. The document holds ties and items

Files: new `darkroom/src/core/document/tie.rs`, `tie_id.rs`, `wire_key.rs`, `item_id.rs`;
`darkroom/src/core/document/graph_view.rs`, `mod.rs`, `error.rs`, `load_report.rs`; and the 27
reads of `selected` (`core/edit/graph_intent/mod.rs`, `core/document/mod.rs`,
`core/edit/step/set_node.rs`, `core/edit/step/set_selection.rs`, `gui/graph_ctx/`,
`gui/pane/graph_ui/` and its `node_ui/` and `gesture/`).

- [ ] `TieId`: a `common::Id`, made with `TieId::unique()` as `NodeId::unique()` is. `Tie { pos:
  Vec2, angle: TieAngle }` and `enum TieAngle { Auto, Pinned(f32) }`, with the pinned angle in
  degrees, in `[0, 360)`. `WireKey`: `Data(InputPort)` or `Event(Subscription)`, `Ord` for a
  `BTreeMap` key. `ItemId`: `Node(NodeId)` or `Tie(TieId)`.
- [ ] `GraphView`: `ties: BTreeMap<TieId, Tie>`, `routes: BTreeMap<WireKey, Route>` with
  `Route(Vec<TieId>)`, and `selected: BTreeSet<ItemId>`. A `Route` is never empty, and it never
  names a tie twice; `Route::checked` returns the error and `Route::new` panics, as the
  conventions ask.
- [ ] `GraphView::validate` takes the wiring: each route names a wire the graph holds, with the
  same output for a data wire; each step names a tie; each tie has at least one route; each
  position and pinned angle is finite. New `GraphViewValidationError` cases, one for each.
- [ ] The load repairs each case and reports it: `DocumentRepair::RouteDropped { wire }` for a
  route to no wire, `RouteStepDropped { wire, tie }` for a step to no tie (the route goes when it
  is left empty), `TieDropped { tie }` for a tie that no route names, `TiePosReset { tie }` for a
  non-finite position. Test: one document with each fault loads into one consistent view, with
  the report listing each repair in order.
- [ ] The selection's readers take `ItemId`. The node-only readers (Ctrl+D, the node menu,
  `SetNode` capture, the header's selected look) filter to `ItemId::Node`. `GraphIntent::click`
  and `SetSelection` take items. The existing selection tests keep their assertions, with
  `ItemId::Node` around each id.

### 2. Edits keep ties and wiring consistent

Files: new `darkroom/src/core/edit/step/set_ties.rs`; `step/undo_step.rs`, `step/set_input.rs`,
`step/set_subscription.rs`, `step/set_node.rs`, `step/set_positions.rs`,
`graph_intent/mod.rs`, `graph_intent/drag_start.rs`, `validate.rs`, `error.rs`.

- [ ] `SetTies`, one step kind for every tie edit: `ties: Vec<(TieId, Change<Option<Tie>>)>` and
  `routes: Vec<(WireKey, Change<Option<Route>>)>`. Adding, removing, moving or turning a tie, and
  adding or releasing a wire, are each one `SetTies`. `dirties_document` is true, and
  `invalidates_cached_geometry` and `changes_node_set` are false. Test: apply then revert leaves
  the document equal to the original, for each edit.
- [ ] The wiring steps carry the routes they end. `SetInput`, `SetSubscription` and `SetNode`
  hold a `SetTies` that `into_step` captures. It holds the route of each wire that the step ends
  or rewires, and each tie that is left with no route. The step writes it in the same direction
  as its own change. Test: rewiring a tied input to another output drops its route; removing a
  node drops the routes of every wire on it and removes the tie that only those wires held; one
  revert brings all of it back; rewiring to the same output keeps the route.
- [ ] Intents: `TieWires { tie: TieId, pos: Vec2, wires: Vec<RouteInsert> }`, where
  `RouteInsert { wire: WireKey, at: usize }` is the route index of the crossed piece;
  `JoinTie { tie, wires: Vec<RouteInsert> }`; `RemoveTie { tie }`; `SetTieAngle { tie, angle:
  TieAngle }`; `ReleaseWire { tie, wire }`. `into_step` refuses a wire the graph does not hold, an
  insertion index past the route's end, and a tie already on that route, as `MalformedIntent`
  cases. A tie gone since the gesture started is stale, so it gives no step.
- [ ] The group drag moves items: `DragStart { item: ItemId, pos }`, and `Move` in
  `SetPositions` takes an `ItemId`. The latch adds each tie whose every wire has both end nodes
  in the moving set. Test: a drag of two nodes takes the tie between them and leaves a tie that
  one outside wire also holds.
- [ ] `GraphIntent::duplicate` clones each tie whose every wire has both end nodes in the set, at
  `+DUPLICATE_OFFSET` with its angle, with the routes of the cloned wires through the clone.
  Test: with `IncomingWires::Keep`, a tie that also holds a kept incoming wire is not cloned.
- [ ] Delete removes the selected ties with `RemoveTie`, beside `RemoveNode` for the nodes.

### 3. Tied wires and bands are drawn

Files: new `darkroom/src/gui/pane/graph_ui/draw/tie/` (`tie_layout.rs`, `band.rs`,
`tests.rs`); `draw/wire/mod.rs`, `gesture/wire_ui/mod.rs`, `gesture/subscription_ui.rs`,
`frame/cull_region/`, `gesture/breaker_ui/breaker_probe.rs`, `draw_ctx.rs`, `theme/`.

- [ ] `TieLayout`, rebuilt each frame into buffers it keeps, so a settled frame still allocates
  nothing (`a_settled_frame_records_without_allocating`). For each tie it holds the centre, the
  strand direction (pinned, or the normalised mean of each member's normalised neighbour chord),
  the band axis, and each member's slot. The slot order is the barycenter of the two neighbouring
  points projected on the axis, ties broken by `WireKey`. A second pass uses the neighbours' slots
  in place of their tie centres, so chained ties keep one order. Slots are 9 units apart and
  centred.
- [ ] A wire is pieces: `WirePath` holds one `Wire` per piece. An untied wire is one piece by
  `Wire::data` or `Wire::event`, as now. A tied piece has its tie-end tangent along the strand
  direction, signed by the wire's travel, and handles of `clamp(0.4 · chord, 24, 140)`. A ribbon
  piece between two ties takes the chord between the tie centres, so its strands share their
  handles. `WirePass::draw_wire` culls, probes and paints each piece. The ramp splits at the
  piece ends by chord length, so the colours of a typed wire still run from end to end.
- [ ] The band paints after the wires and before the node bodies: a halo of `canvas_bg`, the
  band and the head as thick round-capped lines of `text_muted`, `text` on hover or selection,
  then the selection ring and the knob (hollow `selection_rect` for `Auto`, filled
  `status_warning` for `Pinned`). The breaker cuts the strands it crosses, as it cuts other wires.
- [ ] Tests, with the maths shown in each: the slot order of four strands from the barycenters
  in the playground's mini scene (sources at y 83, 105, 213, 303 and inputs at y 243, 83, 265,
  105 give the order m2, m1, m4, m3); the auto direction of two mirrored strands is exactly
  `(1, 0)`; a pinned 90° band has its strands along `+x`; the tangent at a slot is the same on
  both pieces (G1); the slot offsets for three strands are −9, 0 and +9.
- [ ] Check with the harness: the "after" scene from the canvas, in a project, as a `Shot`,
  compared by eye with the mockup.

### 4. The tie line, the tie and its knob

Files: new `darkroom/src/gui/pane/graph_ui/gesture/tie_ui/` (`mod.rs`, `tests.rs`) and
`node_ui`-style `tie_widget.rs`; `gesture/canvas_gesture.rs`, `gesture/mod.rs`,
`gesture/selection_ui/`, `gesture/group_drag/`, `gesture/shortcuts.rs`, `graph_ui/mod.rs`.

- [ ] `CanvasGesture::Tie(PointerButton)`: Shift+RMB-drag and Ctrl+Shift+LMB-drag, checked before
  the breaker's arms. Test: the classify table — RMB-drag is the breaker, Shift+RMB-drag is the
  tie line, Ctrl+LMB-drag is the breaker, Ctrl+Shift+LMB-drag is the tie line, Shift+LMB-drag
  still adds to the band selection.
- [ ] `TieUi`, as `BreakerUi`: a slot that holds the latched button, and a line whose buffers
  outlive the gesture. While it is in flight, the standing wires fade (`WireSet::Faded`), each
  crossed piece paints `selection_rect` with a mark at its crossing, and a crossed tie lights.
  Release emits `TieWires` at the mean of the crossings, or `JoinTie` on the first crossed tie
  along the line with every other crossed wire, then selects the tie. Esc drops the line. Test:
  the intents for a line across four wires; for a line across a tie and one more wire; for a line
  across nothing.
- [ ] The tie is a widget for each visible tie, its rect the band's bounding box grown by the
  head and a margin: a hover target, so hovering lights its wires through `WireEmphasis` as an
  endpoint hover does. A press selects it by `ClickMode` and starts the group drag. Band select
  takes a tie whose centre is in the band.
- [ ] The knob is a widget on the selected tie: its drag sets `SetTieAngle { Pinned }` from the
  pointer's angle about the centre, rounded to 15° with Shift, and its double-click sets `Auto`.
  The frames of one drag fold into one undo entry, as the group drag's do. Test: the pointer at
  52° gives 52° without Shift and 45° with it.

### 5. The tie's menu

Files: new `darkroom/src/gui/pane/graph_ui/draw/tie_context_menu.rs`; `draw/anchored_menu.rs`,
`graph_ui/mod.rs`.

- [ ] RMB-click on a tie opens the anchored menu: Release ▸ one entry per wire, named
  `<producer>.<output> → <consumer>.<input>` (or the event's name), Pin or Free angle, and Remove
  tie. Hovering a Release entry lights that strand. Test: each entry emits its intent, and a
  release of the tie's last wire removes the tie with it.

### 6. Fold the decisions into the documents

Files: `darkroom/AGENTS.md`, `.notes/TODO.md`, `.notes/wire-ties.md`, `WIRE_TIES_PLAN.md`.

- [ ] `darkroom/AGENTS.md`, "Wire ties": each decision of `.notes/wire-ties.md`, with what it
  refuses and its source.
- [ ] Delete the "wire zips" item in `.notes/TODO.md`, then delete `.notes/wire-ties.md` and
  `WIRE_TIES_PLAN.md`.
