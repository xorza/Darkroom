# Review: `darkroom/`

## Proposal

### Root causes

- **A run verdict is three fields that must agree.** `NodeRunState` holds `status`, `error` and `missing_inputs` side by side. Live progress writes only `status`, so a stale `error` from the last run shows beside a fresh `Executed` status until the run completes.
- **A revision counter is global state.** `GraphRevision::fresh` uses a process-wide `static AtomicU64`. Its only readers (`MainWindow::swept_at`, `OutputTypeCache`) live in the `MainWindow` that `Session::new` replaces together with the document, so a per-document counter gives the same guarantee.

### Target design

- **A run verdict is one value.** `ExecStatus` carries its own detail (`Errored(Option<String>)`, `MissingInputs(Vec<usize>)`), so a status and its detail cannot disagree.
- **The graph revision is per document.** `OpenDocument` advances its own counter, as `DocumentQueue` does for `GestureId`.

## Plan

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
