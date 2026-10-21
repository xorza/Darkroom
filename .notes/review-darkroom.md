# Review: `darkroom/`

## Proposal

### Root causes

- **The GUI's shared vocabulary lives at the top of the tree.** `AppCommand` and its groups (`gui::app::app_command`), `AppCtx` (`gui::app::app_ctx`), `WindowCtx` (`gui::main_window::window_ctx`) and `PathPick` (`gui::pane::graph_ui::node_ui::port_row`) are what every surface speaks, but each lives in the module that consumes it last. So `pane`, `main_window`, `requests` and `graph_ctx` import upward into `app` and `main_window`, and `app` imports down into a pane leaf for `PathPick`. Every GUI module sits in one import cycle.
- **A run verdict is three fields that must agree.** `NodeRunState` holds `status`, `error` and `missing_inputs` side by side. Live progress writes only `status`, so a stale `error` from the last run shows beside a fresh `Executed` status until the run completes.
- **Preview pixels are prepared on the main thread.** `PreviewStore::ingest_preview` (from `App::update`) and `PreviewImage::full` (from the record pass) both copy a planar image to interleaved and convert it to RGBA8. A full-sensor frame takes a frame hitch of hundreds of milliseconds.
- **A revision counter is global state.** `GraphRevision::fresh` uses a process-wide `static AtomicU64`. Its only readers (`MainWindow::swept_at`, `OutputTypeCache`) live in the `MainWindow` that `Session::new` replaces together with the document, so a per-document counter gives the same guarantee.

### Target design

- **`gui::frame` — the vocabulary layer.** It holds the context chain (`AppCtx` + `StatusInputs`, `WindowCtx`), `FrameWrites`, `Requests`, and `AppCommand` with its groups (`FileCommand`, `RunCommand`, `PrefsCommand`, `EditCommand`, `MlModelKind`, `PathPick`) as plain data. `gui::app` keeps the behavior: one `App::run_command(&mut self, AppCommand) -> Relayout` match replaces the five `apply` dispatchers. Then the layers read top to bottom: `app → main_window → pane → {graph_ctx, state, theme, widgets, frame} → core`.
- **A run verdict is one value.** `ExecStatus` carries its own detail (`Errored(Option<String>)`, `MissingInputs(Vec<usize>)`), so a status and its detail cannot disagree.
- **Preview preparation runs off the main thread.** One background converter owned by `RunState` turns a published value into a thumbnail raster, and later into a full raster on a viewer's first ask. The main thread only uploads finished rasters.
- **The graph revision is per document.** `OpenDocument` advances its own counter, as `DocumentQueue` does for `GestureId`.

## Plan

### 4. `gui::frame`: the vocabulary layer

Files: `gui/app/app_ctx.rs`, `gui/main_window/window_ctx.rs`, `gui/frame_writes.rs`, `gui/requests/`, `gui/app/app_command/`, `gui/app/mod.rs`, all importers.

- [ ] `gui/app/app_ctx.rs`, `gui/main_window/window_ctx.rs`, `gui/frame_writes.rs`, `gui/requests/`: the context chain and the request queue live in the levels that use them last, so `graph_ctx` imports `main_window` and `main_window` imports `app`. Target: `gui/frame/{app_ctx.rs, window_ctx.rs, frame_writes.rs, requests/}`.
- [ ] `gui/app/app_command/`: `AppCommand` and its groups are data that every surface raises, and `pane`, `main_window` and `requests` import `gui::app` for them. Target: `gui/frame/app_command.rs` (the groups and `MlModelKind` as satellites), data only.
- [ ] `gui/pane/graph_ui/node_ui/port_row.rs` `PathPick`: a command payload in a pane leaf, which `gui::app` imports. Target: beside `EditCommand` in `gui/frame/app_command.rs`.
- [ ] `gui/app/app_command/*.rs` `apply`: five dispatchers forward to `App` methods, and `RunCommand::Cancel` reaches into `app.runtime` directly. Target: one `App::run_command(&mut self, command: AppCommand) -> Relayout` in `gui/app/mod.rs`.
- [ ] `gui/app/mod.rs`: `load_picked_document`, `new_document`, `load_document`, `save_current`, `save_preferences`, `set_confirm_unsaved` and `run_graph` are `pub(crate)` with no caller outside `gui::app`. Target: private.

### 5. Preview preparation off the main thread

Files: `gui/state/preview_store/mod.rs`, `gui/state/run_state/mod.rs`, `gui/pane/image_viewer/mod.rs`, `gui/app/mod.rs` (wake wiring).

- [ ] `gui/state/preview_store/mod.rs` `prepare_content` (called from `App::update`): `LensImage::interleaved` copies a planar image whole, then `Preview::to_rgba8` downsamples it, on the main thread for every published image. Target: a `PreviewConverter` owned by `RunState` (one background thread, a job channel, and a result channel woken through the existing `Wake`). `RunState::sync` sends the published values and takes the finished thumbnail rasters. The main thread only calls `ui.load_image`.
- [ ] `gui/state/preview_store/mod.rs` `PreviewImage::full` (called from the record pass): the full-resolution copy and RGBA8 conversion run inside the frame that first shows a viewer. Target: the first ask queues a full-raster job with the device's `max_image_dimension`. `FullImage` gains `Converting`, the viewer draws the thumbnail stretched to `native_size` until the raster arrives, and the upload happens in the next `sync`.

### 6. Per-document graph revision

Files: `core/document/graph_revision.rs`, `core/document/open_document/mod.rs`.

- [ ] `core/document/graph_revision.rs` `GraphRevision::fresh`: a process-wide `static AtomicU64`, which the "no mutable statics" rule refuses. Both readers live in the `MainWindow` that a document swap replaces. Target: `GraphRevision::next(self)` on a `Default` value, with `OpenDocument::land` advancing its own field, as `GestureId` does.

### 7. Run verdicts as one value

Files: `gui/state/run_state/mod.rs`, `gui/state/preview_store/mod.rs`, `gui/graph_ctx/node_ctx.rs`, `gui/pane/graph_ui/node_ui/{header.rs, port_row.rs, node_widget.rs}`, `gui/pane/graph_ui/paint/inspectors.rs`, `gui/pane/image_viewer/mod.rs`.

- [ ] `gui/state/run_state/mod.rs` `NodeRunState`: `error` and `missing_inputs` sit beside `status`. `apply_progress` rewrites only `status`, so the inspector shows last run's error under a fresh `Executed` status until `Completed` arrives. `drop_empty_nodes` also has to know which fields imply a status. Target: `ExecStatus::Errored(Option<String>)` and `ExecStatus::MissingInputs(Vec<usize>)` (no longer `Copy`, so readers take `&ExecStatus`), and the two fields removed.
- [ ] `gui/state/preview_store/mod.rs` `PreviewStore::entries`: `pub(crate)`, so `RunState` removes and clears entries and `ImageViewer` reads them without passing through the store. Target: a private field with `get`, `remove` and `clear` on the store.

### 8. Shared camera machinery out of the graph pane

Files: `gui/pane/graph_ui/gesture/gesture_slot.rs`, `gui/pane/graph_ui/gesture/pan_zoom/mod.rs`, `gui/pane/image_viewer/{mod.rs, camera/mod.rs}`, `gui/widgets/mod.rs`.

- [ ] `gui/pane/image_viewer/mod.rs:39-40`, `image_viewer/camera/mod.rs:14`: the viewer imports `GestureSlot`, `pan_zoom::scrolled` and `ZoomRange` from inside `graph_ui::gesture`. Target: `GestureSlot` and the pane-neutral zoom algebra (`ZoomRange`, `scrolled`, `scroll_to_zoom_factor`) in `gui/widgets/`. `Framing` and `CameraGesture` stay in `graph_ui`.

### 9. Gesture controller symmetry

Files: `gui/pane/graph_ui/mod.rs`, `gui/pane/graph_ui/gesture/{subscription_ui.rs, new_node_ui/mod.rs, node_menu_ui/mod.rs}`.

- [ ] `gui/pane/graph_ui/gesture/subscription_ui.rs`: has `is_dragging` where every sibling has `in_flight`, and `GraphUI::in_flight` asks it by the other name. Target: `in_flight`, beside the `is_dragging` that the wire fade reads.
- [ ] `new_node_ui/mod.rs` `NewNodeUi::apply` returns intents for the caller to extend, `node_menu_ui/mod.rs` `NodeMenuUi::apply` takes a `GraphCtx`, and `NodeMenuUi::open_on` returns intents, while the other controllers take a `CanvasCtx` and push into `&mut Requests`. Target: `apply(&mut self, ui, cx: CanvasCtx<'_>, out: &mut Requests)` for all of them.

### 10. Duplicates and docs that read wrong

Files: `gui/event_ref.rs` and its importers, `gui/graph_ctx/node_ctx.rs`, `gui/pane/graph_ui/node_ui/header.rs`, `core/document/tab_ref.rs`, `gui/main_window/mod.rs`, `gui/pane/image_viewer/mod.rs`, `gui/state/process_memory.rs`.

- [ ] `gui/event_ref.rs` `EventRef`: the same two fields as `scenarium::EventPort` (`node_id`, `event_idx`), so one event has two identity types. Target: `EventPort` everywhere, with `NodeKey` implemented for it.
- [ ] `gui/graph_ctx/node_ctx.rs` `cache_controls` and `can_evict_cache`: identical bodies under two names, one rule ("has an output a cache can key"). Target: one predicate that both header chips read.
- [ ] `gui/graph_ctx/node_ctx.rs` `can_disable`: `self.sink() && !self.missing()`, where `sink()` is already false for a missing func. Target: `self.sink()`.
- [ ] `core/document/tab_ref.rs` `TabRef`: says tabs are undoable and that a viewer shows "one port's runtime image — one tab per port". The layout records no undo step, and a viewer is keyed by its preview node. Target: docs that state both.
- [ ] `gui/main_window/mod.rs` `MainWindow::image_viewers`: doc says "keyed by the port it shows". Target: keyed by the preview node.
- [ ] `gui/pane/image_viewer/mod.rs` `NOTHING_YET_HINT`: the user sees "the port's image appears here…" on a viewer that shows a preview node. Target: name the preview node.
- [ ] `gui/state/process_memory.rs` module doc: refers to the cache's `RAM` clause. The status bar labels it `Cache`.
