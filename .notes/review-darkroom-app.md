# Review: `darkroom/` and `app/`

Scope: the `darkroom` crate (core, gui, driver, platform) and the `darkroom-app` binary.
The 277 tests of the two crates pass, each under 0.42 s and the suite in 0.62 s under
nextest, so no test is slow.

## Proposal

### Root causes

1. **The canvas's phase split is stated, not kept.** `graph_ui/mod.rs` says `frame` is what a
   pass reads, `gesture` turns input into intents and `draw` paints. But curve and tie geometry
   live in `draw`, gesture state (`GlyphDrag`, `ContextMenu`, `AnchoredMenu`) lives in `draw`,
   committed-wire paint lives in `gesture`, polled ids live in `node_ui`, and theme lookups live
   in `node_ui`. This gives six cross-module cycles (draw ↔ gesture, node_ui ↔ draw,
   frame ↔ node_ui, frame ↔ draw, node_ui ↔ gesture, ctx ↔ gesture), a cubic sampling loop
   written twice, and a gesture reaching into another gesture for a curve constant.
2. **No gesture owns a shared lifecycle.** Each controller writes its own latch, cancel and
   release. The rule "`in_flight` means Esc cancels" is kept by hand, and it breaks in the
   camera pan and the preview spawn. One-frame facts (`ended_on_secondary`, `pending_open`,
   `resume_floating`) escape `CanvasInput`, and the right-click suppression breaks across
   frames. One pointer drag has three `GroupDrag` owners.
3. **Core services report to the status bar themselves.** `RuntimeHost`, `open_at_launch` and
   `LoadReport::report` take `&mut StatusLog` (and `&mut Preferences`) where they could return
   an outcome. This threads UI types down into core, writes the "load failed" text three
   times, and leaves dead returns (`run_node -> bool`).
4. **The frame pipeline is written four times.** `App::update`/`record`,
   `SessionHarness::frame`, the bench's `Editor::record` and `CanvasHarness::frame` each compose
   the phases by hand, and they drift. The canvas harness skips the drain between prepass and
   record, which hides a one-frame wire lag. The allocation gate audits a copy, not `App`.
5. **The window driver has no lifecycle for its requests.** A request outlives the call that
   timed out, a panic in it ends the person's window, a disconnect reads as "quit", and a
   hidden window reads as a 30 s timeout. The driver also leaks into the gui layer (gui
   imports `DialogAnswer` and builds `DriverError`), and the two backends open documents by
   two paths.
6. **The step model forces "anchor plus `Change<T>`" on every step.** Where the real state is
   an enum (insertion or removal, which property) or a self-inverse diff (selection toggles),
   the type admits states the docs then forbid, and a selection click stores two whole sets.
7. **Values are checked late or outside a boundary type.** Key chords parse during play, the
   CLI checks size and scale after clap, nil tie ids pass the gate, a validated document falls
   back to `"?"`, and a release `assert_eq!` checks an internal contract.
8. **Renames left their docs behind.** Many module docs name types, paths and counts that no
   longer exist.

### Target design

- **Canvas layering.** `frame/` holds every geometry a pass reads: `CanvasGeometry` with
  `PortLayer` in its own file, `Wire` (curve, hull, `chords()`, `BEZIER_SAMPLES`), `TieLayout`
  and `BandAxes`. `gesture/` holds every input controller, including `GlyphDrag`,
  `AnchoredMenu`, the item menu (renamed from `ContextMenu`) and `PreviewSpawn`, and
  `CanvasGesture` sits beside `CanvasInput` in `prepass_ctx.rs` with a pure `classify`.
  `draw/` paints: committed wires and subscriptions in `draw/wire`, tie bands in `draw/tie`.
  One leaf `graph_ui/wid.rs` holds every polled widget id. `Theme` answers `port_color` and
  `exec_color`. Edges then run `draw → {frame, gesture probes}`, `gesture → frame`, and none
  back.
- **Gesture shape.** `CanvasInput` is the one answer to "what this frame's input means",
  including the palette handoff and the secondary-button suppression. One `GroupDrag` on
  `GraphUi`. A `ButtonStroke` helper for the breaker and the tie line. Plain `Option` in place
  of `GestureSlot`. One verb per phase and every controller writing to `out`.
- **Core returns outcomes.** `RuntimeHost` returns results (`Result<(), NoCompiledWork>`) and
  never sees `StatusLog` or `Preferences`. `PreferencesStore` owns `open` and `save`.
  `open_at_launch` returns what happened. `LoadReport` is data with `Display`.
- **One frame step.** A `Session` frame step that `App`, `SessionHarness`, the bench and the
  canvas harness all call, with the allocation gate run through `App`.
- **Driver.** `Driver::act` plays a whole batch and names the failing action. Window requests
  carry a cancel token, run under `catch_unwind`, and report `WindowClosed`. The driver
  encodes shots on its own thread. Both backends open through the product path. `DialogAnswer`
  and the open error belong to gui, and the driver converts them. One tracing setup, with the
  ring filtered per layer, for both modes. The session directory is a `common` temp dir that
  the main thread drops.
- **Steps.** `SetNode { entry, edit: Insertion | Removal }`, `PropertyChange`,
  `SetSelection { toggled }`. `MalformedIntent: PartialEq`, so every refusal test compares
  exact errors and whole documents.

### Decisions

Each goes into `darkroom/AGENTS.md` (or palantir's) with the batch that lands it.

- **The camera is outside the undo history.** The viewport stays saved and does not dirty the
  document, but Ctrl+Z never moves it. Refused: one undo entry per gesture, and one merged
  entry for a zoom during a drag, which both keep view moves between the person and their last
  edit. Photoshop keeps zoom and scroll out of History because they change no pixels
  ([Adobe](https://helpx.adobe.com/photoshop-elements/desktop/workspace-and-environment/undo-redo-cancel-actions.html)),
  and Blender records a view move only when it moves the camera object
  ([T92099](https://lists.blender.org/pipermail/bf-blender-cvs/2022-August/176504.html)).
- **Esc on a preview spawn removes the card.** The spawn and its drag are one gesture, so a
  cancel nets to nothing. Refused: Blender's Shift+D rule, where Esc cancels only the move and
  the copy stays ([manual](https://docs.blender.org/manual/en/latest/modeling/meshes/editing/mesh/duplicate.html)),
  which here leaves a card on its producer's port.
- **A duplicate with incoming wires keeps both families.** A cloned subscriber subscribes to
  its outside emitter as a cloned input keeps its outside producer. Blender's
  `duplicate_move_keep_inputs` keeps the copy's input links
  ([commit](https://developer.blender.org/rB6beeba1ef5a)). Refused: data wires only, which the
  menu's label does not say.
- **Bytes are KiB, MiB and GiB.** 1024-based, as RAM is sized, with the IEC 80000-13 names.
  Refused: SI kB at 1000, and KB at 1024, which the standard reads as 1000
  ([Ubuntu units policy](https://wiki.ubuntu.com/UnitsPolicy)).
- **A run time under 1 µs prints in ns.** Precision over a `<1µs` floor; `999ns` fits the
  seven-character budget.
- **Windows opens a URL with `ShellExecuteW`.** Through `common`'s existing `windows`
  dependency, with its `Win32_UI_Shell` feature. Refused: `cmd /C start`, which parses `&`,
  `^` and `|` in the URL, and which the `open` crate keeps only behind its `insecure` feature
  ([docs](https://docs.rs/open/5.4.0)).
- **Palantir gains three public APIs**, with the go-ahead given: a screenshot ticket, as egui's
  `ViewportCommand::Screenshot(UserData)` comes back in `Event::Screenshot`; a hidden flag on
  `HostHandle`, from the `WindowEvent::Occluded` palantir already reads; and
  `StatefulLook::each_mut`.

### Weighed and dropped

- One `Vec<Move>` per `MoveSelection` frame: one allocation and one free per drag frame, not
  new, and removing it exposes the open undo step. The cost is more than the defect.
- `UndoHistory`'s lazy compaction: one memmove of at most 1 MiB per 1 MiB of packed history,
  never on a drag frame.
- The three canvas ctx types: they enforce the prepass and record order at compile time.

## Plan

Each batch leaves `cargo drive check` and `cargo drive test` green. A comment that breaks the
comment rules is trimmed in every file a batch touches. Whoever addresses an item deletes it.

### 1. Saved paths and the document file

Files: `core/io/document/{saved_paths.rs, document_format.rs, error.rs, tests.rs}`.

- [ ] `saved_paths.rs:54-60, 83-101` — a loaded `../x.tiff` is held as `/p/../x.tiff`, and
  `relative_to` strips the folder and writes `../x.tiff`. AGENTS.md "Paths in a document"
  says a save never writes `..`. Target: `relative_to` returns `None` when the rest holds a
  `ParentDir`, and `resolve` folds `..` lexically, with the symlink caveat stated. Add
  `../x.tiff` and `a/../../y` rows to `paths_inside_the_folder_are_saved_relative_and_move_with_it`.
- [ ] `document_format.rs:505-511` — `unzip` reserves `entry.size()`, up to 256 MiB, on the
  archive's word, so a 100-byte file commits 256 MiB. Target: no `with_capacity` from the
  header (`read_to_end` grows it), or a cap from the archive's own length.
- [ ] `document_format.rs:436-451`, `error.rs:237-242` — the save converts `ZipError` into
  `io::Error`, while the load keeps it. Target: `DocumentSaveError::Archive { path, source:
  ZipError }`.
- [ ] `core/io/mod.rs:1-3` — says documents are "zipped archives"; they are RON text or zipped.

### 2. Driver batches and the MCP boundary

Files: `driver/{mod.rs, action.rs, state.rs, shot.rs, tests.rs}`, `app/src/{cli.rs,
mcp/darkroom_server.rs, mcp/params.rs, mcp/mod.rs, mcp/tests.rs}`.

- [ ] `darkroom_server.rs:117-141`, `driver/mod.rs:197-199` — `darkroom_act` calls
  `Driver::act` once per action, and each call resets `runs_before`. A run that ends while
  `ctrl+r` settles is counted before the next action, so `wait_run` times out after 60 s. Each
  call also builds a whole `State` and throws it away. Target: `Driver::act` plays the batch,
  sets `runs_before` once and returns `Err(ActFailure { idx, error })`. The server calls it
  once. Test: `[ctrl+r, click]` then `wait_run` on the sample project.
- [ ] `darkroom_server.rs:142-146` — a failed trailing shot returns only the shot's error, and
  the report of the actions that ran is lost. Target: keep the act report and add the shot's
  error as text that says the actions ran.
- [ ] `action.rs:41, 121, 130`, `driver/mod.rs:344-348` — `Action::Key(String)` parses in
  `play`, so a bad chord at action 3 is found after actions 1 and 2 ran. `shift+a` types "a".
  Target: `Action::Key(Chord)`, converted in `TryFrom<ActionParam>` so the batch is checked
  whole, and Shift upper-cases the text.
- [ ] `driver/mod.rs:286, 367, 392-398` — `clicked` resets on any other action, so
  `[click, key escape, click]` on one spot lands inside palantir's 500 ms and 5 px window and
  reads as a double click. Target: remember the last press time and position from every
  pressing action, Drag included, and space a press only inside that window. Test that case.
- [ ] `cli.rs:73-84`, `mcp/mod.rs:113-130` — `0x0`, too large sizes, and a NaN, zero or
  negative scale pass clap and are refused later as a `String`, exit 1. Target: `WindowSize`
  checks `1..=MAX_WINDOW_SIDE`, a `value_parser` checks the scale, `check_config` goes, and
  the cases join `the_flags_pick_the_launch`.
- [ ] `state.rs:119-120, 474-479` — a concise state ignores `nodes`, the wires ignore the
  filter, and the state has no limit. Target: `nodes` filters both forms and the wires, and a
  node limit with `omitted` as `UiList` has.
- [ ] `shot.rs:89-107` — `slug` keeps the whole name, so a long node name gives a file name
  past `NAME_MAX`. Target: cap the slug at 64 characters.
- [ ] `driver/tests.rs:139-143, 210, 262, 300, 331-335` — `starts_with`, `contains`,
  `is_some` and `>` where the value is known. Target: the exact wire, `[BRIGHTNESS, CONVERT]`,
  the rename field's text, fixture size + `COPY_LIMIT` + 1, the exact log line.

### 3. Window requests

Files: `driver/{window_backend.rs, backend.rs, error.rs}`, `gui/app/driven_requests.rs`,
`gui/app/mod.rs`. After batch 2.

- [ ] `window_backend.rs:37-56, 85-134, 183-202`, `driven_requests.rs` — a request stays queued
  after `wait` gives up. A timed-out open opens later, a stale answer takes the next dialog, and
  a stale shot that never lands keeps `pending()` true, so every later settle fails with
  `UnsettledInTime`. Target: each request carries a cancel token that the driver sets when it
  stops waiting, and `serve` drops a cancelled request unrun.
- [ ] `driven_requests.rs:88-94` — a panic in `State::of`, a locator or `Shot::of` unwinds out
  of the winit loop and ends the person's window, against the decision "With a window, the
  window stays". Target: `serve` runs each request under `catch_unwind` and replies
  `Err(DriverError::Panicked(message))`.
- [ ] `error.rs:27-31`, `window_backend.rs:54, 71, 79, 128, 178` — every window disconnect
  reports `Quit`, "as an action asked", also when the person closed the window. Target:
  `DriverError::WindowClosed`.
- [ ] `driven_requests.rs:28-104` — `lend`, `restore`, `again` and `LentRequests` hand-roll
  `Vec::retain_mut`. Target: `retain_mut` over the taken queue, with requests pushed meanwhile
  appended.
- [ ] `backend.rs:27-35` — `read` also writes (the window's open, a dialog answer). Target:
  name it `with_app`.
- [ ] `window_backend.rs` — only `settled` is tested, and `wait` reads a hidden clock. Target:
  the request builders become free fns returning an `UpdateRequest`, tested against `App` on a
  `UiHarness` as `driven_requests.rs` does: asked, `Again`, landed, `Done`, a region error, a
  dropped request, a cancelled one. `wait` takes its deadline and `native_open` as inputs.

### 4. Preview conversion and app-state bugs

Files: `gui/state/preview_store/{mod.rs, preview_converter.rs, tests.rs}`,
`gui/state/process_memory.rs`, `gui/app/mod.rs`, `gui/dialogs.rs`, `platform/*`,
`common/src/process.rs`.

- [ ] `preview_store/mod.rs:220-235`, `preview_converter.rs:116-134` — every `publish` sends
  its value down an unbounded channel. An event loop that publishes a 24 MP f32 value (288 MB)
  each frame faster than the converter drains piles them up without bound. Target: one job in
  flight and one pending value per node, which a new publish replaces, so each landed result
  still shows and memory holds at most two values per node.
- [ ] `gui/app/mod.rs:755`, `process_memory.rs:54-63` — the memory reading takes
  `Instant::now()` in each record pass, so the two passes of one frame can render different
  text, and offscreen time does not step it. Target: `sample(now: Duration)` from `ui.now()`.
- [ ] `dialogs.rs:105-111, 131-134, 208-248` — `new_file` adds no missing extension and asks
  no overwrite of the extended name, which `save_path` does. Target: one extension filter goes
  through the same `extended` and `unconfirmed_target` loop.
- [ ] `platform/windows/mod.rs:16-20` — `cmd /C start "" url` splits a URL on `&`, `^` or `|`.
  `platform/{linux,macos}/mod.rs` and `common/src/process.rs:8-13` drop the `Child`, so each
  click leaves a zombie. Target (decision "Windows opens a URL with `ShellExecuteW`"):
  `ShellExecuteW` through `common`, and on Unix a `common::process` spawn that reaps its child.

### 5. Canvas frame order

Files: `gui/pane/graph_ui/{frame/canvas_geometry/mod.rs, internals.rs, mod.rs,
draw/wire/mod.rs, node_ui/mod.rs}`.

- [ ] `internals.rs:4-8, 110-134, 246-258` — `CanvasHarness::frame` claims to run the window's
  sequence but never drains the document intents between prepass and record. Target: drain and
  record them, as `session/mod.rs:136` does. Do this first: it exposes the next item.
- [ ] `frame/canvas_geometry/mod.rs:134-136, 161-168`, `mod.rs:329-333` — `rebuild` caches each
  port's center as `node.pos + offset` from the document before the drain, and the record
  draws the moved nodes. Wires and tie spans then lag a dragged node by one frame, and a move
  asks no relayout. The type's own doc refuses a cached position for node rects. Target:
  `PortLayer` caches the offset and `center` adds the current position. Test: a drag frame's
  wire ends at the moved port.
- [ ] `draw/wire/mod.rs:242-267`, `node_ui/mod.rs:143-150` — the breaker and tie probes skip
  culled wires and nodes, so a scribble that zooms or pans mid-stroke stops cutting what only
  its off-screen part crosses. Target: while a probe is active, probe culled pieces and nodes,
  and cull only the paint.
- [ ] `internals.rs:236` — `node_world_rect(&mut self)` needs only `&self`.

### 6. Gesture bugs

Files: `gui/pane/graph_ui/gesture/*`, `graph_ui/{mod.rs, prepass_ctx.rs, draw/context_menu.rs}`.
After batch 5.

- [ ] `wire_ui/mod.rs:108, 211-227`, `mod.rs:433-441` — a right press ends a floating wire and
  sets `ended_on_secondary`, but `apply` clears it each frame, and palantir raises the click on
  release. A press and release in two frames ends the wire and opens the palette. Target:
  `CanvasInput` carries the suppression from the press until the button's release, set once in
  prepass. Test press and release in separate frames.
- [ ] `pan_zoom/camera_gesture/mod.rs:79-106, 308-310` — a latched pan counts as in flight, so
  Esc resolves to Cancels, but `emit` never reads `cancelled()`. Esc does nothing. Target: on
  cancel, restore the pan the latch began at and clear the latch. Test it beside
  `a_pan_drag_measures_from_its_latch_and_releases_once`.
- [ ] `preview_drag/mod.rs:79-101`, `group_drag/mod.rs:122-126` — the spawn's `AddNode` and
  `SetInput` go out before the drag's gesture opens, so Esc returns the card to the port it
  left and keeps it. Target (decision "Esc on a preview spawn removes the card"): the spawn
  joins the drag's gesture, and a cancel removes the card and leaves no undo entry. Add an Esc
  test.
- [ ] `node_menu_ui/mod.rs:76-111`, `tie_ui/tie_menu_ui.rs:79-135`, `draw/context_menu.rs:52-56`
  — a menu stays open after Delete or Ctrl+Z removes its target. Its Remove then removes the
  selection, and the tie menu's rows do nothing. Target: the menu takes a `holds(target)` check
  and closes when the target is gone.
- [ ] `breaker_ui/scribble/mod.rs:12, 17`, `tie_ui/tie_line.rs:168` — the length and spacing
  bounds are in world units, so at zoom 0.1 a breaker stops after 200 screen px. Target: the
  bounds in screen px, divided by `viewport.zoom` at the call.
- [ ] `tie_ui/tie_menu_ui.rs:141-171` — `release_label` falls back to `"?"` on a document the
  load validated. Target: `expect` each lookup.
- [ ] `gesture/canvas_gesture.rs:65-97`, `tie_ui/tests.rs:27-61` — `classify` takes `&mut Ui`,
  so its precedence is tested through five harnesses and four arms are untested. Target:
  `classify(&ResponseState, Modifiers)`, moved into `prepass_ctx.rs` beside `Escape`, with a
  table test over every arm and the double-click precedence.
- [ ] Gesture test gaps: the breaker's supersede filter (`breaker_ui/mod.rs:99-125`),
  `TieLine::crosses` and `first_crossing` (`tie_line.rs:215-262`), `shortcuts::emit`
  (`shortcuts.rs:34-59`), the drop → palette → pick → re-float flow, the exact (30, -20) drag
  offset in `a_tie_clicks_drags_and_sweeps_as_a_node_does`, and the `Debug` string compare in
  `the_tie_menu_releases_pins_and_removes`. Split tests that pack independent harnesses.

### 7. Node card values

Files: `gui/pane/graph_ui/node_ui/const_editor/{mod.rs, tests.rs}`, `draw/tie/band.rs`.

- [ ] `const_editor/mod.rs:240-244, 355-385` — a focus and blur with no typing commits, and
  `parse_any(format_any(v))` is not `v` (`Null` → `String("")`, `String("42")` → `Int(42)`).
  Tabbing through an `Any` field writes an edit and changes its kind. Target: `any_smart`
  returns `None` when the text equals the value's canonical text.
- [ ] `const_editor/mod.rs:466-468` — `v.abs()` overflows on `i64::MIN`. Target:
  `v.unsigned_abs()`, and an `i64::MIN` row in `int_speed_floors_then_scales`.
- [ ] `draw/tie/band.rs:71-72` — `strand_count() - 1` underflows for the empty band `Band`
  allows. Target: `debug_assert!(band.strand_count() > 0)`.

### 8. Viewer and number formats

Files: `gui/pane/image_viewer/*`, `gui/widgets/{zoom_range, format}/*`,
`gui/pane/preferences.rs`.

- [ ] `image_viewer/camera/mod.rs:34`, `widgets/zoom_range/mod.rs:69-77` — the fit zoom is not
  clamped. A 16 px image fits at 37.5, and a wheel step in clamps to 32, so the image shrinks.
  Target: `zoom_about` clamps to `[min.min(zoom), max.max(zoom)]`, so a step never moves
  against its direction. Add a table row that starts outside the range.
- [ ] `image_viewer/mod.rs:475` — `{:.0}%` shows a fit under 0.5 % as "0%". Target: more digits
  under 10 %.
- [ ] `widgets/format/elapsed.rs:37-38` — a 100 ns run shows "0µs". Target (decision "A run
  time under 1 µs prints in ns"): an `ns` tier, `0ns` only for exactly 0, with boundary rows in
  the table.
- [ ] `widgets/format/bytes.rs:21, 31-43` — 1024-based figures are labelled KB, MB and GB.
  Target (decision "Bytes are KiB, MiB and GiB"): KiB, MiB and GiB.
- [ ] `preferences.rs:150-155, 211, 295` — `path_problem` checks the lossy text, so a non-UTF-8
  path reads as "File not found". Target: check the stored `Path`, and the text only on commit.
- [ ] Image viewer gestures have no test: the fit taking effect on the first gesture, the
  double-click reset, wheel zoom about the cursor. Target: one `UiHarness` test with exact
  `Viewport` values.

### 9. Edit layer

Files: `core/edit/**`, callers in `core/open_document/mod.rs`,
`gui/pane/graph_ui/{gesture/node_menu_ui, gesture/shortcuts.rs, node_ui/header.rs}`.

- [ ] `undo_history/mod.rs:33-39, 95-113`, `open_document/mod.rs:193-219`,
  `step/set_viewport.rs` — the history holds one open gesture. A wheel zoom during a node drag
  alternates the two gestures, and each frame seals the other, so one drag becomes an entry per
  frame. Target (decision "The camera is outside the undo history"): `SetViewport` writes the
  document without an undo step, so `UndoStep::Viewport` and its absorb go, and one pointer
  can hold only one recorded gesture. Test: a zoom during a drag leaves one entry.
- [ ] `graph_intent/mod.rs:296-340`, `incoming_wires.rs:3-12` — "Duplicate with incoming
  wires" keeps data wires from outside the set but drops subscriptions from outside emitters.
  Target (decision "A duplicate with incoming wires keeps both families"): `Keep` also
  subscribes each cloned subscriber to its outside emitter. Test it beside
  `duplicate_keeps_external_producers_on_request`.
- [ ] `step/set_selection.rs:15-26` — a selection step stores both whole sets: a Shift-click in
  a 500-item selection packs about 17 KB, and 60 clicks evict every real edit from the 1 MiB
  history. Target: `SetSelection { toggled }`, the symmetric difference, applied as
  `selected ^= toggled` both ways.
- [ ] `step/set_node.rs:102-169` — `Change<Option<NodeEntry>>` admits two absences, stores the
  node id twice, and checks a removal with a release `assert_eq!` on every removal. Target:
  `SetNode { entry, edit: Insertion | Removal, ties }`, with `debug_assert_eq!`.
- [ ] `step/set_node_property.rs:10-45` — `Change<NodeProperty>` admits two different
  properties, and `NodeProperty` lives in a step module that gui imports. Target:
  `PropertyChange { Disabled(Change<bool>), RuntimeCache(Change<CacheMode>) }`, and
  `NodeProperty` in `graph_intent/node_property.rs`.
- [ ] `graph_intent/mod.rs:761-786` — removal scans every binding for the inputs it fed, which
  the captured `NodeEntry` already holds. Target: `NodeEntry::fed_inputs()`, read after the
  step applies.
- [ ] `graph_intent/mod.rs:587-592, 664-726`, `validate.rs:29-37` — `Raise` and every tie
  intent read a nil id as stale, against the gate's rule. Target: `non_nil_node` in `Raise`, a
  `NilTieId { role }` check first in every tie arm.
- [ ] `graph_intent/mod.rs:278, 366` — `duplicate` mints its ids, so tests find clones by
  position. Target: an id source parameter; callers pass `unique`, tests a counter.
- [ ] `graph_intent/mod.rs:145-151, 445` — `SetSubscription { subscribe }` where every other
  set intent says `to`; `into_step` is `pub(crate)` with one caller in its file. Target: `to`,
  and a private `into_step`.
- [ ] `error.rs:30` — `MalformedIntent` lacks `PartialEq` on the false premise that its foreign
  errors have none. Target: derive `Clone, Copy, PartialEq, Eq`.
- [ ] `graph_intent/tests.rs:311, 933, 993, 1247, 1316, 1753` — six refusal tests with four
  `refused` closures and three assertion styles. Target: one table of (what, fixture, intent,
  expected error), asserting the exact error and `document == before`. `refusal_leak` goes.
- [ ] `graph_intent/tests.rs:126, 731, 806, 860` — round trips spot-check fields. Target: fold
  them into the `Edit` table, one row per intent kind, `RenameNode` included.
- [ ] `step/tests.rs:89-276` — three predicate sweeps repeat their lists and leave out `Ties`.
  Target: one table built through an exhaustive match.
- [ ] Untested: a tie event wire in `cloned_wire`, a click on a tie, the exact
  `InvalidViewport`, and in `UndoHistory` the full rewind then a new edit, one entry over the
  budget, a gesture frame that drops the redo tail, and `can_undo` with only a no-op open
  gesture.
- [ ] `edit/mod.rs:3-14` — "Three modules" leaves out five. Target: a one-line `//!`.

### 10. Core services return outcomes

Files: `core/runtime_host/*`, `core/runtime_library.rs`, `core/io/cache/*`,
`core/io/preferences/*`, `core/open_document/*`, `core/document/load_report.rs`,
`gui/app/mod.rs`, `lib.rs`.

- [ ] `runtime_host/mod.rs:159-182` — `Repointed` clears `root_prepared` but only `Gained`
  prepares the root, so a run in flight writes blobs into a new root with no `.gitignore`.
  `runtime_host/mod.rs:26-35`, `io/cache/mod.rs:33-41` — the root is three hand-synced fields,
  and `cache::prepare` drops its errors while the host marks the root prepared. Target:
  `disk_root: Option<DiskRoot { path, prepared }>`, prepared on every change, and `prepare`
  returns its error for a `warn!`.
- [ ] `runtime_host/mod.rs:114-120, 186-271`, `gui/app/mod.rs:608-653` — every run method takes
  `&mut StatusLog`, `run_node -> bool` is dropped by its one caller, and `evict_cache` and
  `flush_cache` stop an event loop for a node with no compiled work. The six `App` run methods
  forward one line each. Target: `run_node`, `evict_cache` and `flush_cache` return
  `Result<(), NoCompiledWork>`, `App`'s `run_command` match calls the host and reports, and
  the host loses its `status_log` dependency.
- [ ] `runtime_host/mod.rs:80-100`, `runtime_library.rs:31-37` — the host takes all of
  `Preferences` for `ml_models`, under three names. Target: `&MlModelPaths` and one name,
  `set_ml_model_paths`.
- [ ] `io/preferences/preferences_store.rs:19-31`, `gui/app/mod.rs:141-184, 574-584`,
  `lib.rs:65-73` — the store loads but `App` saves by matching `File(path)`, and
  `LaunchInputs.preferences` is a `Result` parameter. Target: `PreferencesStore::open()` returns
  the preferences, the store (`Memory` on failure) and the failure. `save(&Preferences)` is a
  no-op for `Memory`. `save_to` is private, and `path()` folds into `user()`.
- [ ] `open_document/mod.rs:115-121, 309-362`, `gui/app/mod.rs:213-216, 501-511` —
  `open_at_launch` clears `preferences.document_path` and `App` diffs a clone to notice, and
  the load-or-report block is written three times. Target: `open_at_launch` returns
  `Launched { open, forgot_remembered }`, and one helper loads and reports.
- [ ] `document/load_report.rs:12, 112-126` — a document type writes the status line. Target:
  `LoadReport` keeps data and `Display`, and the helper above reports it.
- [ ] `open_document/mod.rs:133-188` — `commit` returns a `Result` for an error its doc calls
  unreachable, which both callers `expect`. Target: `commit` panics where it fails and returns
  `Relayout`.
- [ ] Test gaps: `land` moves `graph_revision` on a node-set change only, undo and redo set
  `dirty`, a disk-backed run then a first save prepares the root and flushes, `clear_program`
  resets `disk_backed`, `default_preferences` has `relative_paths == true`. Setup: use
  `wake::internals::noop()` in the host and bridge tests, one `DocumentFixture` helper for the
  stub `DetachedNode`, merge `ties_round_trip_as_ron` and `dock_layout_round_trips_as_ron` into
  `document_round_trips_in_either_format`, and assert the partial preferences parse once.
- [ ] Comments: `document/mod.rs:151-192` lists callers that moved, `graph_view.rs:65-66`
  counts callers, `status_log/mod.rs:51` "config'd", `io/cache/tests.rs:28` names `build`.

### 11. Tracing and the session's lifetime

Files: `lib.rs`, `driver/{mod.rs, log_ring.rs}`, `app/src/{main.rs, mcp/mod.rs,
mcp/session_thread.rs}`, `common/src` (a product `TempDir`).

- [ ] `driver/mod.rs:125-134`, `lib.rs:127-152`, `log_ring.rs:31-46`, `main.rs:41` — headless
  logging is split: the editor thread's events reach only the ring, worker warnings reach only
  stderr, the thread default has no level filter so every `trace!` on that thread is built
  each frame, and in window mode `RUST_LOG=error` hides warnings from the agent. Two drivers on
  one thread in `tests.rs:402, 517` break the "one per thread" contract. Target: one tracing
  init with an optional ring, called from `main` for every launch; the ring filters to WARN per
  layer and stderr has its own `EnvFilter`; `Driver::new(config, log)` takes the ring as
  `Driver::window` does.
- [ ] `lib.rs:32-52, 111-124` — `run_driven` forwards to `run_window`, and `load_icon` returns
  `Option` for an embedded PNG a test decodes. Target: rename `run_window` to `run_driven`,
  `run` calls it, and `load_icon` `expect`s.
- [ ] `app/src/mcp/mod.rs:63-86`, `driver/mod.rs:516-537` — with `--mcp` and a window, the
  `Driver` lives on a detached thread, so closing the window exits without dropping
  `SessionDir`: the project copy (up to 256 MiB) and every shot stay in tmpfs. `SessionDir`
  copies `common::TempDir` with its own mutable static, skips `first_free`, and panics on an
  I/O failure. Target: the main thread drops the session when the window ends, a product
  `TempDir` in `common` with `first_free`, and `DriverError::Io` on failure.
- [ ] `session_thread.rs:91-102` — each session installs a panic hook and never removes it, so
  parallel tests can lose a hook. Target: one hook, installed once, that records by
  `ThreadId`, with an `expect` naming the global it needs.

### 12. Cut gui ↔ driver and open one way

Files: `gui/dialogs.rs`, `gui/app/mod.rs`, `driver/{action.rs, error.rs, offscreen.rs,
backend.rs}`, `lib.rs`. After batch 3.

- [ ] `gui/dialogs.rs:15`, `gui/app/mod.rs:22, 817-848` — gui imports `driver::DialogAnswer`
  and builds `DriverError`, while the driver imports `App` in eight files. Target:
  `DialogAnswer` lives in `gui::dialogs` (published from `lib.rs`), `open_driven` returns a
  gui `DrivenOpenError { Unsaved, NotOpened }`, and the driver converts it with `From`.
- [ ] `offscreen.rs:84-102`, `backend.rs:88-93` — offscreen `open` builds a new `App` and
  detects failure by comparing paths, so the harness never runs the window's open path.
  Target: both backends call `open_driven` and `new_driven`, refusing unsaved edits in the
  window and discarding them offscreen as the decision says. Relaunch stays only after a quit.

### 13. The shot pipeline

Files: `driver/{shot.rs, display.rs, offscreen.rs, window_backend.rs, mod.rs, backend.rs}`.
After batch 3.

- [ ] `offscreen.rs:122-125`, `shot.rs:150-180, 250-254` — a 1600×1000 shot copies the
  screenshot (6.4 MB) only for a `&mut` borrow, rebuilds the RGBA8 bytes texel by texel
  (6.4 MB), and `fit` clones an image that already fits. Target: a `&self` accessor to the
  `Ui`, `fit(RgbaImage) -> RgbaImage` by value, the pixel `Vec` moved.
- [ ] `window_backend.rs:118-122` — `Shot::of` resizes and encodes on the main thread, which
  stalls the person's window for tens of ms. Target: the request replies with the screenshot
  and the legend, and the driver thread fits and encodes.
- [ ] `driver/mod.rs:238-251`, `backend.rs:95-105`, `shot.rs:43` — the file path goes through
  both backends only to be stored. Target: the backend returns image and legend, and
  `Driver::shot` names and writes the file.

### 14. Driver waits on wakes

Files: `driver/{mod.rs, offscreen.rs, window_backend.rs}`, `gui/app/offscreen_signals.rs`,
`gui/app/mod.rs`. After batch 13.

- [ ] `driver/mod.rs:257-277`, `offscreen.rs:141-176` — offscreen `wait_run` runs a settle with
  at least one frame every 5 ms, 100–200 frames/s that step UI time 2–3× real time, on the
  cores the worker needs (lavapipe on CI). Target: `OffscreenSignals` gets a `Condvar` that
  `note_wake` signals, and the waits block on it with a deadline.
- [ ] `window_backend.rs:139-161` — after the frame settles, `settle` reads previews and capture
  through `run_on_main(.. true)`, which repaints every window, so a converting preview forces
  about 60 frames/s. Target: the app publishes "previews in flight or capturing" in an atomic
  beside `native_open`.

### 15. One frame step

Files: `gui/app/{mod.rs, session/*, discard_dialog.rs, driven_requests.rs}`,
`gui/frame/requests/mod.rs`, `gui/state/run_state/*`, `gui/main_window/status_bar/tests.rs`.
After batches 4 and 10.

- [ ] `gui/app/mod.rs:692-789`, `session/internals.rs:109-157`, `session/bench.rs:255-280` — the
  frame is composed three times and has drifted: the harness skips `reconcile_caches` and
  never clears `requests`, and the bench runs `update`'s sweeps in each record pass. The
  allocation gate (`session/tests.rs:50`) audits only the harness copy. Target: one `Session`
  frame step that all three call, and the gate run through `App` with `UiHarness::frame_app`.
- [ ] `gui/app/mod.rs:758-760`, `requests/mod.rs:436-439` — `requests.clear()` never removes
  anything, as `drain_requests` already emptied the queue. Target:
  `debug_assert!(requests.is_empty())`, and delete `Requests::clear`.
- [ ] `gui/app/mod.rs:55-108` — the unsaved-changes guard is seven methods and a field on
  `App`. Target: `DiscardGuard` in `discard_dialog.rs`, whose `record` returns the transition.
- [ ] `run_state/mod.rs:145-161` — `compiled`, `activity`, `runs_ended`, `cache_ram` and
  `previews` are `pub(crate)` and writable anywhere, though `App` is the only writer. Target:
  private fields with `const fn` readers.
- [ ] `gui/app/mod.rs:907-916` — `a_handed_in_transition_carries_its_path_through_the_prompt`
  tests a derive. Target: delete it, and add a table test on an offscreen `App`: a dirty
  document, `open_document_at(path)`, then Discard, Save then cancel, and Cancel.
- [ ] `gui/app/mod.rs:937-946, 1023-1032`, `driven_requests.rs:138-147` — the `App` test setup
  is copied three times. Target: one `#[cfg(test)]` constructor at the end of `gui/app/mod.rs`.
- [ ] `run_state/tests.rs` — untested: log attribution from `RunSummary.logs`, an entry kept
  only for `ram > 0`, and `runs_ended` counting `Completed` and `Exec` but not
  `EventLambdaPanic`.
- [ ] `status_bar/tests.rs:69-82` — `> 0.0` and `with > without`. Target: `2 * PAD_Y` and
  `2 * PAD_Y` plus one body line height.
- [ ] Comments: `main_window/mod.rs:45` and `gui/app/mod.rs:97` name `Editor`;
  `session/internals.rs:16, 109` say `frame()` returns the first pass's commands;
  `frame/app_ctx.rs:199-201` is contradicted by `status_bar::show`.

### 16. Canvas layering

Files: `gui/pane/graph_ui/**`, `gui/widgets/support.rs`, `gui/theme/*`. After batches 5, 6
and 7.

- [ ] `draw/wire/mod.rs:74-179`, `draw/tie/{tie_layout.rs, band.rs}` — `Wire`, `TieLayout` and
  `BandAxes` are geometry that gestures hit-test. Target: move them to `frame/`.
- [ ] `breaker_ui/scribble/mod.rs:157-177`, `tie_ui/tie_line.rs:8, 215-245` — the cubic
  sampling and hull reject are written twice, the scribble rebuilds `Wire::hull` by hand,
  `tie_line` imports `BEZIER_SAMPLES` from the scribble, and `crosses` takes a square root for
  every one of its 16 samples. Target: `Wire::chords()` and `BEZIER_SAMPLES` beside `Wire`,
  and the length guard once per pass.
- [ ] `draw/wire/glyph_drag.rs`, `draw/anchored_menu.rs`, `draw/context_menu.rs` — gesture state
  under `draw`. Target: move to `gesture/`, and rename `ContextMenu<K>` (palantir's
  `ContextMenu` is in the prelude) to `ItemMenu<K>`, with `open_on` from both menus on it.
- [ ] `gesture/wire_ui/mod.rs:276-311`, `gesture/subscription_ui.rs:206-248` — committed-wire
  paint, which reads no gesture. `gesture/tie_ui/tie_widget.rs` — band paint and hit tests.
  Target: `draw/wire` and `draw/tie`.
- [ ] `node_ui/wid.rs`, `port_row.rs:206-221, 612-614`, `header.rs:110-112`,
  `preview_row.rs:182-184`, `draw/inspectors.rs:459-466` — polled ids live in the files that
  draw them, so `frame` imports `node_ui`. Target: one leaf `graph_ui/wid.rs`.
- [ ] `node_ui/port_color/mod.rs:26-56`, `node_widget.rs:229-238` — theme lookups as free fns,
  with an `unreachable!` arm for `Any`. Target: `Theme::port_color(&DataType)` with one
  exhaustive match and `Theme::exec_color(&ExecStatus)`.
- [ ] `frame/cull_region/mod.rs:19, 58-60` — `keeps_wire` is `keeps_rect(wire.hull())`. Target:
  delete it.
- [ ] `mod.rs:520-528`, `canvas.rs:28-43`, `cull_region/mod.rs:32-51` — the wire previews and
  the cull derive the world pointer from the inner canvas's origin, which equals the outer
  one. Target: `CullRegion::from_size`, one `canvas::world_pointer(ui, viewport)`, and no
  `canvas_origin` parameters.
- [ ] `node_ui/port_row.rs:531-561` — `PreviewSpawn` builds intents for `gesture/preview_drag`.
  `frame/canvas_geometry/mod.rs:247-385` — `PortLayer`. `draw_ctx.rs:294-335` — `Selection`.
  Target: `PreviewSpawn` into `preview_drag`, `port_layer.rs`, `selection.rs`.
- [ ] `node_ui/node_widget.rs:35-58` — `NodeWidget` is built and consumed on one line and has
  no setters. Target: `NodeUi::draw_node`.
- [ ] `gui/widgets/support.rs:51-154` — card chrome, pads, `LabeledValue`, `bare_value`,
  `play_triangle_at` and `frame` serve only `node_ui` and `toolbar`, against the "no graph
  knowledge" doc, and `frame` takes the refused word. Target: `node_ui/card.rs`, the toolbar
  pieces into `toolbar.rs`, `frame` renamed.
- [ ] Names: `centre` beside `center` (`tie_layout.rs:64, 375`, `band.rs:63, 117`), `n_rows`
  (`port_row.rs:75`), `ctx` for an x (`toolbar.rs:168`), two private `Pick` and two
  `PortLabel` of different meanings.
- [ ] `draw/wire/mod.rs:283-307` — the chord-share ramp is inline and untested, and re-tests the
  cull up to three times a piece. Target: a plain fn from pieces to shares, tested.
- [ ] Tests: move the preview-card and port double-click tests (`mod.rs:646-721`) and the
  `NodeKey` test (`draw/wire/tests.rs:180-222`) beside their code; merge
  `emphasis_tiers_fade_dim_and_lift` and `stroke_lets_the_broken_alarm_win_over_hover` into one
  six-row table; exact rows with boundaries in `canvas_bg.rs:409-427`; a reason for `< 0.01` in
  `canvas_geometry/tests.rs:59-63`; the name of `port_color/tests.rs:79`.
- [ ] Comments: `mod.rs:311-315, 479-481`, `canvas_geometry/mod.rs:23-32, 299-301`,
  `draw/mod.rs:4-5`, `draw_ctx.rs:19`, `header.rs:294`, `draw/wire/mod.rs:209-219, 357-375,
  425-427`.

### 17. Gesture shape

Files: `gui/pane/graph_ui/{gesture/*, mod.rs, node_ui/mod.rs, prepass_ctx.rs}`,
`gui/widgets/gesture_slot.rs`. After batch 16.

- [ ] `node_ui/mod.rs:34`, `tie_ui/tie_widget.rs:33`, `preview_drag/mod.rs:31` — three
  `GroupDrag`s for one pointer, each advanced each prepass, and `advance` re-latches on the
  same widget because a stale anchor can sit in another owner. Target: one `GroupDrag` on
  `GraphUi`; node and tie records return `Option<Grab>`.
- [ ] `wire_ui/mod.rs:34, 38`, `new_node_ui/mod.rs:36, 41`, `mod.rs:344-346, 433-442` — the
  drop → palette → re-float flow runs through `pending_open`, `source` and `resume_floating`,
  relayed by hand, and `WireUi::in_flight`'s `pending_open` term is never true. Target: a
  `CanvasInput` field the palette reads, and `without_gesture()` goes.
- [ ] `breaker_ui/mod.rs:27-142`, `tie_ui/mod.rs:33-89` — one button-latched stroke written
  twice. Target: a `ButtonStroke` that steps to `Idle | Started | Moved | Released`, and the
  breaker's release a free fn like `tie_ui::release`.
- [ ] `gui/widgets/gesture_slot.rs` — `GestureSlot` maps one to one onto `Option`, enforces
  nothing, and is mixed with `Option` in one struct; `SelectionUi::preview` is a
  `GestureSlot<()>`. Target: `Option` and a `bool`.
- [ ] `camera_gesture/mod.rs:79`, `tie_widget.rs:76`, `group_drag/mod.rs:306`,
  `new_node_ui/mod.rs:95-100`, `wire_ui/mod.rs:158`, `mod.rs:417-420, 551-552` — one phase step
  under four verbs, `NewNodeUi` returns its intent, `WireUi` has `in_flight` and `is_dragging`,
  and the bare-canvas Deselect sits in `GraphUi`. Target: `prepass` and `apply`, every
  controller writes to `out`, one `in_flight`, Deselect in `SelectionUi::apply`.
- [ ] `selection_ui/mod.rs:65-69`, `pan_zoom/mod.rs:217-220` — hand-built `Rect` from corners.
  Target: `Rect::from_min_max`.

### 18. Widgets and theme

Files: `gui/widgets/{inline_rename, badge.rs, chip.rs, edit_buffer}/*`,
`gui/pane/{preferences.rs, image_viewer/mod.rs, mod.rs}`, `gui/graph_ctx/*`, `gui/theme/*`,
`gui/main_window/dock_panes.rs`.

- [ ] `inline_rename/mod.rs:29-32, 39-40, 82-103` — an auto id no caller uses, whose doc warns
  of its hazard, a test-only default length, and a `RenameEvent` that admits a click and a
  commit at once. Target: `InlineRename::new(wid, name, style, max_chars)` and
  `enum RenameEvent { Idle, Clicked, Committed(String) }`.
- [ ] `badge.rs:1-33, 153, 184`, `chip.rs:63-67` — "chip" names two widgets with different glyph
  contracts, and `Badge` reads its hover twice. Target: badge vocabulary in `badge.rs`, one
  glyph contract, one hover read.
- [ ] `preferences.rs:134-156, 312-325`, `edit_buffer/mod.rs` — two buffered text fields solve
  one problem. Target: `PathField` is an `EditBuffer` plus its cached problem.
- [ ] `image_viewer/mod.rs:218-223`, `preferences.rs:42-45` — two panes report a preferences
  change two ways. Target: both push to `out`.
- [ ] `graph_ctx/mod.rs:191-193` — `contains` resolves a whole `NodeCtx`. Target:
  `node_placements.contains_key`.
- [ ] `theme/app_colors.rs:62` — the link hover lerps 0.5 toward white outside
  `color::toward`, and `toward`'s doc says "hue" for an RGB lerp. Target: `color::toward` with a
  named factor.
- [ ] Names: `id` for a `WidgetId` (`preferences.rs:133, 356, 428`, `inline_rename/mod.rs:50,
  85`, `edit_buffer/mod.rs:23`), and a `path_field_wid` the test calls.
- [ ] Tests: `port_glyph.rs:293-312` tests glam, not the module — extract
  `Painted::arrow_vertices` and test exact vertices; `InputCtx::default_const_value`
  (`graph_ctx/input_ctx.rs:86-110`) needs a table test; `theme/tests.rs:101-113` computes
  luminance through `u8`; `graph_ctx/tests.rs:37, 112` and `node_ctx.rs:15` imports and an
  `assert!(len == 2)`.
- [ ] Comments: `pane/mod.rs:1-5`, `inline_rename/mod.rs:2`, `inline_rename_theme.rs:7`,
  `type_scale.rs:28`, `elapsed.rs:25`, `preferences.rs:1-3, 65, 93`, `port_glyph.rs:287-289`.

### 19. Edit facts for the relayout

Files: `core/edit/{relayout.rs, step/*}`, `core/open_document/mod.rs`, `gui/frame`,
`gui/main_window`, `gui/pane/graph_ui`. After batches 9 and 15.

- [ ] `edit/relayout.rs:1-25`, `step/{reversible.rs, set_input.rs, set_node_name.rs,
  set_viewport.rs, set_positions.rs}` — core's steps answer `invalidates_cached_geometry` from
  palantir canvas facts, and `Relayout` is defined in core. Target: steps report domain facts
  (binding kind changed, name changed, node set changed), and gui maps them to a relayout in
  `gui/frame`.

### 20. Palantir changes (go-ahead given, decision "Palantir gains three public APIs")

Files: `palantir/src/{ui, host, widgets}`, then `darkroom/src/{gui/app, driver, gui/theme}`.

- [ ] `palantir/src/ui/mod.rs:375-391`, `gui/app/mod.rs:656-668`, `view_capture.rs:39-49, 376-381`,
  `window_backend.rs:104-127` — a screenshot carries no ticket, so a person's View capture and
  a driver shot that ask in the same or near frames can take each other's image. Target:
  `request_screenshot` returns a ticket that `screenshot_taken` echoes.
- [ ] `palantir/src/host/winit/window/mod.rs:275`, `window_backend.rs:139-202` — a minimized or
  occluded window runs no frame, so each call waits 30 s and reports a timeout. Target: the
  host handle says the window is hidden, and the waits return `DriverError::WindowHidden`.
- [ ] `theme/{inline_rename_theme.rs:42, 61, palantir_bridge.rs:84-160, mod.rs:240}` — six
  hand lists of `normal, hovered, active, disabled` skip a new state silently. Target: a public
  `StatefulLook::each_mut` that destructures, and darkroom loops over it.
