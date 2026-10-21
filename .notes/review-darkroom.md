# Review: `darkroom/`

## Proposal

Every root cause is fixed. The batches left are local: shared machinery in the wrong module, uneven APIs, duplicates and docs that read wrong.

## Plan

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
