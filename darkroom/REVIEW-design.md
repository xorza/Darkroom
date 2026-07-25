# Darkroom — simplification / consolidation review

**When you address an item, delete it from this file.** Items are checklist
entries; a fixed finding should leave no residue here.

## Scope and lens

Production code under `darkroom/src` only — tests, fixtures, and the API
tests use were ignored. This pass looks for *simplification, consolidation,
optimization, code reduction, unnecessary complexity, and bad design*, not
correctness. The separate `REVIEW.md` is a correctness/trust-boundary review;
several of its findings have since been fixed. Where an item below overlaps a
still-open one there, it is marked `(cf. REVIEW.md)` — the framing here is the
structural one, not the bug.

Findings are ordered by severity × payoff.

---

## High

- [ ] **The canvas prepass is ~19 independent full-scene sweeps per frame,
  none of them culled.** Every frame, separate loops walk `scene.nodes` or
  `scene.pinned_outputs()` and poll `Ui::response_for` on deterministic ids:
  `emit_graph_opens`, `emit_play_clicks`, `emit_cache_evictions`,
  `emit_path_picks`, `emit_port_dblclicks` (`gui/node/prepass.rs:33-167`),
  `CanvasGeometry::rebuild` (`gui/canvas/geometry.rs:176-215`),
  `scan_drag_start` / `scan_snap_target` / `dropped_on_empty_canvas`
  (`gui/canvas/connection_ui.rs:414-528`), `scan_event_drag_start` /
  `scan_sub_drag_start` / `scan_sub_target` / `scan_emitter_target`
  (`gui/canvas/subscription_ui.rs:278-343`), `scan_port_drag_start` /
  `scan_widget_drag_start` / `emit_pin_refresh_clicks` /
  `emit_pin_image_opens` (`gui/canvas/pin_ui.rs:100-130, 452-474`),
  `NodeMenuUi::apply` (`gui/canvas/node_menu.rs:63-76`), `GraphMenuUi::apply`
  (`gui/canvas/graph_menu.rs:34-42`), `Inspectors::apply` and
  `outside_action` (`gui/canvas/inspector.rs:103-120, 316-332`), and the
  rubber-band sweep (`gui/canvas/selection_ui.rs:116-131`). There are 69
  `response_for` call sites in the GUI tree. `CullRegion` only gates *paint*
  (`gui/canvas/cull.rs`), so an off-screen graph still pays every one of
  these sweeps in full, and several run even when no gesture is in flight.

- [ ] **Three wire-gesture controllers re-implement the same state machine.**
  `ConnectionUI` (801 lines), `SubscriptionUI` (343), and `PinUi` (546) each
  own an `Option<InFlight>`, a "first glyph whose drag started" scan, a
  per-frame snap-target scan, an Esc cancel, a release-edge resolve keyed off
  `dragging()` flipping false, a stale-endpoint drop, a `draw` over the
  committed set with an identical breaker-probe/emphasis/cull preamble, and a
  `draw_in_flight` preview. The drag-anchor half was already factored out
  (`gui/canvas/drag_anchor.rs::GroupDragAnchor`); the wire half was not.
  `WireEmphasis`/`add_cubic_wire` share the paint primitive only.

- [ ] **Script-message policy is implemented twice, once per frontend.**
  `App::handle_script_inbound` (`gui/app/mod.rs:180-216`) and
  `TerminalSession::tick` (`core/terminal_session/mod.rs:36-60`) match the
  same four `ScriptMessage` variants with the same semantics (prefix prints
  with `"script: "`, apply a batch and report refusals, coalesce repeated
  `RunOnce` into one run, quit on `Shutdown`). The commit half is duplicated
  too: `Editor::apply_external_intents`
  (`gui/app/editor/mod.rs:156-163`) and the free `apply_intents`
  (`core/terminal_session/mod.rs:116-125`) both resolve
  `active_target().unwrap_or(Main)` and fold `Refusal::Invalid` reasons into a
  `Vec<String>`. A shared handler returning "what the frontend must do" would
  leave only the run/quit wiring per shell.

- [ ] **Every run action passes through three forwarding layers.**
  `RuntimeHost::run_once` / `run_node` / `evict_cache` / `start_event_loop`
  (`core/runtime_host.rs:161-213`) are four copies of "compile, bail on
  `None`, install, send one message". `Workspace::run_once` / `run_node` /
  `evict_cache` / `start_event_loop` (`core/workspace/mod.rs:71-85`) are
  four one-line forwarders whose only job is supplying
  `&self.open.document.graph`. `App::run_graph` / `start_events` /
  `stop_events` (`gui/app/commands/run.rs:43-73`) are three more one-line
  forwarders. The project's own style rule discourages thin wrapper methods;
  this is the same call restated at three altitudes.

- [ ] **Pinned-output reconciliation is document-wide work, per entry, per
  frame.** `Editor::frame` calls `PinnedOutputStore::reconcile`
  unconditionally (`gui/app/editor/mod.rs:226`). That collects
  `document.viewer_outputs()` into a fresh `HashSet` (a walk of every dock
  group and tab), calls `materialize_full` for each viewer port, then
  `entries.retain(…)` — and the retain predicate calls
  `Document::is_output_pinned` (`core/document/mod.rs:355-365`), which
  recursively walks *every nested graph* in the document, once per stored
  entry (`gui/pinned_output.rs:78-96`). `PinnedOutputStore::ingest` runs the
  same recursive walk per pushed value.

- [ ] **`image_viewer::port_label` does a recursive whole-document node search
  per viewer tab, twice per frame.** `gui/image_viewer.rs:379-391` uses
  `NodeSearch::Recursive` to find the node's name, and it is called from
  `dock::tab_text` for every tab in every strip
  (`gui/dock/mod.rs:324-336`) *and* again from the pane content closure
  (`gui/main_window.rs:123`). Each call also allocates a fresh `String`.

- [ ] **Port tooltip text is rebuilt and then cloned, per port, per frame.**
  `input_label_cell` / `output_cell` build `type_label(...)` (a `String`,
  `gui/node/port_row/mod.rs:514-532`) and wrap it in `port_tip` (another
  `String`, `:503-509`) for every visible port, then `circle_frame` passes it
  to `tooltip_after` as `tip.to_owned()` (`gui/node/port_row/glyph.rs:110`) —
  a second allocation. `event_cell` does the same with a `format!`
  (`:465`). `tooltip_after` already accepts `Cow<'static, str>`, so the
  allocation is avoidable for the static cases and cacheable for the rest.

- [ ] **`Scene` is rebuilt in full every frame, deep-cloning per-port data,
  and `scene_dirty` only ever gates a second rebuild.**
  `Editor::frame` rebuilds unconditionally at `gui/app/editor/mod.rs:240`
  and then conditionally again at `:256-261`. `Scene::rebuild`
  (`gui/scene.rs:274-521`) clears and repopulates every pool, clones
  `view.selected` (`:284`), and per input port clones a `DataType`, a
  resolved default `StaticValue`, and every `ValueVariant` into the pool
  (`:422-438`). The `scene_dirty` flag, the `scene_target` mismatch check,
  and the two-rebuild protocol are a lot of machinery whose only observable
  effect is skipping the *second* pass. (cf. REVIEW.md "idle graph frames
  rebuild semantic projection state".)

- [ ] **`TabRef::ImageViewer` stores a `PortRef` when only an output port is
  representable.** `core/document/mod.rs:88` admits `PortKind::Input` and an
  arbitrary `port_idx`. Three places exist purely to cope:
  `Document::viewer_outputs` `filter_map`s the Output kind away
  (`:367-376`), `Editor::open_image_viewer` asserts the kind
  (`gui/app/editor/mod.rs:465`), and `image_viewer::port_label` carries an
  `"in"` arm that nothing can reach (`gui/image_viewer.rs:386-389`).
  Carrying `OutputPort` instead deletes all three. (cf. REVIEW.md "viewer
  tabs persist port references that are not validated".)

---

## Medium

- [ ] **The dock model pays arena/canonicalization complexity for a tree
  capped at four splits.** `MAX_SPLIT_DEPTH = 4` bounds the layout at 16
  panes (`core/document/dock.rs:43`), yet the representation is a flat
  `Vec<DockNode>` with serialized `NodeIdx` edges, a canonical-pre-order
  invariant, a `normalize` repack after every structural op (`:556-604`),
  a bit-packed `DockPath` address type with a sentinel bit (`:83-114`), and a
  `validate` that re-walks the tree to prove reachability, acyclicity, and
  index topology (`:613-685`). Three separate recursive walks
  (`group_depth`, `normalize`, `validate`) exist to maintain it. (cf.
  REVIEW.md, same finding.)

- [ ] **`build_step` opens with five `if let Intent::X = intent` early
  returns, then an `unreachable!()` first match arm.**
  `core/edit/intent/build.rs:38-144` peels the document-global intents off
  one at a time before the scope lookup at `:145`, and the match at
  `:147-153` has to re-list all five just to declare them unreachable. One
  match that resolves the scope lazily in the arms that need it would remove
  both the prologue and the dead arm.

- [ ] **Two parallel chip/badge widget systems.** `node::header::Badge`
  (`gui/node/header.rs:463-617` — `BadgeKind`, `BadgeGlyph`, `control` /
  `control_drawn` / `marker` / `show`) and `widgets::toolbar::Chip`
  (`gui/widgets/toolbar.rs:75-152` — builder, hover fill, tooltip, click)
  solve the same problem: a square, hover-lifting, tooltipped, optionally
  toggled glyph button. `Badge` is `pub(crate)` and reached from
  `gui/canvas/pin_preview.rs:89` despite living inside a node module, while
  `Chip` sits in `widgets/` where a shared widget belongs.

- [ ] **Five overlapping per-frame context bundles.** `AppContext`
  (`gui/app/mod.rs:27-37`), `RecordCtx` (`gui/node/mod.rs:43-61`),
  `PanelDraw` (`gui/canvas/inspector.rs:64-69`), `StripCtx`
  (`gui/dock/strip.rs:141-148`), and `PinGeometry`
  (`gui/canvas/pin_ui.rs:385-395`). `PanelDraw`'s fields
  (`theme`, `library`, `scene`, `run_state`) are a strict subset of
  `RecordCtx`'s, which is itself `AppContext` plus `selected`, `geometry`,
  `inspectors`. Meanwhile five functions still carry
  `#[allow(clippy::too_many_arguments)]`
  (`gui/canvas/{connection_ui.rs:275, subscription_ui.rs:177, pin_ui.rs:244,
  pin_preview.rs:107}`, `gui/node/port_row/glyph.rs:57`) — the wire draws
  take the same seven parameters each.

- [ ] **The intent/step model spells its variant list six times.** `Intent`
  (19 variants), `GraphStep` + `DocStep` (19 more), `apply_graph`/`apply_doc`,
  `revert_graph`/`revert_doc`, and then five exhaustive predicates —
  `is_noop`, `requires_relayout`, `dirties_document`, `gesture_key`,
  `coalesce` (`core/edit/intent/query.rs`). The doc comment on `Intent`
  (`core/edit/intent/types.rs:61-80`) documents the cost directly: "adding a
  variant — touch these 6 spots". Several predicates are per-variant
  constants (`requires_relayout`, `dirties_document`, `gesture_key`) that
  could ride on the variant rather than being re-derived by match.

- [ ] **`AppCommand` is a two-level enum, a dispatcher, and six `impl App`
  blocks in six files for roughly twenty actions.**
  `gui/app/commands/{mod,file,graph,run,prefs,edit,shell}.rs`. `App`'s
  inherent API is assembled from six separate files, several of whose
  handlers are single-line forwarders (`ShellCommand::Quit` →
  `guard_discard`, `RunCommand::Cancel` → `runtime.cancel_run()`,
  `PrefsCommand::Changed` → `apply_preferences`). The grouping buys module
  separation at the cost of spreading one type's surface across the tree.

- [ ] **The frame's `Option<AppCommand>` slot has two contradictory
  disciplines.** `MainWindow::frame` overwrites it at five sites,
  last-writer-wins (`gui/main_window.rs:96, 109-113, 118-120, 135`), while
  `GraphUI::frame` guards every producer with `if cmd.is_none()`, first-wins
  (`gui/canvas/mod.rs:224-248`). Same slot, same frame, opposite rules. (cf.
  REVIEW.md "the frame-wide `Option<AppCommand>` drops co-occurring
  actions".)

- [ ] **`classify_canvas_gesture` is computed twice per frame from the same
  response.** `gui/canvas/mod.rs:148` (prepass) and `:183` (frame). The
  module doc says the classification is "resolved *once* per phase" so
  precedence lives in one place, but the two calls can only agree by
  coincidence of reading the same unchanged state.

- [ ] **One map is owned by two layers.** `MainWindow::image_viewers` is
  pruned by `Editor::sync_image_viewers` (`gui/app/editor/mod.rs:474-479`,
  reaching through `self.main_window`) and populated by the render closure
  inside `MainWindow::frame` (`gui/main_window.rs:129-131`). Similarly
  `Editor` owns both the edit pipeline (undo stack, intent buffer) and the
  whole widget tree (`main_window`), so "GUI policy" and "widget ownership"
  are not actually separated.

- [ ] **`PortRef` ⇄ `OutputPort` conversion is open-coded at ~8 sites.**
  `gui/node/mod.rs:407`, `gui/node/port_row/mod.rs:428`,
  `gui/canvas/pin_ui.rs:126-129, 194, 260-264, 409-413`,
  `gui/main_window.rs:128`, `core/document/mod.rs:369-373`. No `From`
  impl or method exists in either direction.

- [ ] **`StatusLog::error` is a public field cleared by hand at seven sites
  while setting goes through a method.** `core/status.rs:14-23` exposes
  `pub(crate) error`, set by `StatusLog::error(line)` but cleared by direct
  assignment in `gui/app/mod.rs:161`, `gui/app/commands/file.rs:75, 99`,
  `gui/app/commands/graph.rs:56`, `core/runtime_host.rs:109, 138`, and
  `core/terminal_session/mod.rs:89`. The asymmetry is what makes the
  "same-family clearing" the doc promises impossible to enforce. (cf.
  REVIEW.md "one untyped sticky-error slot".)

- [ ] **`RunState` duplicates its status mapping and its prune predicate.**
  The `NodeExecutionStatus → ExecStatus` match appears in both
  `apply_node_patch` (`gui/run_state.rs:173-184`) and `replace_results`
  (`:207-220`), differing only in the `Running` arm and assign-vs-merge. The
  `nodes.retain(|_, n| status != None || !logs.is_empty() || ram.total() > 0)`
  predicate appears verbatim at `:237-238` and `:249-251`.

- [ ] **`unimplemented!()` sits in a live command handler.**
  `App::run_node` panics when the active target is not `Main`
  (`gui/app/commands/run.rs:50-52`). Reachability is prevented only by
  `SceneNode::run_available` gating the play chip and the menu item; a
  command that arrives from any other path (or a stale response) crashes the
  editor. A refusal or a debug assert would carry the same intent without
  the crash.

- [ ] **`set_output_pinned` uses `usize::MAX` as a "top of stack" sentinel
  that only works because the move clamps.**
  `core/edit/intent/apply.rs:283-290`: the fresh-pin path passes
  `usize::MAX` into `move_item_to_index`, relying on the `min(len - 1)` in
  `GraphView::move_item_to_index` (`core/document/mod.rs:223-230`) to mean
  "leave it where insertion put it". The intent ("insert at top") is
  expressible directly.

- [ ] **`reuse_local_graph` mutates its `&mut Node` argument as a side effect
  of a query-shaped function.** `core/edit/intent/build.rs:405-426` rewrites
  `node.kind` to point at an existing local def while returning
  `Option<(GraphId, Box<GraphDef>)>` — the name and signature both read as a
  pure lookup.

- [ ] **`SceneNode` mixes stored and computed predicates over the same
  inputs.** `gui/scene.rs:141-226`: `cache_controls` and `can_evict_cache`
  are precomputed at rebuild from `graph`/`uncacheable`/`outputs`/`impure`,
  while `runnable()`, `can_disable()`, and `executable_kind()` are derived at
  use from `run_available`/`boundary`/`missing`/`graph`/`sink`. Twenty
  fields, no rule for which side of the line a predicate belongs on.

- [ ] **`core` still imports an Aperture type.** `core/io/preferences.rs:3`
  pulls in `aperture::ImageFilter` and serializes it into the persisted
  preferences schema, against `core/mod.rs:1-5`'s stated "No Aperture"
  boundary — and `TerminalSession::new` loads that schema on a headless
  start. (cf. REVIEW.md, same finding.)

---

## Low

- [ ] **`Theme::port_radius` is `pub(crate)` but used once, inside its own
  impl.** `gui/theme.rs:717-720`, called only from `port_overhang`
  (`:740`). Narrowest-visibility rule says private.

- [ ] **Two different `dot` helpers with the same name.**
  `gui/widgets/support.rs:107` emits a `Shape`; `gui/node/memory_row.rs:76`
  builds a `Panel` that occupies layout. Same name, different contract.

- [ ] **The default dark theme makes one of `card_border`'s three tiers
  inert.** `dark::NODE_BORDER` is `Color::TRANSPARENT`
  (`gui/theme.rs:64`), and `colors.node_border` has exactly one consumer —
  the resting arm of `Theme::card_border` (`:771`). On the default preset
  that arm paints nothing.

- [ ] **`palette_struct!` is a declarative macro with one invocation.**
  `gui/theme.rs:275-291`, used only for `PaletteColors` (`:609-683`). Its
  whole benefit is avoiding one duplicated field list.

- [ ] **`GraphView`'s manual `PartialEq` hand-rolls `Iterator::eq`.**
  `core/document/mod.rs:190-201` does `len ==` plus `zip(..).all(..)`; the
  order-sensitivity it exists for is exactly `iter().eq(other.iter())`.

- [ ] **`materialize_full` removes and re-inserts a map entry to get an owned
  value.** `gui/pinned_output.rs:87-96` — two hash lookups plus a
  potential rehash for what a `get_mut` + `mem::replace` does in place.

- [ ] **`RuntimeHost`'s two drains have asymmetric shapes.**
  `drain_worker` returns an iterator, `drain_script` returns a `Vec`
  (`core/runtime_host.rs:221-232`), so both call sites collect anyway
  (`gui/app/mod.rs:133`, `core/terminal_session/mod.rs:79`).

- [ ] **`prepare_content` probes the same downcast twice.**
  `gui/pinned_output.rs:116` checks `value.as_custom::<LensImage>().is_none()`
  and then `prepare_image` immediately re-does the same downcast (`:138-140`).

- [ ] **`build_duplicate_intent_for` asks for all bindings touching a node,
  then discards the half it didn't want.**
  `core/edit/intent/duplicate.rs:100-105` iterates
  `graph.bindings_touching(old_id)` and `continue`s on
  `port.node_id != old_id` — i.e. it wants only the node's *own* inputs.

- [ ] **`tui::run` and `headless::run` are the same loop.**
  `tui/mod.rs:18-42` and `headless/mod.rs:16-33` share tick → break on
  `quit` → `select!` on `notify`, differing only in the second `select!` arm
  (stdin line vs Ctrl-C) and the prompt.

- [ ] **`REVIEW.md` (a review artifact) is checked into the crate root**
  alongside `Cargo.toml` and `build.rs`, and is not excluded by the
  package's `exclude` list.
</content>
</invoke>
