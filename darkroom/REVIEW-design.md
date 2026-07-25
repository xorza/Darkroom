# Darkroom — simplification / consolidation review

**When you address an item, delete it from this file.** A fixed finding should
leave no residue here.

Scope: production code under `darkroom/src` (tests and their fixtures ignored),
with extra depth on `gui/canvas/`. The lens is *simplification, consolidation,
optimization, code reduction, unnecessary complexity, bad design* — not
correctness. Ordered by severity × payoff.

References are `file::symbol` rather than line numbers, which rot on the first
edit.

---

## High

- [ ] **The canvas prepass is ~19 independent full-scene sweeps per frame, none
  of them culled.** Separate loops walk `scene.nodes` or
  `scene.pinned_outputs()` and poll `Ui::response_for` on deterministic ids:
  `node/prepass.rs`'s `emit_graph_opens` / `emit_play_clicks` /
  `emit_cache_evictions` / `emit_path_picks` / `emit_port_dblclicks`;
  `canvas/geometry.rs::CanvasGeometry::rebuild` (nodes × ports × events × subs);
  `connection_ui`'s `scan_drag_start` / `scan_snap_target` /
  `dropped_on_empty_canvas`; `subscription_ui`'s four scans; `pin_ui`'s
  `scan_port_drag_start` / `scan_widget_drag_start` / `emit_pin_refresh_clicks` /
  `emit_pin_image_opens`; `node_menu::apply`; `graph_menu::apply`;
  `inspector`'s `Inspectors::apply` and `outside_action`; and
  `selection_ui`'s rubber-band sweep. 69 `response_for` call sites in the GUI
  tree. `CullRegion` gates only *paint*, so an off-screen graph pays every sweep
  in full, and several run with no gesture in flight. This is the canvas's fixed
  per-frame floor and it grows linearly with graph size on every idle frame.

- [ ] **Three wire-gesture controllers re-implement the same state machine.**
  `ConnectionUI`, `SubscriptionUI` and `PinUi` each own an `Option<InFlight>`, a
  "first glyph whose drag started" scan, a per-frame snap-target scan, an Esc
  cancel, a release-edge resolve keyed off `dragging()` flipping false, a
  stale-endpoint drop, and a `draw_in_flight` preview. The *group-drag* half is
  now fully shared (`drag_anchor::GroupDrag` owns latch → advance → commit) and
  the committed-wire renderers are free functions over a shared `WirePass`, so
  what remains is specifically the in-flight gesture lifecycle.

- [ ] **Script-message policy is implemented twice, once per frontend.**
  `gui/app/mod.rs::App::handle_script_inbound` and
  `core/terminal_session/mod.rs::TerminalSession::tick` match the same four
  `ScriptMessage` variants with the same semantics (prefix prints with
  `"script: "`, apply a batch and report refusals, coalesce repeated `RunOnce`
  into one run, quit on `Shutdown`). The commit half is duplicated too:
  `Editor::apply_external_intents` and the free `terminal_session::apply_intents`
  both resolve `active_target().unwrap_or(Main)` and fold `Refusal::Invalid`
  reasons into a `Vec<String>`. A shared handler returning "what the frontend
  must do" would leave only the run/quit wiring per shell.

- [ ] **Every run action passes through three forwarding layers.**
  `RuntimeHost`'s `run_once` / `run_node` / `evict_cache` / `start_event_loop`
  are four copies of "compile, bail on `None`, install, send one message".
  `Workspace`'s four same-named methods are one-line forwarders whose only job
  is supplying `&self.open.document.graph`. `commands/run.rs`'s `run_graph` /
  `start_events` / `stop_events` are three more. The project's own style rule
  discourages thin wrapper methods; this is one call restated at three
  altitudes.

- [ ] **Port tooltip text is rebuilt and then cloned, per port, per frame.**
  `node/port_row/mod.rs`'s `input_label_cell` / `output_cell` build
  `type_label(..)` (a `String`) and wrap it in `port_tip` (another `String`) for
  every visible port; `port_row/glyph.rs::circle_frame` then hands it to
  `tooltip_after` as `tip.to_owned()` — a third allocation. `event_cell` does
  the same via `format!`. `tooltip_after` already accepts `Cow<'static, str>`,
  so the static cases need no allocation at all and the rest are cacheable.

- [ ] **`Scene` is rebuilt in full every frame, and `scene_dirty` only ever
  gates the *second* rebuild.** `Editor::frame` rebuilds unconditionally, then
  conditionally again before the record. `Scene::rebuild` clears and repopulates
  every pool, clones `view.selected`, and per input port clones a `DataType`, a
  resolved default `StaticValue`, and every `ValueVariant`. Measured cost is
  ~1.95 µs/node, linear — 0.4 ms at 200 nodes, 2 ms at 1000, on every idle
  frame.

  Two things worth knowing before attempting this again. First, the per-port
  *clones* are the small part: `DataType`'s only heap variant is
  `FsPath(Arc<..>)`, so cloning is a refcount bump; interning dominates.
  Second, gating the first rebuild was tried and reverted — `Scene::rebuild`
  reads five inputs (graph, view, library, `run_state`, `run_available`) and
  only the first two move through intents. `RunState` is written by the worker
  from `App::update` with no revision counter, and the library is republished by
  graph-library commands, so **the unconditional rebuild is currently what makes
  worker-driven status visible at all**. Any gate needs a change signal for
  those two plus a way to prove the signal stays complete.

---

## Medium

- [ ] **The dock model pays arena/canonicalization complexity for a tree capped
  at four splits.** `MAX_SPLIT_DEPTH = 4` bounds the layout at 16 panes, yet
  `core/document/dock.rs` represents it as a flat `Vec<DockNode>` with
  serialized `NodeIdx` edges, a canonical-pre-order invariant, a `normalize`
  repack after every structural op, a bit-packed `DockPath` with a sentinel bit,
  and a `validate` re-walking the tree for reachability, acyclicity and index
  topology. Three recursive walks (`group_depth`, `normalize`, `validate`) exist
  to maintain it.

- [ ] **`build_step` opens with five `if let Intent::X = intent` early returns,
  then an `unreachable!()` first match arm.** `core/edit/intent/build.rs` peels
  the document-global intents off one at a time before the scope lookup, then
  the main match re-lists all five just to declare them unreachable. One match
  resolving the scope lazily in the arms that need it removes both.

- [ ] **Two parallel chip/badge widget systems.** `node/header.rs::Badge`
  (`BadgeKind`, `BadgeGlyph`, `control` / `control_drawn` / `marker` / `show`)
  and `widgets/toolbar.rs::Chip` (builder, hover fill, tooltip, click) solve the
  same problem: a square, hover-lifting, tooltipped, optionally toggled glyph
  button. `Badge` is `pub(crate)` and reached from `canvas/pin_preview.rs`
  despite living inside a node module, while `Chip` sits in `widgets/` where a
  shared widget belongs.

- [ ] **Overlapping per-frame context bundles.** `AppContext` (`gui/app/mod.rs`),
  `RecordCtx` (`gui/node/mod.rs`), `PanelDraw` (`canvas/inspector.rs`) and
  `StripCtx` (`gui/dock/strip.rs`). `PanelDraw`'s fields (`theme`, `library`,
  `scene`, `run_state`) are a strict subset of `RecordCtx`'s, which is itself
  `AppContext` plus `selected`, `geometry`, `inspectors`.

- [ ] **`pin_preview::draw_widget` takes 10 arguments behind
  `#[allow(clippy::too_many_arguments)]`.** Six of them (`title`, `border`,
  `border_width`, `image`, `text`, `runnable`) are derived in `PinUi::draw_pin`
  from data — `theme`, `scene`, `run_state`, `selected` — that `RecordCtx`
  already carries end to end. The suppression treats the symptom.
  `node/port_row/glyph.rs` carries the same allow.

- [ ] **The intent/step model spells its variant list six times.** `Intent` (19
  variants), `GraphStep` + `DocStep` (19 more), `apply_graph`/`apply_doc`,
  `revert_graph`/`revert_doc`, then five exhaustive predicates in
  `core/edit/intent/query.rs` — `is_noop`, `requires_relayout`,
  `dirties_document`, `gesture_key`, `coalesce`. `Intent`'s own doc comment
  documents the cost: "adding a variant — touch these 6 spots". Several
  predicates are per-variant constants that could ride on the variant instead of
  being re-derived by match.

- [ ] **`AppCommand` is a two-level enum, a dispatcher, and six `impl App`
  blocks in six files for roughly twenty actions.**
  `gui/app/commands/{mod,file,graph,run,prefs,edit,shell}.rs`. `App`'s inherent
  API is assembled from six files, several handlers being single-line forwarders
  (`ShellCommand::Quit` → `guard_discard`, `RunCommand::Cancel` →
  `runtime.cancel_run()`, `PrefsCommand::Changed` → `apply_preferences`).

- [ ] **`MainWindow::render` clobbers `command` from three of its five sources.**
  `menu_bar::show` assigns first, then `graph_toolbar`, `preferences_view` and
  the image viewer each do a bare `command = Some(c)`, overwriting whatever a
  source above produced. `GraphUI::frame` now respects a pre-set value, so it is
  the odd one out in the *other* direction. The dock closure runs once per
  visible pane, so a split layout really can have two panes produce a command in
  one frame with the later pane silently winning. Making it uniform means
  choosing a cross-pane priority nobody has stated yet.

- [ ] **`classify_canvas_gesture` is computed twice per frame from the same
  response.** Once in `GraphUI::prepass`, once in `GraphUI::frame`. The module
  doc says the classification is "resolved *once* per phase" so precedence lives
  in one place; the two calls can only agree by coincidence of reading the same
  unchanged state.

- [ ] **One map is owned by two layers.** `MainWindow::image_viewers` is pruned
  by `Editor::sync_image_viewers` (reaching through `self.main_window`) and
  populated by the render closure inside `MainWindow::render`. More broadly
  `Editor` owns both the edit pipeline (undo stack, intent buffer) and the whole
  widget tree, so "GUI policy" and "widget ownership" are not actually
  separated.

- [ ] **`PortRef` ⇄ `OutputPort`/`InputPort` conversion is open-coded at ~10
  sites.** `node/mod.rs`, `node/port_row/mod.rs`, `node/prepass.rs`,
  `canvas/connection_ui.rs`, `canvas/pin_ui.rs`, `gui/main_window.rs`,
  `core/document/mod.rs`. No `From` impl exists in either direction, so each
  site restates `{ node_id, kind, port_idx }`. `pin_ui::emit_pin_image_opens` is
  the degenerate case: `OutputPort::new(port.node_id, port.port_idx)` where
  `port` is *already* an `OutputPort`. (`pin_ui` has since grown a local
  `output_port_ref` helper for two of its own sites — worth promoting.)

- [ ] **`StatusLog::error` is a public field cleared by hand at seven sites
  while setting goes through a method.** `core/status.rs` exposes
  `pub(crate) error`, set by `StatusLog::error(line)` but cleared by direct
  assignment in `gui/app/mod.rs`, `commands/file.rs` (twice), `commands/graph.rs`,
  `core/runtime_host.rs` (twice) and `core/terminal_session/mod.rs`. The
  asymmetry is what makes the "same-family clearing" the doc promises
  impossible to enforce.

- [ ] **`RunState` duplicates its status mapping and its prune predicate.** The
  `NodeExecutionStatus → ExecStatus` match appears in both `apply_node_patch`
  and `replace_results`, differing only in the `Running` arm and
  assign-vs-merge. The `nodes.retain(|_, n| status != None || !logs.is_empty()
  || ram.total() > 0)` predicate appears verbatim in `replace_results` and
  `clear_cache_projections`.

- [ ] **`set_output_pinned` uses `usize::MAX` as a "top of stack" sentinel that
  only works because the move clamps.** `core/edit/intent/apply.rs`'s fresh-pin
  path passes `usize::MAX` into `move_item_to_index`, relying on the
  `min(len - 1)` inside `GraphView::move_item_to_index` to mean "leave it where
  insertion put it". The intent ("insert at top") is expressible directly.

- [ ] **`reuse_local_graph` mutates its `&mut Node` argument as a side effect of
  a query-shaped function.** `core/edit/intent/build.rs` rewrites `node.kind` to
  point at an existing local def while returning `Option<(GraphId,
  Box<GraphDef>)>` — name and signature both read as a pure lookup.

- [ ] **`SceneNode` mixes stored and computed predicates over the same inputs.**
  `cache_controls` and `can_evict_cache` are precomputed at rebuild from
  `graph`/`uncacheable`/`outputs`/`impure`, while `runnable()`, `can_disable()`
  and `executable_kind()` are derived at use from
  `run_available`/`boundary`/`missing`/`graph`/`sink`. Twenty fields, no rule
  for which side of the line a predicate belongs on.

- [ ] **`core` still imports an Aperture type.** `core/io/preferences.rs` pulls
  in `aperture::ImageFilter` and serializes it into the persisted preferences
  schema, against `core/mod.rs`'s stated "No Aperture" boundary — and
  `TerminalSession::new` loads that schema on a headless start.

- [ ] **`node_menu.rs` and `graph_menu.rs` are the same controller at two widget
  ids.** Both: loop every scene node → test a filter (`!n.boundary` /
  `matches!(n.graph, Some(GraphLink::Local(_)))`) → test
  `response_for(<id>(n.id)).right.clicked()` → stash a target `NodeId` →
  `AnchoredMenu::open_at(pointer)` → `menu.show` with a body returning a
  `MenuChoice` → route the pick. `AnchoredMenu` already factored out the popup
  chrome; the *scan + target latch + route* half is still duplicated, and each
  declares its own private `MenuChoice`.

- [ ] **`connection_ui::add_boundary_port_intent` matches on `port.kind` three
  separate times** — for `(count, side, prefix)`, then again for `taken`, each
  with two arms mirroring `outputs`/`inputs`. ~40 lines where resolving the side
  once up front leaves one body.

- [ ] **`subscription_ui`'s two halves are mirror images.** `scan_sub_target` /
  `scan_emitter_target` have identical shape (pointer → filter nodes by "not
  self" → map to keys → `first_containing`), differing only in the key domain
  and one extra `n.sink` predicate. The same mirroring runs through
  `InFlight::{FromEmitter, FromSubscriber}`, `snap_sub`/`snap_emitter`, and
  `draw_in_flight`'s two arms — five paired sites for one "the drag has a fixed
  end and a free end" idea.

- [ ] **Three unrelated meanings of "to world" in one module tree.**
  `canvas::to_world(outer_local, viewport)` divides out pan/zoom;
  `BreakerProbe::to_world(rect)` subtracts a canvas origin; `CanvasGeometry`'s
  `layout_center` computes `node_pos + offset`. Three coordinate conversions,
  two sharing a name, one unnamed — nothing states which frame each produces or
  that they agree.

- [ ] **`GraphUI::frame` is a ~220-line function doing five jobs.** Gesture
  arbitration, deselect emission, six controller `apply` calls, command
  arbitration, three hover overrides, then the entire two-level nested record
  closure. The `let Self { .. }` destructure with four `_:` placeholders exists
  purely to get disjoint field borrows into the closure, and has to be edited
  every time a `Gestures` field is added.

- [ ] **`canvas/mod.rs` is orchestration plus a helper grab-bag.** Alongside
  `GraphUI` it holds `CanvasGesture` + `classify_canvas_gesture`,
  `emit_chip_command`, `node_events`, `node_ports`, `to_world`, `pointer_world`,
  `free_end`, `outer_canvas_widget_id`, `inner_canvas_widget_id` — items every
  sibling imports from the parent module. Coordinate conversion, glyph
  iteration, and widget ids are three separable concerns living in the file that
  arbitrates gestures.

- [ ] **`canvas/pan_zoom.rs` holds three unrelated concerns.** (a) generic
  viewport algebra shared with the image viewer — `PanAnchor`,
  `fold_scroll_zoom`, `zoom_about`, `scroll_to_zoom_factor`, all `pub(crate)`
  and all imported by `gui/image_viewer.rs`; (b) the canvas's own pan gesture,
  `emit_pan_zoom`; (c) the toolbar's framing actions — `ViewAction`,
  `view_action_intent`, `reset_target`, `node_bounds`, `fit_target`, imported by
  `gui/graph_toolbar.rs`. The module doc describes only (b). Two external
  modules import from a file named after a gesture neither performs.

- [ ] **`BreakerState` carries four parallel collections and the ceremony that
  goes with them.** `broken`, `broken_nodes`, `broken_subscriptions`,
  `broken_pins` each need a field, a `mark_broken_*` method, a line in
  `begin_frame`, a line in `start`, a drain loop in the release arm, and — for
  three of them — a hand-written `doomed_nodes.contains(..)` filter. The test
  `begin_frame_clears_every_broken_collection` exists because two of the four
  were forgotten once already; the shape guarantees it can recur.

- [ ] **`Inspectors::draw_panels` iterates a `HashMap`.** Iteration order is not
  stable across rebuilds, so with two overlapping panels open the front/back
  order can flip between frames. Panels record after node bodies specifically so
  they win hit-tests, but among themselves the order is unspecified.

- [ ] **`WireEmphasis` and `WirePass` are one thing split in two.**
  `WireEmphasis` is two fields (`fading`, `canvas_bg`) built by a constructor
  named `resolve` one line before the `WirePass` that borrows it, and
  `WirePass::resolve` immediately delegates to `emphasis.stroke`.
  `WireStroke::broken` is a pure pass-through of the argument the caller handed
  in.

- [ ] **`PinUi::draw_pin` re-derives by hand what `Scene::pinned_outputs()`
  already yields.** It walks `scene.nodes.get` → `scene.outputs(n.outputs).get`
  with two `let else` bail-outs, reconstructing most of the `PinnedOutput
  { port, pos, output }` triple `scene.rs` already produces — and which
  `draw_wires`, `scan_widget_drag_start`, `emit_pin_refresh_clicks`,
  `emit_pin_image_opens` and the rubber-band sweep all consume. The two paths
  can disagree about which pins exist.

- [ ] **`emit_pin_refresh_clicks` panics on an invariant it re-derives
  needlessly.** It does `scene.nodes.get(&pin.port.node_id).expect("pinned
  output owner must exist in the scene")` inside a per-frame `find` closure —
  but `pin.port` came from `pinned_outputs()`, which is *built by iterating*
  `scene.nodes.values()`. The lookup cannot fail. It exists only because
  `PinnedOutput` doesn't carry the node (or its `runnable()`).

- [ ] **`connection_ui::port_data_type` clones a `DataType` on every call.**
  `draw` calls it twice per connection per frame and `accepts_wire` twice more
  per snap test, all returning owned values whose consumers immediately call
  `compatible_with` or `port_color`. A borrow would do.

- [ ] **`CanvasGeometry`'s cross-frame caches are never pruned.**
  `PortLayer::offsets` and `node_sizes` only grow; entries for deleted nodes
  persist until the whole `GraphUI` is dropped on document reload. `offsets`
  documents this deliberately; `node_sizes` doesn't mention it at all. A long
  session with heavy node churn accumulates dead entries in both.

---

## Low

- [ ] **The default dark theme makes one of `card_border`'s three tiers inert.**
  `dark::NODE_BORDER` is `Color::TRANSPARENT`, and `colors.node_border` has
  exactly one consumer — the resting arm of `Theme::card_border`. On the default
  preset that arm paints nothing.

- [ ] **`palette_struct!` is a declarative macro with one invocation**
  (`PaletteColors`). Its whole benefit is avoiding one duplicated field list.

- [ ] **`tui::run` and `headless::run` are the same loop** — tick → break on
  `quit` → `select!` on `notify`, differing only in the second `select!` arm
  (stdin line vs Ctrl-C) and the prompt.

- [ ] **`PinUi::apply` can pin an output without seeding its position.** It
  pushes `set_output_pinned(port_ref, true)` unconditionally, but the
  `seed_pin_position_intent` + drag latch sit inside `if let Some(port_center) =
  geometry.ports.center(port_ref)`. On the `None` branch the pin exists at the
  zero default the adjacent comment explicitly says it's avoiding, with no drag
  latched to place it.

- [ ] **`background::build_tile` produces a hard-edged dot** — a binary
  `dx*dx + dy*dy <= r2` test with no coverage falloff, so the tile is aliased
  before linear filtering sees it. At `TILE_PX = 64` with a small radius the
  stair-stepping is the dominant detail in the dot.

- [ ] **Comment volume outruns its own rule in places.** `gui/canvas` carries
  ~1120 doc lines + ~500 comment lines against ~3900 code lines. Much is genuine
  "why" and earns its place (the pre-record commit ordering in `canvas/mod.rs`,
  the drag-capture hover suppression in `geometry.rs`). But `GraphUI::frame`
  carries ~90 comment lines in a ~220-line body, several narrating the next
  statement; `PortInfo`'s field docs largely restate the field names; and
  `pin_ui.rs`'s 29-line module header describes the paint-stack mechanics twice
  more inside `draw_wires`/`draw_pin`'s own docs.
