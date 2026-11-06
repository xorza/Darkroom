# Review: Darkroom GUI call graph, ownership and contexts

Status: proposal. No code changed. Scope: `darkroom/src`, the GUI above all. Delete an item when it
lands, and delete this file with the last one. Keep each lasting decision in the doc comment of the
type it governs.

# Part 1: Proposal

## Result

The core is sound: `core` imports nothing of `gui`, each shared owner has a reason, and the context
chain is `Copy` and read-only. No redesign of ownership is needed. Four root causes remain. The design
below removes them with one rule for the whole GUI:

> **The update path takes `stage`. The record path takes `(ui, stage)`. A level adds to its stage's
> context and hands it down. Nothing returns upward except the frame's `InputOutcome`.**

```rust
// gui/frame/input_stage.rs
pub(crate) struct InputStage<'a, 'u, C> {
    pub(crate) update: &'a mut UpdateCtx<'u>,
    pub(crate) ctx: C,
    pub(crate) out: &'a mut Requests,
}
// gui/frame/record_stage.rs
pub(crate) struct RecordStage<'a, C> {
    pub(crate) ctx: C,
    pub(crate) out: &'a mut Requests,
}
```

```text
App::update(update)                         App::record(ui)
  Session::input(update, app, out)            Session::record(ui, app, parts)
    MainWindow::input(&mut InputStage<Graph>)   MainWindow::record(ui, &mut RecordStage<Graph>, parts)
      GraphUi::input(..)                          DockPanes -> GraphUi::record(ui, stage)
        Gestures::input(&mut InputStage<Canvas>)    Gestures::record(ui, &mut RecordStage<Draw>)
```

## The graphs as they stand

**Call graph.** `App::update` runs its sweeps, then `Session::input`, `MainWindow::{scan_navigation,
prepass}`, `GraphUi::prepass` and the input half of five controllers. `App::record` runs
`Session::record`, `MainWindow::frame`, `DockPanes`, `GraphUi::draw`, then `resolve_gestures`, which is
the input half of the other controllers, and `record_canvas`.

**Ownership.** `App` owns `Session`, `RuntimeHost`, `RunState`, `StatusLog`, `Theme`, `Preferences`,
`Dialogs`, `DiscardGuard` and `Requests`. `Session` owns `OpenDocument` and `MainWindow`, replaced as a
unit. `MainWindow` owns `GraphUi` and the viewers. A frame lends these down as contexts.

**Numbers**, from the code, tests left out.

| Measure | Value |
|---|---|
| GUI functions by arguments past `self`: 0, 1, 2, 3, 4, 5 | 336, 272, 201, 130, 18, 6 |
| Signatures with a palantir handle and a context | 22 update, 31 record |
| Context levels from `DrawCtx` to `AppCtx` | 6 |
| `CanvasCtx::new(PrepassCtx::new(..))` and `PrepassCtx::new` built by hand | 5 in `graph_ui/mod.rs` |
| Lines that name `Relayout` | 44, in 8 files |

The histogram is healthy: 24 of 963 functions pass the four-argument line. The cost is not the count
but that one bundle, a handle, a context and `out`, repeats with no name.

## Root causes

1. **A gesture's input lives in two stages.** Camera, group drag, wire, subscription and tie knobs read
   input in the prepass. Rubber band, breaker, tie line, palette opening, node grab, and tie click and
   grab read it in the record. So `CanvasInput` is parked on `GraphUi` for the record to read, `settle`
   clears it, and each drag latch and stop takes a second pass. This is the cause of most of what
   follows, and it is what `App::update` exists to end.
2. **A relayout travels up the stack by return value.** `Relayout` goes up from each prepass, each
   drain and each command, and `App::input_relayout` carries it across the two hooks by hand. The test
   apps drop it, so `SessionFrame` and `CanvasFrame` run a frame that production does not.
3. **The same bundle has no name.** `(update, ctx, out)` and `(ui, ctx, out)` repeat at each level, and
   the mutable half differs by accident: `&mut UpdateCtx`, `&mut Input` or `&Input`, with `line_px`,
   `theme` and `zoom` passed beside a context that holds them.
4. **The context chain has levels with no reader.** `GraphCtx` adds `camera` and a flag only two
   `debug_assert!`s read. `WindowCtx` adds `open`. `PrepassCtx` is nested in `CanvasCtx` only so a
   getter can hand it back, and the record reads `CanvasCtx` though it needs no input.

## Target design

### Contexts

| Type | Holds | Reader |
|---|---|---|
| `AppCtx` | `theme`, `library`, `run_state` | Session |
| `GraphCtx` | `app`, `open`, `camera` | Surround, panes, node handles |
| `PrepassCtx` | `graph`, `input: CanvasInput` | Update path, before the geometry settles |
| `CanvasCtx` | `graph`, `input`, `geometry` | Update path, after it settles |
| `DrawCtx` | `graph`, `geometry`, `selected`, `cull` | Record path |

- **`WindowCtx` merges into `GraphCtx`.** Each pane already builds one from the other, and `display` is
  known where `WindowCtx` is built. Refused: keeping both to say that the status bar reads no graph, as
  `GraphCtx::app()` still gives it the narrower view.
- **`CanvasInput` belongs to the update path alone.** With cause 1 removed, no record reads it, so
  the field `GraphUi.input` goes, and the record's contexts cannot read an edge that is spent.
- **`StatusInputs` leaves `AppCtx`.** One surface reads it, and the update path builds an `AppCtx` with
  a fake "no memory" reading. It goes in `WindowParts` with `preferences`, which replaces `FrameWrites`.

### Stages

- **`InputStage` holds the handle, and `RecordStage` does not.** The update path has no closure that
  lends its handle back, so all three fit one argument. On the record path `Panel::show(ui, |ui| ..)`
  lends `ui` to each body, and a stage holding it would be rebuilt in each closure, so `ui` stays the
  first argument, as each palantir widget takes it.
- **Public fields, so a function borrows them apart.** Shadow's
  [host context](https://shadow.github.io/docs/rust/src/shadow_rs/host/context.rs.html) takes this form
  for that reason. A method on the stage would borrow all of it.
- **`stage.with(ctx)` hands a level down**, as a reborrow with the next context. Refused: `out` inside
  the `Copy` chain, as `AppCtx` already refuses, which takes `Copy` from every context and lets two call
  sites fight over one sink; one flat context with `Option` levels, where a level can be absent at run
  time; a struct per level, six names for one shape; keeping the arguments, as
  [GPUI's `window` and `cx`](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/window.rs)
  do, which is sound but keeps the bundle unnamed.
- **Session builds the root stage per phase.** A drain needs `&mut OpenDocument` between phases, and a
  stage holds a `GraphCtx` over it, so a stage lasts one phase.
- **Cost, stated.** A helper that takes `&Input` shows today that it cannot write a request. A stage
  gives each helper `out`. A pure helper that reads one thing keeps taking that thing.

### Gestures

- **Each canvas controller has one input half and one record half, of one name and shape.**
  `input(&mut self, stage: &mut InputStage<'_, '_, CanvasCtx<'_>>)`, `record(&self or &mut self, ui,
  stage)`, `in_flight`, `reset`. Today's names are `prepass`, `apply`, `draw`, `draw_in_flight` and
  `record`, over five argument shapes.
- **All bare-canvas and node-grab edges move to `input`.** Menus, the palette's rows, chips and fields
  stay in the record, as widgets read them. Then a drag latch and stop on the canvas answer `Handled`.
  `take_clicks` becomes `claims(action)` over those edges.
- **`Gestures` owns the controllers, and `GraphUi` owns `geometry`, `ties`, `bg` and `Gestures`.** The
  flat 21 fields make each method hold all of `self`, which is why `CanvasCtx` is built by hand five
  times. Two fields borrow apart. Refused: a `Controller` trait, as the set is closed and the order of
  calls is the design; replacing the group on reset, which throws away grown buffers, so `reset` stays
  a call to each.

### One relayout sink

- **`Requests` holds the debt.** `out.owe_relayout()` at any level, and `out.spend_relayout(ui)` once
  in `App::record`. No level returns it, `App::input_relayout` and `InputEnd` go, and `frame/relayout.rs`
  becomes `EditFacts::strands_geometry`. The test apps call the same spend, so they run the real frame.
  Refused: `UpdateCtx::request_relayout`, a public method for what one queue already carries.

### One preference route

- **`App` compares and saves.** A form edits `Preferences` in place, and `App` compares it with the last
  saved copy after the record, the window field aside, and clones only on a change. Five sites raise
  `PreferencesCommand::Changed` by hand today, and one that forgets loses a save with no error. Refused:
  a clone per frame, which allocates; a setter per field, against palantir's bound widgets.

### Layers

- **`Framing` moves to `gui/frame`, and its `impl` stays in `pan_zoom`.** `frame/mod.rs` says it imports
  no level, and `app_command.rs:16` imports `pane`. That edge is the cycle `frame` to `pane` to `frame`.
- **`Hover` moves to `theme`, and `icon::{cross_span, stroke_width}` to `theme/icon_scale`.** These two
  edges (`const_editor_theme.rs:8`, `palantir_bridge.rs:9`) make `theme` and `widgets` a cycle, and
  palantir's rule is that a theme knows no widget.
- **`OpenDocument`'s fields become private with getters, tests through `internals`.** Its doc says every
  mutation goes through it. No production code writes a field from outside, so this costs only 47 reads
  and makes the claim one the compiler holds.

## Ownership

| Shared value | Holders | Verdict |
|---|---|---|
| `Wake` (`Arc<dyn Fn>`) | Worker, converter, UI | Kept: wakes the window from another thread |
| `Arc<PreviewSink>` + `Mutex` | Library, worker's lambda, runtime | Kept: the worker writes, and a recompose keeps it |
| `Arc<CompiledGraph>` | Runtime, worker, `RunState` | Kept: immutable, and crosses threads |
| `Arc<ReportMailbox>` + `Mutex` | Worker, bridge | Kept: a channel with its merge rules |
| `Jobs`, `Inbox` (`Arc<Shared>` + `Mutex`) | UI, converter thread | Kept: same |
| `NativeDialogFlag` (`Arc<AtomicBool>`) | Dialogs, driver thread | Kept: read while the main thread blocks |
| `LogRing` (`Arc<Mutex>`) | Tracing layer, driver | Kept: tracing owns its layer |
| `Rc<[DragStart]>` | The latched drag, each frame's intent | Kept: immutable, and a copy per frame costs more |
| `Arc<FsPathConfig>` | Scenarium's `DataType`, `PathPick` | Kept: a foreign shape |
| macOS `SINK` (`thread_local` `RefCell`) | Platform callback | Kept: the callback has no user data |

## Refused after review

- **Removing `Session`.** It looks like a pass-through, but it holds the rule that a document and its
  panes are replaced as one.
- **Replacing `NodePass`, `TiePass`, `WirePass` and `NodeRecord`.** Each holds extras beyond a stage, so
  each keeps its name and holds a stage's fields.
- **Moving the driver off `gui`.** It reads the app as a client, one way.
- **Flattening for speed.** By my estimate a context copies about 170 bytes at most, I did not measure it, and `.notes/PERF.md` does not name them.

## Not known yet

- **Saving.** I have no number for a drag latch. The `click` case measured 111 µs for one record, layout
  and cascade on bare canvas, on one high-end laptop core.
- **The move of the rubber band and the breaker.** I read their reads, but did not build them. A read in
  the record that the list above misses shows as a failed test.

# Part 2: Plan

Each batch leaves the gates green. Batch 1, the relayout sink, batch 2, the layers, batch 3, the
contexts, and batch 4, the update path, have landed.

## Batch 5: gestures in one stage

Files: `gui/pane/graph_ui/{mod,prepass_ctx,canvas_ctx}.rs`, `gui/pane/graph_ui/node_ui/mod.rs`,
`gui/pane/graph_ui/gesture/{selection_ui,breaker_ui,tie_ui,new_node_ui,button_stroke}*`.

- [ ] `selection_ui/mod.rs:100`, `breaker_ui/mod.rs:52`, `tie_ui/mod.rs:53`, `button_stroke.rs:44`: `apply`
  becomes `input`, read off `stage.update.input`.
- [ ] `node_ui/mod.rs:258-290`: the grab latch moves to the input stage, with `Grab`'s selection edit, and
  `draw_all` and `draw_node` return nothing.
- [ ] `tie_widget.rs:164-201`: the tie's click, right click and grab move too, and `TiesRecorded` goes.
  `open_on` takes `&mut Input` alone, and `tie_menu_ui.rs:51` follows.
- [ ] `new_node_ui/mod.rs:101`: the palette's opening moves. Its rows stay in the record.
- [ ] `graph_ui/mod.rs:417`: `take_clicks` becomes `claims(action)`, which also takes latches and stops on
  the outer canvas, node handles and ties, and answers `Handled`. Extend
  `the_stage_answers_handled_only_for_a_click_it_takes_alone`: a latch and a stop now answer `Handled`,
  and a title's click stays `Record`.
- [ ] `graph_ui/mod.rs:134`: the field `input` goes, with its reset in `settle`, and `CanvasInput` lives on
  the stack of `GraphUi::input`. `canvas_ctx.rs` holds it only on the update path.
- [ ] `graph_ui/mod.rs:146-163`: `Gestures` holds the controllers, and `CanvasCtx::new` by hand goes.
- [ ] Add a pass count to `a_click_the_input_stage_takes_records_once`: a node drag's latch and stop, and
  a rubber band's, each take 1.

## Batch 6: record path

Needs batch 5. Files: `gui/frame/record_stage.rs` (new), `gui/main_window/*`, `gui/pane/{inspector,preferences}.rs`,
`gui/pane/image_viewer/mod.rs`, `gui/pane/graph_ui/{mod,toolbar}.rs`, `gui/pane/graph_ui/gesture/*`,
`gui/pane/graph_ui/node_ui/*`, `gui/pane/graph_ui/draw/*`.

- [ ] `record_stage.rs`: `RecordStage`. `(ui, ctx, out)` at 17 signatures becomes `(ui, stage)`, as
  `inspector.rs:45,89`, `toolbar.rs:48`, `menu_bar.rs` and `status_bar`.
- [ ] `breaker_ui/mod.rs:83`, `selection_ui/mod.rs:195`, `tie_ui/mod.rs:81`, `wire_ui/mod.rs:226`,
  `subscription_ui.rs:171`: one `record(ui, ctx: DrawCtx)`, and `theme` and `zoom` go.
- [ ] `NodePass`, `TiePass`, `WirePass`, `NodeRecord`: hold a `RecordStage` and their extras.
- [ ] Names: `MainWindow::frame` and `GraphUi::draw` become `record`, `prepass` becomes `input`, and
  `pane/graph_ui/frame/` becomes `pane/graph_ui/scene/`, as "frame" is one pass of the render loop.

## Batch 7: one preference route

Files: `gui/app/mod.rs`, `gui/frame/app_command.rs`, `gui/pane/preferences.rs`, `gui/pane/image_viewer/mod.rs`,
`core/io/preferences/mod.rs`.

- [ ] `app/mod.rs:540`: `App` keeps the last saved `Preferences`, and after each record compares with it,
  the window field aside. On a change it saves and clones.
- [ ] `preferences.rs:93,146,195,471`, `image_viewer/mod.rs:350`: the raises go, with
  `PreferencesCommand::Changed`.
- [ ] `apply_preferences`: `set_ml_model_paths` runs only when `ml_models` moved, as now it recomposes on
  each change. Test: an edit in the form saves once, a resize saves nothing, and an edit back saves
  again.

## Sources

- [eframe: `App::logic` and `App::ui`](https://docs.rs/eframe/latest/eframe/trait.App.html)
- [Shadow: context structs with public fields](https://shadow.github.io/docs/rust/src/shadow_rs/host/context.rs.html)
- [GPUI: `Window`](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/window.rs)
- [Split borrows](https://telex-tui.github.io/blog/rust-patterns-split-borrows.html)
