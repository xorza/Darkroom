//! The per-frame GUI edit pipeline over a borrowed [`OpenDocument`].
//!
//! `Editor` owns undo history, the derived [`Scene`] and run projections,
//! the GUI tree, and transient gesture state. [`App`] lends it the workspace's
//! document for each operation, keeping document/runtime ownership shared with
//! terminal frontends without giving terminal sessions GUI responsibilities.
//!
//! [`App`]: crate::gui::app::App

use aperture::Ui;
use scenarium::Graph;
use scenarium::Library;

use crate::core::document::dock::{DockOp, TabAddress};
use crate::core::document::open_document::OpenDocument;
use crate::core::document::{GraphRef, PortKind, PortRef, TabRef};
use crate::core::edit::action_stack::ActionStack;
use crate::core::edit::intent::apply::commit_intent_cascading;
use crate::core::edit::intent::duplicate::{
    build_duplicate_intent_for, remove_selection_intents, selected_node_ids,
};
use crate::core::edit::intent::types::{Intent, NodeProperty};
use crate::core::io::preferences::Preferences;
use crate::gui::UiAction;
use crate::gui::app::commands::AppCommand;
use crate::gui::canvas::node_menu::NodeMenuAction;
use crate::gui::main_window::MainWindow;
use crate::gui::run_state::RunState;
use crate::gui::scene::Scene;
use crate::gui::theme::Theme;

use crate::gui::app::AppContext;

/// Keyboard chords → intents/commands. A child module so it can drive the
/// pipeline through `Editor`'s private fields (undo stack, intent buffer,
/// dirty flags) without widening their visibility.
mod shortcuts;

/// Byte budget for the undo history's packed buffer (~1 MiB). Bounds
/// memory rather than entry count — a single large edit can't be
/// undone away, but the oldest entries drop once the buffer overflows.
const UNDO_HISTORY_BYTES: usize = 1 << 20;

#[derive(Debug)]
pub(crate) struct Editor {
    /// Unsaved-changes flag: set whenever a content-changing step is
    /// applied (via [`Document`]'s edit paths — new edits, undo/redo
    /// replay, and the direct graph mutations), cleared on save. Only
    /// steps that alter saved content flip it — pure navigation (camera,
    /// selection, tab focus) doesn't (see [`UndoStep::dirties_document`]).
    /// Read by `App` on exit to decide whether to prompt. It can read
    /// "dirty" after an undo returns the document to its saved state — the
    /// safe direction (prompt rather than silently discard).
    pub(crate) dirty: bool,
    action_stack: ActionStack,
    scene: Scene,
    main_window: MainWindow,
    /// Which graph `scene` last reflected. A mismatch with the active
    /// target means the tab changed: drop transient gesture state
    /// (`reset_transient`) and request a relayout. `None` forces that on
    /// the next frame (a fresh `Editor`). It does *not* gate the rebuild
    /// itself — the projection is rebuilt every frame regardless.
    scene_target: Option<GraphRef>,
    /// Set by `drain_intents` whenever it applies a step; consumed by the
    /// pre-record rebuild so the record sees doc edits the pre-record
    /// drain made (drag, connection commit). Only meaningful in the
    /// window between the unconditional pre-prepass rebuild (which clears
    /// it) and the pre-record rebuild.
    scene_dirty: bool,
    /// Per-frame accumulator: set by any step/transition that changes
    /// something the layout engine reads (see `requires_relayout`), and
    /// consumed once at the end of `frame` as a single
    /// `ui.request_relayout()`. Reset at the top of every frame. A plain
    /// side-effect field like `scene_dirty`, rather
    /// than a `bool` threaded back through every helper's return.
    needs_relayout: bool,
    /// Set when a cache-mode toggle (`Intent::SetNodeProperty` with a
    /// `NodeProperty::RuntimeCache`) is applied this frame. `App` consumes it via
    /// [`Self::take_caches_dirty`] to flush the node's resident value to disk (a
    /// `SaveCaches` to the worker) without a re-run.
    caches_dirty: bool,
    /// Per-frame scratch buffer of pending mutations. Cleared at the
    /// top of every `frame`, filled by prepass/record/shortcut
    /// handling, and fully drained before `frame` returns — it carries
    /// no state across frames. Kept as a field only to reuse the
    /// allocation; not part of the observable state.
    intents: Vec<Intent>,
    /// Per-frame scratch buffer of view-state requests (open/activate/
    /// close tab) raised during record. Drained each frame; carries no
    /// cross-frame state — kept only to reuse the allocation.
    actions: Vec<UiAction>,
    /// The last completed run's per-node state, keyed by the document's
    /// `NodeId`s so it resolves against any tab (root or graph
    /// interior): execution status (the glow + header time, projected into
    /// each `SceneNode::exec_status` at rebuild) and log lines. `App` drives
    /// it as it drains the worker (`RunState::set_results` / `apply_progress`
    /// / `clear`). Off the serialized state.
    pub(crate) run_state: RunState,
}

impl Editor {
    /// Build fresh GUI editing state for an open document.
    pub(crate) fn new() -> Self {
        Self {
            dirty: false,
            action_stack: ActionStack::new(UNDO_HISTORY_BYTES),
            scene: Scene::default(),
            main_window: MainWindow::default(),
            scene_target: None,
            scene_dirty: false,
            needs_relayout: false,
            caches_dirty: false,
            intents: Vec::new(),
            actions: Vec::new(),
            run_state: RunState::default(),
        }
    }

    /// Apply a single `intent` against the active target and record it as
    /// its own undo entry. For edits raised *outside* the frame's intent
    /// drain — e.g. a file-picker result `App` handles after the record.
    /// No-ops (and self-cancelling steps) are dropped, like the in-frame
    /// drain.
    pub(crate) fn apply_edit(
        &mut self,
        open: &mut OpenDocument,
        intent: Intent,
        library: &Library,
    ) {
        let target = open.document.active_target().unwrap_or(GraphRef::Main);
        self.commit_batch(open, target, library, [intent]);
    }

    /// Apply a batch of externally-sourced `intents` (e.g. from a script)
    /// against the active target as a single undo entry — the multi-intent
    /// analogue of [`Self::apply_edit`]. Used by `App` when draining the
    /// script inbound queue before the frame; the unconditional pre-prepass
    /// rebuild folds the edits in, so `scene_dirty` needn't be set here.
    pub(crate) fn apply_external_intents(
        &mut self,
        open: &mut OpenDocument,
        intents: Vec<Intent>,
        library: &Library,
    ) {
        let target = open.document.active_target().unwrap_or(GraphRef::Main);
        self.commit_batch(open, target, library, intents);
    }

    /// Build, apply, and record `intents` against `target` as one undo
    /// entry, accumulating the frame's relayout/reconcile signals. Returns
    /// whether anything applied. Shared core of [`Self::apply_edit`],
    /// [`Self::apply_external_intents`], and [`Self::drain_intents`]: no-op
    /// and stale intents (anchor node already gone) are dropped per-intent,
    /// and an empty batch records nothing.
    fn commit_batch(
        &mut self,
        open: &mut OpenDocument,
        target: GraphRef,
        library: &Library,
        intents: impl IntoIterator<Item = Intent>,
    ) -> bool {
        let mut batch = Vec::new();
        for intent in intents {
            // A `SetInput` that retypes a wildcard output cascades into dropping
            // the now-incompatible downstream wires — all one undo entry.
            for step in commit_intent_cascading(intent, &mut open.document, target, library) {
                self.needs_relayout |= step.requires_relayout();
                open.normalization_pending |= step.requires_reconcile();
                self.dirty |= step.dirties_document();
                batch.push(step);
            }
        }
        let applied = !batch.is_empty();
        if applied {
            self.action_stack.push_current(target, &batch);
        }
        applied
    }

    /// Add an imported graph to the document, flagging the reconcile
    /// the import needs: an imported graph's stored interface may not match
    /// its interior wiring (hand-edited / older file), so it's re-derived
    /// on the next rebuild. Keeps the "import ⇒ reconcile" invariant here
    /// rather than on the caller.
    pub(crate) fn import_graph(&mut self, open: &mut OpenDocument, graph: Graph) {
        open.document.import_graph(graph);
        open.normalization_pending = true;
        self.dirty = true;
    }

    /// Run one frame of the edit pipeline against the borrowed runtime
    /// context (`library`, `theme`, `host`), returning the [`AppCommand`]
    /// the frame surfaced (if any) for the next `App::update` to execute.
    ///
    /// The frame splits into a **navigation phase** (settle *which* graph
    /// is active, from frame-top inputs) and an **edit phase** (mutate that
    /// graph), because input that switches tabs/opens graphs comes from
    /// *last* frame's click responses and must resolve before anything
    /// edits or records.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn frame(
        &mut self,
        open: &mut OpenDocument,
        ui: &mut Ui,
        library: &Library,
        theme: &Theme,
        preferences: &mut Preferences,
        events_running: bool,
        status_error: Option<&str>,
    ) -> Option<AppCommand> {
        self.intents.clear();
        self.actions.clear();
        self.needs_relayout = false;

        // Settle the active tab entirely from frame-top inputs (keyboard
        // undo/redo + last-frame click responses). `navigate` reads *last*
        // frame's `scene` to resolve tab/chip clicks, so it must run before
        // this frame's rebuild. After it, the active tab is fixed.
        self.navigate(ui, open, library);

        // Tabs are settled: drop viewer state for closed tabs.
        self.sync_image_viewers(open);
        self.run_state.pinned_outputs.reconcile(ui, &open.document);
        // `Some` for a graph pane, `None` for a non-graph view (Preferences):
        // the scene projection + canvas edit pipeline run only when a graph
        // tab is active.
        let graph_target = open.document.active_target();

        if let Some(target) = graph_target {
            self.sync_target(target);

            // Rebuild the projection for this frame, after the navigation
            // phase has fully settled the document — so prepass and
            // `CanvasGeometry` never read a stale graph. Unconditional for a
            // graph tab: `Scene` re-interns port names into the active
            // record-pass text arena.
            self.rebuild_scene(ui, open, target, library);
            self.scene_dirty = false;

            // Prepass emits input-derived graph mutations (drag, pan/zoom,
            // connection commit) drained *before* the record so Pass A sees
            // the settled doc. It reads everything off `Scene`.
            self.main_window.prepass(ui, &self.scene, &mut self.intents);
            self.drain_intents(open, target, library);
            self.apply_canvas_shortcuts(ui, open, target);
        }

        let command_from_shortcut = self.menu_shortcut(ui);

        // Record. Rebuild again only if the pre-record drain actually
        // changed the doc (drag, connection commit) — an idle frame or a
        // bare tab switch leaves `scene_dirty` false and skips it.
        if self.scene_dirty {
            if let Some(target) = graph_target {
                self.rebuild_scene(ui, open, target, library);
            }
            self.scene_dirty = false;
        }
        let ctx = AppContext {
            theme,
            library,
            run_state: &self.run_state,
            events_running,
            status_error,
        };
        let command = self
            .main_window
            .frame(
                ui,
                &ctx,
                &self.scene,
                preferences,
                &open.document,
                &mut self.intents,
            )
            .or(command_from_shortcut);

        // A node context-menu pick resolves here, where the Document is
        // available to build the duplicate / removal intents against the
        // live selection (the canvas gesture only sees the read-only Scene).
        // Pushed before the post-record drain so it lands this frame.
        if let Some(target) = graph_target
            && let Some(action) = self.main_window.graph_ui.take_node_menu_action()
        {
            self.apply_node_menu_action(open, action, target);
        }

        // Post-record drain — graph edits the record surfaced (node select,
        // cache toggle, const edit) plus tab-strip renames. Those and the
        // navigation steps are graph-agnostic, so a non-graph active tab
        // drains against `Main` (the target is unused for them).
        self.drain_intents(open, graph_target.unwrap_or(GraphRef::Main), library);

        // Single consumption point for the frame's accumulated relayout
        // signal (edits, tab switch, undo/redo). A menu side effect adds
        // its own relayout in `App`, since it runs after this returns.
        if self.needs_relayout {
            ui.request_relayout();
        }
        command
    }

    /// Resolve a node context-menu pick against `target`'s live selection
    /// (right-click already selected the clicked node). Duplicate variants
    /// reuse the Ctrl+D builder (pinned-output previews carry no node
    /// identity to clone, so they're filtered out); Remove mirrors the
    /// Delete-key path — one intent per selected member, batched into a
    /// single undo entry by the post-record drain.
    fn apply_node_menu_action(
        &mut self,
        open: &OpenDocument,
        action: NodeMenuAction,
        target: GraphRef,
    ) {
        let Some(view) = open.document.view(target) else {
            return;
        };
        match action {
            NodeMenuAction::Duplicate | NodeMenuAction::DuplicateWithIncoming => {
                let incoming = matches!(action, NodeMenuAction::DuplicateWithIncoming);
                let node_ids = selected_node_ids(view);
                if let Some(intent) =
                    build_duplicate_intent_for(&open.document, target, &node_ids, incoming)
                {
                    self.intents.push(intent);
                }
            }
            NodeMenuAction::Remove => {
                self.intents
                    .extend(remove_selection_intents(&view.selected));
            }
        }
    }

    /// Settle which graph is active for this frame, from inputs all
    /// available before the record: keyboard undo/redo (which can replay
    /// a dock-layout change) and tab/graph-open clicks read from
    /// *last* frame's responses.
    ///
    /// Done up front so the edit pipeline runs against a fixed target and
    /// a switched-to graph records in the same present's Pass A.
    fn navigate(&mut self, ui: &mut Ui, open: &mut OpenDocument, library: &Library) {
        self.apply_undo_redo(ui, open);
        // Surface tab/open clicks from last frame's responses. `scene`
        // still holds the last-rendered graph here — exactly the one
        // whose chips were clicked.
        self.main_window
            .scan_navigation(ui, &open.document, &self.scene, &mut self.actions);
        // Open mutates the layout directly; activate/close queue
        // undoable `Intent::Dock` ops — drain them (dock steps are
        // graph-agnostic, so the target passed here doesn't matter).
        self.apply_view_actions(open);
        // The queued intents (switch/close/rename) are graph-agnostic, so
        // a non-graph active tab drains against `Main` harmlessly.
        self.drain_intents(
            open,
            open.document.active_target().unwrap_or(GraphRef::Main),
            library,
        );
        // A closed/deleted target can't be active; fall back to Main.
        open.document.ensure_valid_layout();
    }

    /// Note a possible active-graph change: when `target` differs from
    /// what `scene` last reflected, drop transient gesture state (so a
    /// drag started on one graph can't bleed into another) and request a
    /// relayout. Keeps `CanvasGeometry`'s offset cache, so a graph shown again
    /// resolves its port centers immediately. The rebuild itself is the
    /// caller's unconditional one — this only reacts to the switch.
    fn sync_target(&mut self, target: GraphRef) {
        if self.scene_target == Some(target) {
            return;
        }
        self.main_window.reset_transient();
        self.scene_target = Some(target);
        self.needs_relayout = true;
    }

    /// Rebuild the `Scene` projection from the graph + view the `target`
    /// points at. For a `Local` target, also hands the scene the enclosing
    /// `Graph` so the interior's boundary nodes can mirror its
    /// interface as their ports.
    ///
    /// Reconciles every graph's interface against its interior wiring
    /// first (derived state, like the scene itself) so boundary nodes
    /// render the right ports + placeholder and the doc is consistent
    /// before any save — but only when normalization is pending (a
    /// structural edit, undo/redo, or document replacement since the last
    /// reconcile). Idle/selection/viewport frames skip it: the interface
    /// can't have changed, and reconcile is idempotent there anyway.
    pub(crate) fn rebuild_scene(
        &mut self,
        ui: &mut Ui,
        open: &mut OpenDocument,
        target: GraphRef,
        library: &Library,
    ) {
        open.normalize(library);
        let graph = open
            .document
            .graph_for(target)
            .expect("active tab graph exists");
        let view = open.document.view(target).expect("active tab view exists");
        self.scene
            .rebuild(ui, graph, view, library, &self.run_state);
    }

    /// Drain `intents`, applying each non-no-op intent to `document`,
    /// and push the whole frame's resulting steps onto the undo stack
    /// as a single batch entry — so a gesture that emits N intents
    /// (e.g. breaker swipe deleting K nodes + unbinding M ports) is
    /// one Cmd-Z. Marks the scene dirty when anything applied (so the
    /// pre-record rebuild folds the change in) and accumulates the
    /// relayout / reconcile signals onto the frame's fields.
    fn drain_intents(&mut self, open: &mut OpenDocument, target: GraphRef, library: &Library) {
        // Move the scratch buffer out so it can drive `commit_batch` (which
        // borrows `self` mutably), then put the now-empty buffer back to
        // reuse its allocation next frame.
        let mut scratch = std::mem::take(&mut self.intents);
        // A cache-mode toggle should flush the node's resident value to disk now;
        // flag it for `App` to send a `SaveCaches` after the frame. A `Disabled`
        // toggle (the other `SetNodeProperty`) doesn't touch caching, so it's excluded.
        if scratch.iter().any(|i| {
            matches!(
                i,
                Intent::SetNodeProperty {
                    to: NodeProperty::RuntimeCache(_),
                    ..
                }
            )
        }) {
            self.caches_dirty = true;
        }
        if self.commit_batch(open, target, library, scratch.drain(..)) {
            self.scene_dirty = true;
        }
        self.intents = scratch;
    }

    /// Whether a disk-cache toggle was applied since the last call (clears the flag).
    /// `App` uses this to flush the node's resident value to disk without a re-run.
    pub(crate) fn take_caches_dirty(&mut self) -> bool {
        std::mem::take(&mut self.caches_dirty)
    }

    /// Apply the record pass's view-state requests. Adding a tab to a
    /// strip is the only non-undoable part of opening; the focus change it
    /// implies, plus activate and close, are queued as `Intent::Dock` ops
    /// so they join the undo history (their relayout is decided later,
    /// when they drain).
    fn apply_view_actions(&mut self, open: &mut OpenDocument) {
        for action in std::mem::take(&mut self.actions) {
            match action {
                UiAction::OpenGraph(target) => self.open_graph(open, target),
                UiAction::Dock(op) => self.intents.push(Intent::Dock(op)),
                UiAction::NewGraph => {
                    // Creating the graph + instance isn't undoable (no undo
                    // history references the fresh graph, so the stack stays
                    // valid); `open_graph` still records the focus switch.
                    // Not routed through a step, so flag the edit directly.
                    let id = open.document.create_graph();
                    open.normalization_pending = true;
                    self.dirty = true;
                    self.open_graph(open, GraphRef::Local(id));
                }
                UiAction::OpenImageViewer(port) => self.open_image_viewer(open, port),
            }
        }
    }

    /// Open `port`'s image-viewer tab and focus it — one tab per port,
    /// deduped. Mirrors [`Self::open_preferences`]: adding the tab is the
    /// non-undoable part, focus routes through a recorded activation.
    fn open_image_viewer(&mut self, open: &mut OpenDocument, port: PortRef) {
        assert_eq!(port.kind, PortKind::Output);
        let group = open.document.layout.focused;
        let addr = open
            .document
            .layout
            .find_or_insert(TabRef::ImageViewer(port), group);
        self.push_activate(addr);
    }

    /// Keep the viewer tabs in step with the document by dropping navigation
    /// state whose tab closed.
    fn sync_image_viewers(&mut self, open: &OpenDocument) {
        let layout = &open.document.layout;
        self.main_window
            .image_viewers
            .retain(|port, _| layout.all_tabs().any(|t| t == TabRef::ImageViewer(*port)));
    }

    /// Queue the recorded focus/activation half of an open.
    fn push_activate(&mut self, addr: TabAddress) {
        self.intents.push(activate_intent(addr));
    }

    /// Open `target`'s tab in the primary group (graph tabs are pinned
    /// there) and focus it. Adding the tab to the strip (lazily seeding a
    /// `Local` interior's view) is the non-undoable part; focusing it
    /// routes through a recorded activation like every other focus
    /// change — queued here, drained right after. Undo then faithfully
    /// reverses focus (the opened tab stays open) and a fresh open
    /// discards the redo tail, instead of mutating focus outside the
    /// record.
    fn open_graph(&mut self, open: &mut OpenDocument, target: GraphRef) {
        // Idempotent view seeding, so it can run before the open-or-focus
        // dedupe rather than only inside the "new tab" arm.
        if let GraphRef::Local(id) = target
            && !open.document.ensure_sub_view(id)
        {
            return; // graph vanished — nothing to open
        }
        let group = open.document.layout.primary().id;
        let addr = open
            .document
            .layout
            .find_or_insert(TabRef::Graph(target), group);
        self.push_activate(addr);
    }

    /// Open the [`TabRef::Preferences`] settings tab in the focused group
    /// and focus it (reusing the existing tab wherever it lives). Mirrors
    /// [`open_graph`]: adding the tab is the non-undoable part; the focus
    /// routes through a recorded activation. Called from the File ▸
    /// Preferences menu via `App`, so it records the switch and drains it
    /// immediately like every external edit.
    pub(crate) fn open_preferences(&mut self, open: &mut OpenDocument, library: &Library) {
        let group = open.document.layout.focused;
        let addr = open
            .document
            .layout
            .find_or_insert(TabRef::Preferences, group);
        self.apply_edit(open, activate_intent(addr), library);
    }
}

/// The recorded focus/activation half of opening a tab at `addr`.
fn activate_intent(addr: TabAddress) -> Intent {
    Intent::Dock(DockOp::ActivateTab {
        group: addr.group,
        index: addr.index,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use glam::Vec2;
    use scenarium::DataType;
    use scenarium::{Binding, Func, FuncId, FuncInput, FuncOutput};
    use scenarium::{Graph, InputPort, Library, Node, NodeKind};

    use crate::core::document::open_document::OpenDocument;
    use crate::core::document::{Document, GraphRef, ItemRef, PortKind, PortRef, TabRef};
    use crate::core::edit::intent::types::Intent;
    use crate::gui::UiAction;
    use crate::gui::app::editor::Editor;
    use crate::gui::image_viewer::ImageViewer;

    #[derive(Debug)]
    struct TestEditor {
        editor: Editor,
        open: OpenDocument,
    }

    impl TestEditor {
        fn new(document: Document) -> Self {
            Self {
                editor: Editor::new(),
                open: OpenDocument {
                    document,
                    path: None,
                    normalization_pending: true,
                },
            }
        }

        fn open_graph(&mut self, target: GraphRef) {
            self.editor.open_graph(&mut self.open, target);
            self.editor
                .drain_intents(&mut self.open, GraphRef::Main, &Library::default());
        }

        fn undo(&mut self) -> bool {
            self.editor
                .action_stack
                .undo(&mut self.open.document, &mut |_| {})
        }

        fn redo(&mut self) -> bool {
            self.editor
                .action_stack
                .redo(&mut self.open.document, &mut |_| {})
        }
    }

    #[test]
    fn dirty_flag_tracks_content_edits_not_navigation() {
        let lib = Library::default();
        let mut test = TestEditor::new(Document::default());
        assert!(!test.editor.dirty, "a freshly opened document is clean");

        // Seed a node, then rename it — a content edit must dirty.
        let node = Node::new(NodeKind::Func(FuncId::unique()));
        let id = test.open.document.graph.add(node);
        test.open
            .document
            .main_view
            .item_placements
            .insert(ItemRef::Node(id), Vec2::ZERO);
        test.editor.apply_edit(
            &mut test.open,
            Intent::RenameNode {
                node_id: id,
                to: "renamed".into(),
            },
            &lib,
        );
        assert!(test.editor.dirty, "renaming a node is unsaved work");

        // Clear (as a save would), then a pure selection change: applied,
        // but navigation — it must not mark the document dirty again.
        test.editor.dirty = false;
        test.editor.apply_edit(
            &mut test.open,
            Intent::SetSelection {
                to: BTreeSet::from([ItemRef::Node(id)]),
            },
            &lib,
        );
        assert_eq!(
            test.open.document.main_view.selected,
            BTreeSet::from([ItemRef::Node(id)]),
            "the selection edit did apply",
        );
        assert!(
            !test.editor.dirty,
            "selecting a node must not dirty the document"
        );

        // Creating a graph takes the direct (non-undoable) path, which
        // must still flag the edit.
        test.editor.actions.push(UiAction::NewGraph);
        test.editor.apply_view_actions(&mut test.open);
        assert!(test.editor.dirty, "creating a graph is unsaved work");
    }

    #[test]
    fn image_viewer_tabs_dedupe_per_port_and_prune_state_on_close() {
        let lib = Library::default();
        let mut test = TestEditor::new(Document::default());
        let node = Node::new(NodeKind::Func(FuncId::unique()));
        let id = test.open.document.graph.add(node);
        test.open
            .document
            .main_view
            .item_placements
            .insert(ItemRef::Node(id), Vec2::ZERO);
        let port = |port_idx| PortRef {
            node_id: id,
            kind: PortKind::Output,
            port_idx,
        };
        let open = |test: &mut TestEditor, p| {
            test.editor.actions.push(UiAction::OpenImageViewer(p));
            test.editor.apply_view_actions(&mut test.open);
            test.editor
                .drain_intents(&mut test.open, GraphRef::Main, &lib);
        };
        let tabs = |test: &TestEditor| test.open.document.layout.all_tabs().collect::<Vec<_>>();
        let active = |test: &TestEditor| test.open.document.layout.primary().active;

        // Opening only changes the layout. Viewer state is created by the
        // renderer when it draws the tab and pulls from `RunState`.
        open(&mut test, port(0));
        assert_eq!(
            tabs(&test),
            vec![TabRef::Graph(GraphRef::Main), TabRef::ImageViewer(port(0))]
        );
        assert_eq!(active(&test), 1);
        assert!(
            test.editor.main_window.image_viewers.is_empty(),
            "the editor does not create or populate view state"
        );
        test.editor
            .main_window
            .image_viewers
            .insert(port(0), ImageViewer::new(port(0)));

        // Re-clicking the same port reuses its tab; a different port gets
        // its own renderer-owned state.
        open(&mut test, port(0));
        assert_eq!(tabs(&test).len(), 2, "same port dedupes");
        open(&mut test, port(1));
        assert_eq!(tabs(&test).len(), 3, "distinct port adds a tab");
        assert_eq!(active(&test), 2);
        test.editor
            .main_window
            .image_viewers
            .insert(port(1), ImageViewer::new(port(1)));
        assert!(test.editor.main_window.image_viewers.contains_key(&port(0)));
        assert!(test.editor.main_window.image_viewers.contains_key(&port(1)));

        test.editor.sync_image_viewers(&test.open);

        // Closing a tab drops its viewer state on the next sync (the
        // remaining port's state survives).
        test.open
            .document
            .layout
            .retain_tabs(|t| t != TabRef::ImageViewer(port(1)));
        test.open.document.ensure_valid_layout();
        test.editor.sync_image_viewers(&test.open);
        assert!(test.editor.main_window.image_viewers.contains_key(&port(0)));
        assert!(
            !test.editor.main_window.image_viewers.contains_key(&port(1)),
            "closed tab's viewer state is pruned"
        );
    }

    #[test]
    fn opening_a_graph_records_an_undoable_focus_switch() {
        let mut test = TestEditor::new(Document::default());
        let a = test.open.document.create_graph();
        let b = test.open.document.create_graph();
        let tabs = |test: &TestEditor| test.open.document.layout.all_tabs().collect::<Vec<_>>();
        let active = |test: &TestEditor| test.open.document.layout.primary().active;

        // Opening appends the tab and focuses it through a recorded
        // activation.
        test.open_graph(GraphRef::Local(a));
        assert_eq!(
            tabs(&test),
            vec![
                TabRef::Graph(GraphRef::Main),
                TabRef::Graph(GraphRef::Local(a))
            ]
        );
        assert_eq!(active(&test), 1);

        // Undo reverses the focus only — the opened tab stays in the strip
        // (adding it isn't undoable), and `active` returns to its real prior
        // value rather than a stale stored index.
        assert!(test.undo());
        assert_eq!(active(&test), 0, "undo restores the prior focus");
        assert_eq!(
            tabs(&test).len(),
            2,
            "undo reverses focus, not the tab open"
        );

        // A fresh open after that undo is a new action: it discards the
        // redoable tail instead of leaving it replayable on a stale `active`.
        test.open_graph(GraphRef::Local(b));
        assert_eq!(active(&test), 2);
        assert!(
            !test.redo(),
            "the undone switch is unreachable after a fresh open"
        );
        assert_eq!(active(&test), 2);

        // Re-focusing an already-open tab also routes through a recorded
        // activation: `active` follows and no second tab is added.
        test.open_graph(GraphRef::Local(a));
        assert_eq!(active(&test), 1, "re-focus moves active");
        assert_eq!(tabs(&test).len(), 3, "re-focusing an open tab adds none");
    }

    #[test]
    fn undo_of_a_passthrough_rewire_restores_the_severed_edge() {
        let float_src =
            Func::new(FuncId::unique(), "fsrc").output(FuncOutput::new("o", DataType::Float));
        let string_src =
            Func::new(FuncId::unique(), "ssrc").output(FuncOutput::new("o", DataType::String));
        let float_sink = Func::new(FuncId::unique(), "fsink")
            .input(FuncInput::required("x", DataType::Float))
            .output(FuncOutput::new("o", DataType::Float));
        let pass_func = Func::new(FuncId::unique(), "pass")
            .input(FuncInput::required("x", DataType::Any))
            .wildcard_output("o", 0);
        let library = Library::from([
            float_src.clone(),
            string_src.clone(),
            float_sink.clone(),
            pass_func.clone(),
        ]);

        let mut graph = Graph::default();
        let fp = graph.add_func_node(&float_src);
        let sp = graph.add_func_node(&string_src);
        let pass = graph.add_func_node(&pass_func);
        let sink = graph.add_func_node(&float_sink);
        graph.set_input_binding(InputPort::new(pass, 0), Binding::bind(fp, 0));
        graph.set_input_binding(InputPort::new(sink, 0), Binding::bind(pass, 0));

        let mut test = TestEditor::new(graph.into());

        // Rewire the passthrough input to the String producer: the cascade
        // severs the now-incompatible Float sink edge, in one undo batch.
        test.editor.apply_edit(
            &mut test.open,
            Intent::SetInput {
                input: InputPort::new(pass, 0),
                to: Some(Binding::bind(sp, 0)),
            },
            &library,
        );
        assert_eq!(
            test.open
                .document
                .graph
                .bindings
                .get(&InputPort::new(sink, 0)),
            None,
            "the incompatible sink edge is severed"
        );

        // Undo reverts the whole batch — the rewire *and* the sever — so the
        // graph returns to the original valid Float → passthrough → Float-sink
        // rather than leaving the severed edge dropped.
        assert!(test.undo());
        assert_eq!(
            test.open
                .document
                .graph
                .bindings
                .get(&InputPort::new(pass, 0)),
            Some(&Binding::bind(fp, 0)),
            "the input rewire is undone"
        );
        assert_eq!(
            test.open
                .document
                .graph
                .bindings
                .get(&InputPort::new(sink, 0)),
            Some(&Binding::bind(pass, 0)),
            "the severed edge is restored, not left dangling"
        );
    }
}
