//! The per-frame edit pipeline over a [`Document`].
//!
//! `Editor` owns the document being edited, its undo history, the derived
//! [`Scene`] projection (plus the per-run status/log projections), the GUI
//! tree state, and the transient frame-coordination scratch. [`App`] is a
//! thin shell around it: it owns the runtime/IO (func lib, theme, preferences,
//! file path, worker, host handle), drains the worker into the editor's
//! projections, runs one `Editor::frame`, and handles the [`AppCommand`]
//! that frame surfaces. Everything the GUI tree reads or mutates lives
//! here; nothing in `Editor` knows about file dialogs or the worker.
//!
//! [`App`]: crate::gui::app::App

use palantir::Ui;
use scenarium::graph::NodeId;
use scenarium::graph::subgraph::SubgraphDef;
use scenarium::library::Library;

use crate::core::document::{Document, GraphRef, TabRef};
use crate::core::edit::action_stack::ActionStack;
use crate::core::edit::intent::{Intent, build_duplicate_intent_for, commit_intent_cascading};
use crate::core::io::preferences::Preferences;
use crate::core::worker::ValueRequest;
use crate::gui::UiAction;
use crate::gui::app::AppCommand;
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
    pub(crate) document: Document,
    /// Unsaved-changes flag: set whenever a content-changing step is
    /// applied (via [`Document`]'s edit paths — new edits, undo/redo
    /// replay, and the direct subgraph mutations), cleared on save. Only
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
    /// Set when an applied/undone step can change a subgraph's derived
    /// interface (see `requires_reconcile`); consumed by `rebuild_scene`,
    /// which reruns `reconcile_boundaries` only then. Derived state is
    /// recomputed on structural edits, not on every frame's projection
    /// rebuild — idle/selection/viewport frames skip the per-def edge
    /// scan. Starts `true` so the first rebuild canonicalizes a freshly
    /// loaded (or hand-edited) document.
    needs_reconcile: bool,
    /// Per-frame accumulator: set by any step/transition that changes
    /// something the layout engine reads (see `requires_relayout`), and
    /// consumed once at the end of `frame` as a single
    /// `ui.request_relayout()`. Reset at the top of every frame. A plain
    /// side-effect field like `scene_dirty` / `needs_reconcile`, rather
    /// than a `bool` threaded back through every helper's return.
    needs_relayout: bool,
    /// Set when a disk-cache toggle (`Intent::SetPersist`) is applied this frame.
    /// `App` consumes it via [`Self::take_caches_dirty`] to flush the node's resident
    /// value to disk (a `SaveCaches` to the worker) without a re-run.
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
    /// `NodeId`s so it resolves against any tab (root or subgraph
    /// interior): execution status (the glow + header time, projected into
    /// each `SceneNode::exec_status` at rebuild), log lines, and the
    /// runtime values fetched on demand for open inspection panels. `App`
    /// drives it as it drains the worker (`RunState::set_results` /
    /// `ingest_values` / `clear`); requests go through `take_value_requests`
    /// here, which also reads the open-panel set. Off the serialized state.
    pub(crate) run_state: RunState,
}

impl Editor {
    /// Build the editor around a starting `document`. Derived state
    /// (scene, status/log projections) starts empty and `needs_reconcile`
    /// starts `true` so the first frame canonicalizes the document.
    pub(crate) fn new(document: Document) -> Self {
        Self {
            document,
            dirty: false,
            action_stack: ActionStack::new(UNDO_HISTORY_BYTES),
            scene: Scene::default(),
            main_window: MainWindow::default(),
            scene_target: None,
            scene_dirty: false,
            needs_reconcile: true,
            needs_relayout: false,
            caches_dirty: false,
            intents: Vec::new(),
            actions: Vec::new(),
            run_state: RunState::default(),
        }
    }

    /// Pending value requests for the open inspection panels: each open
    /// node that has no value yet and no request in flight, marked
    /// in-flight here and returned (with the current run epoch) for `App`
    /// to forward to the worker. Keeps all request bookkeeping on the
    /// projection; `App` only performs the IO.
    pub(crate) fn take_value_requests(&mut self) -> Vec<ValueRequest> {
        self.run_state
            .take_requests(self.main_window.graph_ui.open_inspector_nodes())
    }

    /// Apply a single `intent` against the active target and record it as
    /// its own undo entry. For edits raised *outside* the frame's intent
    /// drain — e.g. a file-picker result `App` handles after the record.
    /// No-ops (and self-cancelling steps) are dropped, like the in-frame
    /// drain.
    pub(crate) fn apply_edit(&mut self, intent: Intent, library: &Library) {
        let target = self.document.active_target().unwrap_or(GraphRef::Main);
        self.commit_batch(target, library, [intent]);
    }

    /// Apply a batch of externally-sourced `intents` (e.g. from a script)
    /// against the active target as a single undo entry — the multi-intent
    /// analogue of [`Self::apply_edit`]. Used by `App` when draining the
    /// script inbound queue before the frame; the unconditional pre-prepass
    /// rebuild folds the edits in, so `scene_dirty` needn't be set here.
    pub(crate) fn apply_external_intents(&mut self, intents: Vec<Intent>, library: &Library) {
        let target = self.document.active_target().unwrap_or(GraphRef::Main);
        self.commit_batch(target, library, intents);
    }

    /// Build, apply, and record `intents` against `target` as one undo
    /// entry, accumulating the frame's relayout/reconcile signals. Returns
    /// whether anything applied. Shared core of [`Self::apply_edit`],
    /// [`Self::apply_external_intents`], and [`Self::drain_intents`]: no-op
    /// and stale intents (anchor node already gone) are dropped per-intent,
    /// and an empty batch records nothing.
    fn commit_batch(
        &mut self,
        target: GraphRef,
        library: &Library,
        intents: impl IntoIterator<Item = Intent>,
    ) -> bool {
        let mut batch = Vec::new();
        for intent in intents {
            // A `SetInput` that retypes a wildcard output cascades into dropping
            // the now-incompatible downstream wires — all one undo entry.
            for step in commit_intent_cascading(intent, &mut self.document, target, library) {
                self.needs_relayout |= step.requires_relayout();
                self.needs_reconcile |= step.requires_reconcile();
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

    /// Add an imported subgraph def to the document, flagging the reconcile
    /// the import needs: an imported def's stored interface may not match
    /// its interior wiring (hand-edited / older file), so it's re-derived
    /// on the next rebuild. Keeps the "import ⇒ reconcile" invariant here
    /// rather than on the caller.
    pub(crate) fn import_subgraph(&mut self, def: SubgraphDef) {
        self.document.import_subgraph(def);
        self.needs_reconcile = true;
        self.dirty = true;
    }

    /// Run one frame of the edit pipeline against the borrowed runtime
    /// context (`library`, `theme`, `host`), returning the [`AppCommand`]
    /// the frame surfaced (if any) for `App` to action outside the record.
    ///
    /// The frame splits into a **navigation phase** (settle *which* graph
    /// is active, from frame-top inputs) and an **edit phase** (mutate that
    /// graph), because input that switches tabs/opens subgraphs comes from
    /// *last* frame's click responses and must resolve before anything
    /// edits or records.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn frame(
        &mut self,
        ui: &mut Ui,
        library: &Library,
        theme: &Theme,
        preferences: &mut Preferences,
        events_running: bool,
    ) -> Option<AppCommand> {
        self.intents.clear();
        self.actions.clear();
        self.needs_relayout = false;

        // ── Navigation phase ─────────────────────────────────────────
        // Settle the active tab entirely from frame-top inputs (keyboard
        // undo/redo + last-frame click responses). `navigate` reads *last*
        // frame's `scene` to resolve tab/chip clicks, so it must run before
        // this frame's rebuild. After it, the active tab is fixed.
        self.navigate(ui, library);
        // `Some` for a graph pane, `None` for a non-graph view (Preferences):
        // the scene projection + canvas edit pipeline run only when a graph
        // tab is active.
        let graph_target = self.document.active_target();

        if let Some(target) = graph_target {
            self.sync_target(target);

            // Rebuild the projection for this frame, after the navigation
            // phase has fully settled the document — so prepass and
            // `PortFrame` never read a stale graph. Unconditional for a
            // graph tab: `Scene` re-interns port names into palantir's
            // per-frame text arena (cleared each `Ui::frame`).
            self.rebuild_scene(target, library);
            self.scene_dirty = false;

            // ── Edit phase ───────────────────────────────────────────
            // Prepass emits input-derived graph mutations (drag, pan/zoom,
            // connection commit) drained *before* the record so Pass A sees
            // the settled doc. It reads everything off `Scene`.
            self.main_window.prepass(ui, &self.scene, &mut self.intents);
            self.drain_intents(target, library);
            self.apply_canvas_shortcuts(ui, target);
        }

        let command_from_shortcut = self.menu_shortcut(ui);

        // Record. Rebuild again only if the pre-record drain actually
        // changed the doc (drag, connection commit) — an idle frame or a
        // bare tab switch leaves `scene_dirty` false and skips it.
        if self.scene_dirty {
            if let Some(target) = graph_target {
                self.rebuild_scene(target, library);
            }
            self.scene_dirty = false;
        }
        let ctx = AppContext {
            theme,
            library,
            run_state: &self.run_state,
            events_running,
        };
        let command = self
            .main_window
            .frame(
                ui,
                &ctx,
                &self.scene,
                preferences,
                &self.document,
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
            self.apply_node_menu_action(action, target);
        }

        // Post-record drain — graph edits the record surfaced (node select,
        // cache toggle, const edit) plus tab-strip renames. Those and the
        // navigation steps are graph-agnostic, so a non-graph active tab
        // drains against `Main` (the target is unused for them).
        self.drain_intents(graph_target.unwrap_or(GraphRef::Main), library);

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
    /// reuse the Ctrl+D builder; Remove mirrors the Delete-key path —
    /// one `RemoveNode` per node, batched into a single undo entry by the
    /// post-record drain.
    fn apply_node_menu_action(&mut self, action: NodeMenuAction, target: GraphRef) {
        let Some(view) = self.document.view(target) else {
            return;
        };
        match action {
            NodeMenuAction::Duplicate | NodeMenuAction::DuplicateWithIncoming => {
                let incoming = matches!(action, NodeMenuAction::DuplicateWithIncoming);
                if let Some(intent) = build_duplicate_intent_for(
                    &self.document,
                    target,
                    &view.selected_nodes,
                    incoming,
                ) {
                    self.intents.push(intent);
                }
            }
            NodeMenuAction::Remove => {
                let ids: Vec<NodeId> = view.selected_nodes.iter().copied().collect();
                for node_id in ids {
                    self.intents.push(Intent::RemoveNode { node_id });
                }
            }
        }
    }

    /// Settle which graph is active for this frame, from inputs all
    /// available before the record: keyboard undo/redo (which can replay
    /// a `SwitchTab`) and tab/subgraph-open clicks read from *last*
    /// frame's responses.
    ///
    /// Done up front so the edit pipeline runs against a fixed target and
    /// a switched-to graph records in the same present's Pass A.
    fn navigate(&mut self, ui: &mut Ui, library: &Library) {
        self.apply_undo_redo(ui);
        // Surface tab/open clicks from last frame's responses. `scene`
        // still holds the last-rendered graph here — exactly the one
        // whose chips were clicked.
        self.main_window
            .scan_navigation(ui, &self.document, &self.scene, &mut self.actions);
        // Open mutates the tab list directly; activate/close queue
        // undoable `SwitchTab` / `CloseTab` intents — drain them (both
        // steps are graph-agnostic, so the target passed here doesn't
        // matter).
        self.apply_view_actions();
        // The queued intents (switch/close/rename) are graph-agnostic, so
        // a non-graph active tab drains against `Main` harmlessly.
        self.drain_intents(
            self.document.active_target().unwrap_or(GraphRef::Main),
            library,
        );
        // A closed/deleted target can't be active; fall back to Main.
        self.document.ensure_valid_active();
    }

    /// Note a possible active-graph change: when `target` differs from
    /// what `scene` last reflected, drop transient gesture state (so a
    /// drag started on one graph can't bleed into another) and request a
    /// relayout. Keeps `PortFrame`'s offset cache, so a graph shown again
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
    /// `SubgraphDef` so the interior's boundary nodes can mirror its
    /// interface as their ports.
    ///
    /// Reconciles every subgraph's interface against its interior wiring
    /// first (derived state, like the scene itself) so boundary nodes
    /// render the right ports + placeholder and the doc is consistent
    /// before any save — but only when `needs_reconcile` is set (a
    /// structural edit, undo/redo, or document replacement since the last
    /// reconcile). Idle/selection/viewport frames skip it: the interface
    /// can't have changed, and reconcile is idempotent there anyway.
    pub(crate) fn rebuild_scene(&mut self, target: GraphRef, library: &Library) {
        if self.needs_reconcile {
            self.document.reconcile_boundaries(library);
            // Same pass: drop bindings/subscriptions left dangling by a library
            // skew on load (an input/output/event the node no longer exposes),
            // or by a removed endpoint. Runs before the rebuild below, so the
            // stale wire never projects.
            self.document.prune_dangling_wiring(library);
            self.needs_reconcile = false;
        }
        let graph = self
            .document
            .graph_for(target)
            .expect("active tab graph exists");
        let view = self.document.view(target).expect("active tab view exists");
        let ctx_def = match target {
            GraphRef::Main => None,
            GraphRef::Local(id) => self.document.graph.subgraphs.by_key(&id),
        };
        self.scene
            .rebuild(graph, view, library, ctx_def, &self.run_state);
    }

    /// Drain `intents`, applying each non-no-op intent to `document`,
    /// and push the whole frame's resulting steps onto the undo stack
    /// as a single batch entry — so a gesture that emits N intents
    /// (e.g. breaker swipe deleting K nodes + unbinding M ports) is
    /// one Cmd-Z. Marks the scene dirty when anything applied (so the
    /// pre-record rebuild folds the change in) and accumulates the
    /// relayout / reconcile signals onto the frame's fields.
    fn drain_intents(&mut self, target: GraphRef, library: &Library) {
        // Move the scratch buffer out so it can drive `commit_batch` (which
        // borrows `self` mutably), then put the now-empty buffer back to
        // reuse its allocation next frame.
        let mut scratch = std::mem::take(&mut self.intents);
        // A disk-cache toggle should flush the node's resident value to disk now;
        // flag it for `App` to send a `SaveCaches` after the frame.
        if scratch
            .iter()
            .any(|i| matches!(i, Intent::SetPersist { .. }))
        {
            self.caches_dirty = true;
        }
        if self.commit_batch(target, library, scratch.drain(..)) {
            self.scene_dirty = true;
        }
        self.intents = scratch;
    }

    /// Whether a disk-cache toggle was applied since the last call (clears the flag).
    /// `App` uses this to flush the node's resident value to disk without a re-run.
    pub(crate) fn take_caches_dirty(&mut self) -> bool {
        std::mem::take(&mut self.caches_dirty)
    }

    /// Apply the record pass's view-state requests. Adding a tab to the
    /// strip is the only non-undoable part of opening; the focus change it
    /// implies, plus activate and close, are queued as `Intent::SwitchTab` /
    /// `Intent::CloseTab` so they join the undo history (their relayout is
    /// decided later, when they drain).
    fn apply_view_actions(&mut self) {
        for action in std::mem::take(&mut self.actions) {
            match action {
                UiAction::OpenGraph(target) => self.open_graph(target),
                UiAction::ActivateTab(index) => {
                    self.intents.push(Intent::SwitchTab { to: index });
                }
                UiAction::CloseTab(index) => {
                    self.intents.push(Intent::CloseTab { index });
                }
                UiAction::NewSubgraph => {
                    // Creating the def + instance isn't undoable (no undo
                    // history references the fresh def, so the stack stays
                    // valid); `open_graph` still records the focus switch.
                    // Not routed through a step, so flag the edit directly.
                    let id = self.document.create_subgraph();
                    self.dirty = true;
                    self.open_graph(GraphRef::Local(id));
                }
            }
        }
    }

    /// Open `target`'s tab and focus it. Adding the tab to the strip (lazily
    /// seeding a `Local` interior's view) is the non-undoable part; focusing
    /// it routes through a recorded `SwitchTab` like every other focus
    /// change — queued here, drained right after. Undo then faithfully
    /// reverses focus (the opened tab stays open) and a fresh open discards
    /// the redo tail, instead of leaving `active` mutated outside the record.
    fn open_graph(&mut self, target: GraphRef) {
        let tab = TabRef::Graph(target);
        let index = match self.document.tabs.iter().position(|t| *t == tab) {
            Some(index) => index,
            None => {
                if let GraphRef::Local(id) = target
                    && !self.document.ensure_sub_view(id)
                {
                    return; // subgraph vanished — nothing to open
                }
                self.document.tabs.push(tab);
                self.document.tabs.len() - 1
            }
        };
        self.intents.push(Intent::SwitchTab { to: index });
    }

    /// Open the [`TabRef::Preferences`] settings tab and focus it (reusing the
    /// existing tab if already open). Mirrors [`open_graph`]: adding the
    /// tab is the non-undoable part; the focus routes through a recorded
    /// `SwitchTab`. Called from the File ▸ Preferences menu via `App`, so it
    /// records the switch and drains it immediately like every external edit.
    pub(crate) fn open_preferences(&mut self, library: &Library) {
        let index = match self
            .document
            .tabs
            .iter()
            .position(|t| *t == TabRef::Preferences)
        {
            Some(index) => index,
            None => {
                self.document.tabs.push(TabRef::Preferences);
                self.document.tabs.len() - 1
            }
        };
        self.apply_edit(Intent::SwitchTab { to: index }, library);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run an `OpenGraph` exactly as `navigate` does: queue the focus
    /// switch, then drain it onto the undo stack.
    fn open(editor: &mut Editor, target: GraphRef) {
        editor.open_graph(target);
        // `SwitchTab` is graph-agnostic, so the drain target is irrelevant.
        editor.drain_intents(GraphRef::Main, &Library::default());
    }

    fn undo(editor: &mut Editor) -> bool {
        editor.action_stack.undo(&mut editor.document, &mut |_| {})
    }

    fn redo(editor: &mut Editor) -> bool {
        editor.action_stack.redo(&mut editor.document, &mut |_| {})
    }

    #[test]
    fn dirty_flag_tracks_content_edits_not_navigation() {
        use std::collections::BTreeSet;

        use glam::Vec2;
        use scenarium::graph::{Node, NodeKind};
        use scenarium::node::function::FuncId;

        use crate::core::document::view_node::ViewNode;

        let lib = Library::default();
        let mut editor = Editor::new(Document::default());
        assert!(!editor.dirty, "a freshly opened document is clean");

        // Seed a node, then rename it — a content edit must dirty.
        let node = Node::new(NodeKind::Func(FuncId::unique()));
        let id = node.id;
        editor.document.graph.add(node);
        editor.document.main_view.view_nodes.add(ViewNode {
            id,
            pos: Vec2::ZERO,
        });
        editor.apply_edit(
            Intent::RenameNode {
                node_id: id,
                to: "renamed".into(),
            },
            &lib,
        );
        assert!(editor.dirty, "renaming a node is unsaved work");

        // Clear (as a save would), then a pure selection change: applied,
        // but navigation — it must not mark the document dirty again.
        editor.dirty = false;
        editor.apply_edit(
            Intent::SetSelection {
                to: BTreeSet::from([id]),
            },
            &lib,
        );
        assert_eq!(
            editor.document.main_view.selected_nodes,
            BTreeSet::from([id]),
            "the selection edit did apply",
        );
        assert!(
            !editor.dirty,
            "selecting a node must not dirty the document"
        );

        // Creating a subgraph takes the direct (non-undoable) path, which
        // must still flag the edit.
        editor.actions.push(UiAction::NewSubgraph);
        editor.apply_view_actions();
        assert!(editor.dirty, "creating a subgraph is unsaved work");
    }

    #[test]
    fn opening_a_graph_records_an_undoable_focus_switch() {
        let mut editor = Editor::new(Document::default());
        let a = editor.document.create_subgraph();
        let b = editor.document.create_subgraph();

        // Opening appends the tab and focuses it through a recorded switch.
        open(&mut editor, GraphRef::Local(a));
        assert_eq!(
            editor.document.tabs,
            vec![
                TabRef::Graph(GraphRef::Main),
                TabRef::Graph(GraphRef::Local(a))
            ]
        );
        assert_eq!(editor.document.active, 1);

        // Undo reverses the focus only — the opened tab stays in the strip
        // (adding it isn't undoable), and `active` returns to its real prior
        // value rather than a stale stored index.
        assert!(undo(&mut editor));
        assert_eq!(editor.document.active, 0, "undo restores the prior focus");
        assert_eq!(
            editor.document.tabs.len(),
            2,
            "undo reverses focus, not the tab open"
        );

        // A fresh open after that undo is a new action: it discards the
        // redoable tail instead of leaving it replayable on a stale `active`.
        open(&mut editor, GraphRef::Local(b));
        assert_eq!(editor.document.active, 2);
        assert!(
            !redo(&mut editor),
            "the undone switch is unreachable after a fresh open"
        );
        assert_eq!(editor.document.active, 2);

        // Re-focusing an already-open tab also routes through `SwitchTab`:
        // `active` follows and no second tab is added.
        open(&mut editor, GraphRef::Local(a));
        assert_eq!(editor.document.active, 1, "re-focus moves active");
        assert_eq!(
            editor.document.tabs.len(),
            3,
            "re-focusing an open tab adds none"
        );
    }

    #[test]
    fn undo_of_a_passthrough_rewire_restores_the_severed_edge() {
        use scenarium::data::DataType;
        use scenarium::graph::{Binding, Graph, InputPort, Node, NodeKind};
        use scenarium::library::Library;
        use scenarium::node::function::{Func, FuncId, FuncInput};
        use scenarium::node::special::SpecialNode;

        let float_src = Func::new(FuncId::unique(), "fsrc").output("o", DataType::Float);
        let string_src = Func::new(FuncId::unique(), "ssrc").output("o", DataType::String);
        let float_sink = Func::new(FuncId::unique(), "fsink")
            .input(FuncInput::required("x", DataType::Float))
            .output("o", DataType::Float);
        let library = Library::from([float_src.clone(), string_src.clone(), float_sink.clone()]);

        let mut graph = Graph::default();
        let fp = graph.add_func_node(&float_src);
        let sp = graph.add_func_node(&string_src);
        let pass = {
            let node = Node::new(NodeKind::Special(SpecialNode::CachePassthrough {
                bypass: false,
            }));
            let id = node.id;
            graph.add(node);
            id
        };
        let sink = graph.add_func_node(&float_sink);
        graph.set_input_binding(InputPort::new(pass, 0), Binding::bind(fp, 0));
        graph.set_input_binding(InputPort::new(sink, 0), Binding::bind(pass, 0));

        let mut editor = Editor::new(graph.into());

        // Rewire the passthrough input to the String producer: the cascade
        // severs the now-incompatible Float sink edge, in one undo batch.
        editor.apply_edit(
            Intent::SetInput {
                node_id: pass,
                input_idx: 0,
                to: Binding::bind(sp, 0),
            },
            &library,
        );
        assert_eq!(
            editor.document.graph.input_binding(InputPort::new(sink, 0)),
            Binding::None,
            "the incompatible sink edge is severed"
        );

        // Undo reverts the whole batch — the rewire *and* the sever — so the
        // graph returns to the original valid Float → passthrough → Float-sink
        // rather than leaving the severed edge dropped.
        assert!(undo(&mut editor));
        assert_eq!(
            editor.document.graph.input_binding(InputPort::new(pass, 0)),
            Binding::bind(fp, 0),
            "the input rewire is undone"
        );
        assert_eq!(
            editor.document.graph.input_binding(InputPort::new(sink, 0)),
            Binding::bind(pass, 0),
            "the severed edge is restored, not left dangling"
        );
    }
}
