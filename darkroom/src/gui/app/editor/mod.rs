//! The per-frame GUI edit pipeline over a borrowed [`OpenDocument`].
//!
//! `Editor` owns undo history, the derived [`Scene`] and run projections,
//! the GUI tree, and transient gesture state. [`App`] lends it the workspace's
//! document for each operation, keeping document/runtime ownership shared with
//! terminal frontends without giving terminal sessions GUI responsibilities.
//!
//! [`App`]: crate::gui::app::App

use palantir::Ui;
use scenarium::Library;
use scenarium::NodeId;

use crate::core::document::dock::DockOp;
use crate::core::document::open_document::OpenDocument;
use crate::core::document::{GraphRef, TabRef};
use crate::core::edit::action_stack::ActionStack;
use crate::core::edit::intent::apply::{commit_doc_intent, commit_intent};
use crate::core::edit::intent::duplicate::{
    build_duplicate_intent_for, remove_selection_intents, selected_node_ids,
};
use crate::core::edit::intent::sink::{Intents, Queued};
use crate::core::edit::intent::types::{BatchScope, DocIntent, Intent, Refusal, UndoStep};
use crate::core::io::preferences::Preferences;
use crate::gui::UiAction;
use crate::gui::app::commands::AppCommand;
use crate::gui::canvas::node_menu::NodeMenuAction;
use crate::gui::main_window::MainWindow;
use crate::gui::run_state::RunState;
use crate::gui::scene::{Frame, GraphProjection, Scene, SceneSource};
use crate::gui::theme::Theme;

use crate::gui::app::{AppContext, StatusInputs};

#[cfg(test)]
pub(crate) mod harness;

/// Keyboard chords → intents/commands. A child module so it can drive the
/// pipeline through `Editor`'s private fields (undo stack, intent buffer,
/// dirty flags) without widening their visibility.
mod shortcuts;

/// Byte budget for the undo history's packed buffer (~1 MiB). Bounds
/// memory rather than entry count — a single large edit can't be
/// undone away, but the oldest entries drop once the buffer overflows.
const UNDO_HISTORY_BYTES: usize = 1 << 20;

/// What one `Editor::commit_queued` did: whether anything applied, plus the
/// reason for each intent the edit layer refused as malformed.
///
/// Only [`Editor::commit_queued`] builds one, and only its two trust-split
/// wrappers read it — [`Editor::commit_widget_batch`], which asserts
/// `rejected` is empty, and [`Editor::commit_script_batch`], which hands
/// it back to the script that asked.
#[derive(Debug)]
struct BatchOutcome {
    applied: bool,
    rejected: Vec<String>,
}

/// What applying one or more [`UndoStep`]s obliges the frame to do, folded
/// off the steps' own predicates. Accumulated as a value rather than written
/// straight onto [`Editor`] because undo/redo replay folds its steps from a
/// callback while `action_stack` itself is mutably borrowed — so both the
/// commit path and the replay path fold here and hand the result to
/// [`Editor::absorb_signals`], and a fourth signal is added in one place.
#[derive(Default, Debug)]
struct StepSignals {
    geometry_stale: bool,
    dirtied: bool,
    reconcile: bool,
}

impl StepSignals {
    fn fold(&mut self, step: &UndoStep) {
        self.geometry_stale |= step.invalidates_cached_geometry();
        self.dirtied |= step.dirties_document();
        self.reconcile |= step.requires_reconcile();
    }
}

#[derive(Debug)]
pub(crate) struct Editor {
    /// Unsaved-changes flag: set whenever a content-changing step is
    /// applied (via [`Document`](crate::core::document::Document)'s edit paths — new edits, undo/redo
    /// replay, and the direct graph mutations), cleared on save. Only
    /// steps that alter saved content flip it — pure navigation (camera,
    /// selection, tab focus) doesn't (see [`UndoStep::dirties_document`]).
    /// Read by `App` on exit to decide whether to prompt. It can read
    /// "dirty" after an undo returns the document to its saved state — the
    /// safe direction (prompt rather than silently discard).
    pub(super) dirty: bool,
    action_stack: ActionStack,
    scene: Scene,
    main_window: MainWindow,
    /// Which graphs `scene` last reflected, in pane order. A mismatch
    /// with this frame's visible set means a tab was switched, opened, or
    /// closed: drop transient gesture state (`reset_transient`) and
    /// request a relayout. Empty forces that on the first frame (a fresh
    /// `Editor`). It does *not* gate the rebuild itself — the projection
    /// is rebuilt every frame regardless.
    scene_targets: Vec<GraphRef>,
    /// Set by `drain_intents` whenever it applies a step; consumed by the
    /// pre-record rebuild so the record sees doc edits the pre-record
    /// drain made (drag, connection commit). Only meaningful in the
    /// window between the unconditional pre-prepass rebuild (which clears
    /// it) and the pre-record rebuild.
    scene_dirty: bool,
    /// Set alongside the preview store's reconcile request, whenever an
    /// applied step could have removed a node; consumed once per frame to
    /// evict the canvas's `NodeId`-keyed geometry caches. A separate flag
    /// rather than a direct call because the fold that raises it has no
    /// document in hand.
    needs_geometry_prune: bool,
    /// Per-frame accumulator: set by any step that strands
    /// `CanvasGeometry`'s cross-frame caches (see
    /// `invalidates_cached_geometry`) and by `sync_target` for a graph
    /// that has never recorded, then consumed once at the end of `frame`
    /// as a single `ui.request_relayout()`. Reset at the top of every
    /// frame. A plain side-effect field like `scene_dirty`, rather than a
    /// `bool` threaded back through every helper's return.
    needs_relayout: bool,
    /// Per-frame scratch buffer of pending mutations: a graph edit paired
    /// with the graph it commits against (several can be on screen), or a
    /// document-global one, which names none. Cleared at the top of every
    /// `frame`, filled by prepass/record/shortcut handling, and fully
    /// drained before `frame` returns — it carries no state across frames.
    /// Kept as a field only to reuse the allocation; not part of the
    /// observable state.
    intents: Intents,
    /// Per-frame scratch buffer of view-state requests (open/activate/
    /// close tab) raised during record. Drained each frame; carries no
    /// cross-frame state — kept only to reuse the allocation.
    actions: Vec<UiAction>,
    /// The last completed run's per-node state, keyed by the document's
    /// `NodeId`s so it resolves against any tab (root or graph
    /// interior): execution status (the glow + header time, projected into
    /// each `SceneNode::exec_status` at rebuild) and log lines. `App` drives
    /// it as it drains the worker (`RunState::apply_worker_status` / `clear`).
    /// Off the serialized state.
    pub(super) run_state: RunState,
}

impl Editor {
    /// Build fresh GUI editing state for an open document.
    pub(crate) fn new() -> Self {
        Self {
            dirty: false,
            action_stack: ActionStack::new(UNDO_HISTORY_BYTES),
            scene: Scene::default(),
            main_window: MainWindow::default(),
            scene_targets: Vec::new(),
            scene_dirty: false,
            needs_geometry_prune: false,
            needs_relayout: false,
            intents: Intents::default(),
            actions: Vec::new(),
            run_state: RunState::default(),
        }
    }

    /// Apply a single `intent` against the active target and record it as
    /// its own undo entry. For edits raised *outside* the frame's intent
    /// drain — e.g. a file-picker result `App` handles after the record.
    /// No-ops (and self-cancelling steps) are dropped, like the in-frame
    /// drain.
    pub(super) fn apply_edit(&mut self, open: &mut OpenDocument, intent: Intent) {
        let target = open.document.focused_target().unwrap_or(GraphRef::Main);
        self.commit_widget_batch(open, [Queued::Scoped { target, intent }]);
    }

    /// Commit a batch the GUI itself built, reporting whether anything
    /// applied. The trust half of the pipeline that [`Self::commit_script_batch`]
    /// is the other side of, and the reason the two can't be confused: this
    /// return type has nowhere to put a refusal.
    ///
    /// Nothing raised here can legitimately be malformed. Widgets read every
    /// identity they emit out of the live document, so the worst they build
    /// is stale — which refuses [`Refusal::Quiet`]ly and is dropped — and the
    /// data that *is* untrusted never reaches an `Intent` unchecked: an
    /// imported template and the graph-library file are both validated at
    /// their own boundary (`io::graph_template::load`, `io::graph_library`'s
    /// load-and-quarantine). So a [`Refusal::Invalid`] here is our own bug,
    /// and it fails loudly in every build rather than going to a log nobody
    /// reads.
    fn commit_widget_batch(
        &mut self,
        open: &mut OpenDocument,
        queued: impl IntoIterator<Item = Queued>,
    ) -> bool {
        let outcome = self.commit_queued(open, queued);
        assert!(
            outcome.rejected.is_empty(),
            "a widget built a malformed intent: {:?}",
            outcome.rejected,
        );
        outcome.applied
    }

    /// Commit a batch a *script* built, against the active target and as a
    /// single undo entry — the untrusted half that
    /// [`Self::commit_widget_batch`] is the other side of. Used by `App`
    /// when draining the script inbound queue before the frame; the
    /// unconditional pre-prepass rebuild folds the edits in, so
    /// `scene_dirty` needn't be set here.
    ///
    /// Returns the reason for each intent the edit layer refused as
    /// malformed, for the caller to answer the script with. The payload is
    /// deserialized straight into an [`Intent`]
    /// (`script::engine::decode_action`), so a bad one has to report rather
    /// than crash the editor the way the same payload from a widget would.
    /// Scripts drive graph edits only — a [`DocIntent`] has no decoded
    /// form, so nothing here can rearrange the dock.
    pub(super) fn commit_script_batch(
        &mut self,
        open: &mut OpenDocument,
        intents: Vec<Intent>,
    ) -> Vec<String> {
        let target = open.document.focused_target().unwrap_or(GraphRef::Main);
        self.commit_queued(
            open,
            intents
                .into_iter()
                .map(|intent| Queued::Scoped { target, intent }),
        )
        .rejected
    }

    /// Build, apply, and record `queued` — each item against whatever it
    /// names — accumulating the frame's relayout/reconcile signals. The
    /// trust-agnostic core both [`Self::commit_widget_batch`] and
    /// [`Self::commit_script_batch`] wrap, and the only place that reads a
    /// [`BatchOutcome`] whole: no-op and stale intents (anchor node already
    /// gone) are dropped per-intent, and an empty batch records nothing.
    ///
    /// A *run* of scoped intents sharing a target becomes one undo entry, so
    /// a gesture that emits N of them is still one Ctrl+Z. A change of
    /// target mid-stream flushes first — an entry records against exactly
    /// one graph, and undoing across two panes at once would be a lie about
    /// what the user did.
    ///
    /// A document-global intent shares an entry with nothing: it resolves no
    /// graph, so there is no run for it to belong to, and it closes the one
    /// in progress. Staying a single-step entry is also what keeps it
    /// reachable by gesture coalescing — a tab-switch burst and a divider's
    /// drag frames collapse in the stack, which a mixed multi-step entry
    /// would forfeit.
    fn commit_queued(
        &mut self,
        open: &mut OpenDocument,
        queued: impl IntoIterator<Item = Queued>,
    ) -> BatchOutcome {
        let mut batch = Vec::new();
        let mut open_scope = None;
        let mut applied = false;
        let mut rejected = Vec::new();
        let mut signals = StepSignals::default();
        for item in queued {
            let (scope, built) = match item {
                Queued::Scoped { target, intent } => (
                    BatchScope::Graph(target),
                    commit_intent(intent, &mut open.document, target),
                ),
                Queued::Global(intent) => (
                    BatchScope::Document,
                    commit_doc_intent(intent, &mut open.document),
                ),
            };
            let step = match built {
                Ok(step) => step,
                Err(Refusal::Quiet) => continue,
                Err(Refusal::Invalid(reason)) => {
                    rejected.push(reason);
                    continue;
                }
            };
            if open_scope != Some(scope) || scope == BatchScope::Document {
                applied |= self.flush_batch(open_scope, &mut batch);
                open_scope = Some(scope);
            }
            signals.fold(&step);
            batch.push(step);
        }
        applied |= self.flush_batch(open_scope, &mut batch);
        self.absorb_signals(signals);
        BatchOutcome { applied, rejected }
    }

    /// Record the accumulated run as one undo entry against its scope and
    /// empty it, reporting whether anything was there. `scope` is `None`
    /// only before the first committed step, when `batch` is necessarily
    /// empty.
    fn flush_batch(&mut self, scope: Option<BatchScope>, batch: &mut Vec<UndoStep>) -> bool {
        if batch.is_empty() {
            return false;
        }
        let scope = scope.expect("a non-empty batch was opened by some step's scope");
        self.action_stack.push_current(scope, batch);
        batch.clear();
        true
    }

    /// Land folded [`StepSignals`] on the frame's accumulators. The one place
    /// each signal's *effect* is spelled out, for both the commit path and
    /// undo/redo replay.
    fn absorb_signals(&mut self, signals: StepSignals) {
        self.needs_relayout |= signals.geometry_stale;
        // A content edit (or an undone/redone one) leaves the doc differing
        // from the last save — barring the exact round-trip back to it, where
        // we accept a stale "dirty" rather than tracking saved state precisely.
        self.dirty |= signals.dirtied;
        // Pins and viewer tabs move in both directions, so the store has to
        // re-derive which ports it still owes a presentation resource.
        if signals.reconcile {
            self.run_state.previews.request_reconcile();
            self.needs_geometry_prune = true;
        }
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
    pub(crate) fn frame(
        &mut self,
        open: &mut OpenDocument,
        ui: &mut Ui,
        library: &Library,
        theme: &Theme,
        preferences: &mut Preferences,
        status: StatusInputs<'_>,
    ) -> Option<AppCommand> {
        self.intents.clear();
        self.actions.clear();
        self.needs_relayout = false;

        // Settle the active tab entirely from frame-top inputs (keyboard
        // undo/redo + last-frame click responses). `navigate` reads *last*
        // frame's `scene` to resolve tab/chip clicks, so it must run before
        // this frame's rebuild. After it, the active tab is fixed.
        self.navigate(ui, open);

        // Tabs are settled: drop viewer state for closed tabs.
        self.sync_image_viewers(open);
        // Only when an edit moved the retained set or a fresh value landed —
        // an idle frame has nothing to release or upload.
        self.run_state
            .previews
            .reconcile_if_needed(ui, &open.document);
        // Same signal, same reason, one cache over: the canvas's port-offset
        // and node-size tables are keyed by `NodeId` and deliberately outlive
        // the scene, so only the document can say when an entry's node is
        // gone rather than merely off-screen.
        if std::mem::take(&mut self.needs_geometry_prune) {
            self.main_window.release_dead_nodes(&open.document);
        }
        // Every pane showing a graph gets a canvas; a layout of nothing but
        // non-graph views (Preferences, viewers) leaves the set empty and
        // skips the canvas pipeline entirely.
        self.sync_targets(open);

        // Rebuild the projection for this frame, after the navigation
        // phase has fully settled the document — so prepass and
        // `CanvasGeometry` never read a stale graph. Unconditional:
        // `Scene` re-interns port names into the active record-pass text
        // arena.
        self.rebuild_scene(ui, open, library);
        self.scene_dirty = false;

        if !self.scene_targets.is_empty() {
            // Prepass emits input-derived graph mutations (drag, pan/zoom,
            // connection commit) drained *before* the record so Pass A sees
            // the settled doc. It reads everything off `Scene`, and each
            // intent it raises carries the pane it came from.
            let frame = Frame {
                scene: &self.scene,
                doc: &open.document,
            };
            self.main_window
                .prepass(ui, frame, library, &mut self.intents);
            self.drain_intents(open);
            self.apply_canvas_shortcuts(ui, open);
        }

        let command_from_shortcut = self.menu_shortcut(ui);

        // Record. Rebuild again only if the pre-record drain actually
        // changed the doc (drag, connection commit) — an idle frame or a
        // bare tab switch leaves `scene_dirty` false and skips it.
        if self.scene_dirty {
            self.rebuild_scene(ui, open, library);
            self.scene_dirty = false;
        }
        let ctx = AppContext {
            theme,
            library,
            run_state: &self.run_state,
            status_error: status.error,
            process_memory: status.process_memory,
        };
        let command = self
            .main_window
            .frame(
                ui,
                &ctx,
                Frame {
                    scene: &self.scene,
                    doc: &open.document,
                },
                preferences,
                &mut self.intents,
            )
            .or(command_from_shortcut);

        // A node context-menu pick resolves here, where the Document is
        // available to build the duplicate / removal intents against the
        // live selection (the canvas gesture only sees the read-only Scene).
        // Pushed before the post-record drain so it lands this frame.
        if let Some((action, target)) = self.main_window.graph_ui.take_node_menu_action() {
            self.apply_node_menu_action(open, action, target);
        }

        // Post-record drain — graph edits the record surfaced (node select,
        // cache toggle, const edit), each carrying its own target, plus the
        // tab strip's document-global ones (dock ops, graph renames).
        self.drain_intents(open);

        // Sole consumption point for the frame's accumulated signal (edits,
        // tab switch, undo/redo), and darkroom's only `request_relayout`.
        // Resizes driven by something other than an `UndoStep` — the
        // header's elapsed-time label growing as a run reports — are not
        // covered: they leave `CanvasGeometry`'s offsets stale for one
        // frame rather than buying a pass.
        if self.needs_relayout {
            ui.request_relayout();
        }
        command
    }

    /// Resolve a node context-menu pick against `target`'s live selection
    /// (right-click already selected the clicked node). `target` is the pane
    /// the menu was opened in, carried on the pick — the selection it reads
    /// and the intents it queues must name the same graph. Duplicate
    /// variants reuse the Ctrl+D builder; Remove mirrors the Delete-key path
    /// — one intent per selected member, batched into a single undo entry by
    /// the post-record drain.
    fn apply_node_menu_action(
        &mut self,
        open: &OpenDocument,
        action: NodeMenuAction,
        target: GraphRef,
    ) {
        let Some(view) = open.document.view(target) else {
            return;
        };
        let document = &open.document;
        match action {
            NodeMenuAction::Duplicate | NodeMenuAction::DuplicateWithIncoming => {
                let incoming = matches!(action, NodeMenuAction::DuplicateWithIncoming);
                let node_ids = selected_node_ids(view);
                if let Some(intent) =
                    build_duplicate_intent_for(document, target, &node_ids, incoming)
                {
                    self.intents.push(target, intent);
                }
            }
            NodeMenuAction::Remove => self
                .intents
                .extend(target, remove_selection_intents(&view.selected)),
        }
    }

    /// Settle which graph is active for this frame, from inputs all
    /// available before the record: keyboard undo/redo (which can replay
    /// a dock-layout change) and tab/graph-open clicks read from
    /// *last* frame's responses.
    ///
    /// Done up front so the edit pipeline runs against a fixed target and
    /// a switched-to graph records in the same present's Pass A.
    fn navigate(&mut self, ui: &mut Ui, open: &mut OpenDocument) {
        self.apply_undo_redo(ui, open);
        // Surface tab/open clicks from last frame's responses. `scene`
        // still holds the last-rendered graph here — exactly the one
        // whose chips were clicked.
        let frame = Frame {
            scene: &self.scene,
            doc: &open.document,
        };
        self.main_window
            .scan_navigation(ui, frame, &mut self.actions);
        // Open mutates the layout directly; activate/close queue
        // undoable `DocIntent::Dock` ops — drain them.
        self.apply_view_actions(open);
        self.drain_intents(open);
        // A closed/deleted target can't be active; fall back to Main.
        open.document.reconcile_with_graph();
    }

    /// Note a possible change to *which* graphs are on screen: when the
    /// visible set differs from what `scene` last reflected — a tab
    /// switched, a pane split, a graph closed — drop transient gesture
    /// state (so a drag started on one graph can't bleed into another) and
    /// request a relayout. Keeps `CanvasGeometry`'s offset cache, so a
    /// graph shown again resolves its port centers immediately. The
    /// rebuild itself is the caller's unconditional one — this only
    /// reacts to the change.
    fn sync_targets(&mut self, open: &OpenDocument) {
        let visible = open.document.visible_targets();
        if self.scene_targets.iter().copied().eq(visible) {
            return;
        }
        self.main_window.reset_transient();
        self.scene_targets.clear();
        self.scene_targets.extend(open.document.visible_targets());
        self.needs_relayout = true;
    }

    /// Rebuild the `Scene` projection from every graph currently on a
    /// canvas. For a `Local` target the scene gets the whole `GraphDef`,
    /// so the interior's boundary nodes can mirror its interface as their
    /// ports.
    fn rebuild_scene(&mut self, ui: &mut Ui, open: &OpenDocument, library: &Library) {
        let document = &open.document;
        // Destructured so `scene` and `run_state` are borrowed as disjoint
        // fields — otherwise `&self.run_state` and `&mut self.scene` collide
        // and the projections have to be collected into a `Vec` first.
        let Self {
            scene, run_state, ..
        } = self;
        scene.rebuild(
            ui,
            library,
            run_state,
            document.visible_targets().filter_map(|target| {
                // Only a local definition carries the interface its
                // boundary nodes mirror; the root graph has neither. A
                // target that no longer resolves is a tab
                // `reconcile_with_graph` is about to prune — skip it rather
                // than panic mid-frame.
                let source = match target {
                    GraphRef::Main => SceneSource::Entry(&document.graph),
                    GraphRef::Local(id) => SceneSource::Def(document.graph.find_graph(id)?),
                };
                Some(GraphProjection {
                    target,
                    source,
                    view: document.view(target)?,
                })
            }),
        );
    }

    /// Drain `intents`, applying each non-no-op intent to `document`,
    /// and push the whole frame's resulting steps onto the undo stack
    /// as a single batch entry — so a gesture that emits N intents
    /// (e.g. breaker swipe deleting K nodes + unbinding M ports) is
    /// one Cmd-Z. Marks the scene dirty when anything applied (so the
    /// pre-record rebuild folds the change in) and accumulates the
    /// relayout / reconcile signals onto the frame's fields.
    fn drain_intents(&mut self, open: &mut OpenDocument) {
        // Called three times a frame and usually with nothing queued.
        if self.intents.is_empty() {
            return;
        }
        // Move the scratch buffer out so it can drive the commit (which
        // borrows `self` mutably), then put the now-empty buffer back to
        // reuse its allocation next frame.
        let mut scratch = std::mem::take(&mut self.intents);
        if self.commit_widget_batch(open, scratch.drain()) {
            self.scene_dirty = true;
        }
        self.intents = scratch;
    }

    /// Apply the record pass's view-state requests. Adding a tab to a
    /// strip is the only non-undoable part of opening; the focus change it
    /// implies, plus activate and close, are queued as `DocIntent::Dock` ops
    /// so they join the undo history (their relayout is decided later,
    /// when they drain). `FocusPane` is the one exception — pointer-driven
    /// focus writes straight through (see `DockLayout::focus`).
    fn apply_view_actions(&mut self, open: &mut OpenDocument) {
        for action in std::mem::take(&mut self.actions) {
            match action {
                UiAction::OpenGraph(target) => self.open_graph(open, target),
                UiAction::Dock(op) => self.intents.push_global(DocIntent::Dock(op)),
                UiAction::FocusPane(group) => {
                    open.document.layout.focus(group);
                }
                UiAction::NewGraph => {
                    // Creating the graph + instance isn't undoable (no undo
                    // history references the fresh graph, so the stack stays
                    // valid); `open_graph` still records the focus switch.
                    // Not routed through a step, so flag the edit directly.
                    // Scoped to the focused graph: a new graph made from a
                    // local tab's strip nests there.
                    let target = open.document.focused_target().unwrap_or(GraphRef::Main);
                    if let Some(id) = open.document.create_graph(target) {
                        self.dirty = true;
                        self.open_graph(open, GraphRef::Local(id));
                    }
                }
                UiAction::OpenImageViewer(node_id) => self.open_image_viewer(open, node_id),
            }
        }
    }

    /// Open `port`'s image-viewer tab and focus it — one tab per port,
    /// deduped. Mirrors [`Self::open_preferences`]: adding the tab is the
    /// non-undoable part, focus routes through a recorded activation.
    fn open_image_viewer(&mut self, open: &mut OpenDocument, node_id: NodeId) {
        let group = open.document.layout.focused;
        let tab = TabRef::ImageViewer(node_id);
        open.document.layout.find_or_insert(tab, group);
        // Inserting the tab is the half no step records, so it has to ask for
        // the reconcile itself: the port's stored value is preview-only until
        // this viewer's full-resolution upload.
        self.run_state.previews.request_reconcile();
        self.push_activate(tab);
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
    fn push_activate(&mut self, tab: TabRef) {
        self.intents.push_global(activate_intent(tab));
    }

    /// Open `target`'s tab in the focused group and focus it — the same
    /// rule the preferences and viewer tabs follow, now that graph tabs
    /// are no longer pinned to one pane. Adding the tab to the strip
    /// (lazily seeding a `Local` interior's view) is the non-undoable
    /// part; focusing it routes through a recorded activation like every
    /// other focus change — queued here, drained right after. Undo then
    /// faithfully reverses focus (the opened tab stays open) and a fresh
    /// open discards the redo tail, instead of mutating focus outside the
    /// record.
    ///
    /// To see two graphs at once, drag one's chip onto a pane edge: the
    /// split is an ordinary `DockOp::MoveTab`.
    fn open_graph(&mut self, open: &mut OpenDocument, target: GraphRef) {
        // Idempotent view seeding, so it can run before the open-or-focus
        // dedupe rather than only inside the "new tab" arm.
        if let GraphRef::Local(id) = target
            && !open.document.ensure_sub_view(id)
        {
            return; // graph vanished — nothing to open
        }
        let group = open.document.layout.focused;
        let tab = TabRef::Graph(target);
        open.document.layout.find_or_insert(tab, group);
        self.push_activate(tab);
    }

    /// Open the [`TabRef::Preferences`] settings tab in the focused group
    /// and focus it (reusing the existing tab wherever it lives). Mirrors
    /// [`Self::open_graph`]: adding the tab is the non-undoable part; the focus
    /// routes through a recorded activation. Called from the File ▸
    /// Preferences menu via `App`, so it records the switch and drains it
    /// immediately like every external edit.
    pub(super) fn open_preferences(&mut self, open: &mut OpenDocument) {
        let group = open.document.layout.focused;
        open.document
            .layout
            .find_or_insert(TabRef::Preferences, group);
        self.commit_widget_batch(open, [Queued::Global(activate_intent(TabRef::Preferences))]);
    }
}

/// The recorded focus/activation half of opening `tab`.
fn activate_intent(tab: TabRef) -> DocIntent {
    DocIntent::Dock(DockOp::ActivateTab { tab })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use glam::Vec2;
    use scenarium::DataType;
    use scenarium::testing;
    use scenarium::{Binding, Func, FuncId, FuncInput, FuncOutput};
    use scenarium::{Graph, InputPort, Node, NodeId, NodeKind, NodeSearch};

    use crate::core::document::open_document::OpenDocument;
    use crate::core::document::{Document, GraphRef, TabRef};
    use crate::core::edit::intent::types::{DocIntent, Intent};
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
                },
            }
        }

        fn open_graph(&mut self, target: GraphRef) {
            self.editor.open_graph(&mut self.open, target);
            self.editor.drain_intents(&mut self.open);
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

    /// A malformed intent — one no `build_step` precondition can accept.
    fn nil_id_add() -> Intent {
        Intent::AddNode {
            pos: Vec2::ZERO,
            node_id: NodeId::nil(),
            node: Node::new(NodeKind::Func(FuncId::unique())),
            bindings: vec![],
        }
    }

    #[test]
    fn a_scripts_malformed_intent_is_refused_and_reported_back() {
        // A script's payload is deserialized straight into an `Intent`, so
        // this is untrusted input, not a bug in our code: refuse it, hand
        // the reason back for the caller to answer the script with, and
        // leave the document alone.
        let mut test = TestEditor::new(Document::default());
        let rejected = test
            .editor
            .commit_script_batch(&mut test.open, vec![nil_id_add()]);

        assert_eq!(rejected.len(), 1, "the one bad intent answered back");
        assert!(
            rejected[0].contains("nil"),
            "the reason names what was wrong: {rejected:?}",
        );
        assert_eq!(test.open.document.graph.len(), 0, "nothing was written");
        assert!(!test.undo(), "and nothing was recorded to undo");
    }

    /// The same payload from the GUI side is our own bug, so it fails loudly
    /// instead of being reported — in every build, since that is the only
    /// way a defect no user action can explain gets noticed.
    #[test]
    #[should_panic(expected = "a widget built a malformed intent")]
    fn a_widget_built_malformed_intent_is_a_bug_not_a_refusal() {
        let mut test = TestEditor::new(Document::default());
        test.editor.apply_edit(&mut test.open, nil_id_add());
    }

    #[test]
    fn dirty_flag_tracks_content_edits_not_navigation() {
        let mut test = TestEditor::new(Document::default());
        assert!(!test.editor.dirty, "a freshly opened document is clean");

        // Seed a node, then rename it — a content edit must dirty.
        let node = Node::new(NodeKind::Func(FuncId::unique()));
        let id = test.open.document.graph.add(node);
        test.open
            .document
            .main_view
            .item_placements
            .insert(id, Vec2::ZERO);
        test.editor.apply_edit(
            &mut test.open,
            Intent::RenameNode {
                node_id: id,
                to: "renamed".into(),
            },
        );
        assert!(test.editor.dirty, "renaming a node is unsaved work");

        // Clear (as a save would), then a pure selection change: applied,
        // but navigation — it must not mark the document dirty again.
        test.editor.dirty = false;
        test.editor.apply_edit(
            &mut test.open,
            Intent::SetSelection {
                to: BTreeSet::from([id]),
            },
        );
        assert_eq!(
            test.open.document.main_view.selected,
            BTreeSet::from([id]),
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
    fn image_viewer_tabs_dedupe_per_node_and_prune_state_on_close() {
        let mut test = TestEditor::new(Document::default());
        // Two real nodes: a viewer tab is keyed by node, and a tab naming a
        // node the graph no longer holds is pruned by `reconcile_with_graph`.
        let ids: Vec<NodeId> = (0..2)
            .map(|_| {
                let id = test
                    .open
                    .document
                    .graph
                    .add(Node::new(NodeKind::Func(FuncId::unique())));
                test.open
                    .document
                    .main_view
                    .item_placements
                    .insert(id, Vec2::ZERO);
                id
            })
            .collect();
        let viewer_node = |n: usize| ids[n];
        let open = |test: &mut TestEditor, p| {
            test.editor.actions.push(UiAction::OpenImageViewer(p));
            test.editor.apply_view_actions(&mut test.open);
            test.editor.drain_intents(&mut test.open);
        };
        let tabs = |test: &TestEditor| test.open.document.layout.all_tabs().collect::<Vec<_>>();
        let active = |test: &TestEditor| test.open.document.layout.primary().active;

        // Opening only changes the layout. Viewer state is created by the
        // renderer when it draws the tab and pulls from `RunState`.
        open(&mut test, viewer_node(0));
        assert_eq!(
            tabs(&test),
            vec![
                TabRef::Graph(GraphRef::Main),
                TabRef::ImageViewer(viewer_node(0))
            ]
        );
        assert_eq!(active(&test), 1);
        assert!(
            test.editor.main_window.image_viewers.is_empty(),
            "the editor does not create or populate view state"
        );
        test.editor
            .main_window
            .image_viewers
            .insert(viewer_node(0), ImageViewer::new(viewer_node(0)));

        // Re-clicking the same node reuses its tab; a different node gets
        // its own renderer-owned state.
        open(&mut test, viewer_node(0));
        assert_eq!(tabs(&test).len(), 2, "the same node dedupes");
        open(&mut test, viewer_node(1));
        assert_eq!(tabs(&test).len(), 3, "a distinct node adds a tab");
        assert_eq!(active(&test), 2);
        test.editor
            .main_window
            .image_viewers
            .insert(viewer_node(1), ImageViewer::new(viewer_node(1)));
        assert!(
            test.editor
                .main_window
                .image_viewers
                .contains_key(&viewer_node(0))
        );
        assert!(
            test.editor
                .main_window
                .image_viewers
                .contains_key(&viewer_node(1))
        );

        test.editor.sync_image_viewers(&test.open);

        // Closing a tab drops its viewer state on the next sync (the
        // remaining port's state survives).
        test.open
            .document
            .layout
            .retain_tabs(|t| t != TabRef::ImageViewer(viewer_node(1)));
        test.open.document.reconcile_with_graph();
        test.editor.sync_image_viewers(&test.open);
        assert!(
            test.editor
                .main_window
                .image_viewers
                .contains_key(&viewer_node(0))
        );
        assert!(
            !test
                .editor
                .main_window
                .image_viewers
                .contains_key(&viewer_node(1)),
            "closed tab's viewer state is pruned"
        );
    }

    #[test]
    fn a_frames_intents_commit_against_their_own_graphs_and_undo_apart() {
        // Two graph panes can be on screen, so one frame's intent queue can
        // name two different targets. Each must land in *its* graph, and a
        // target change must close the undo entry — undoing an edit on one
        // pane must not drag an unrelated edit on the other back with it.
        let mut test = TestEditor::new(Document::default());
        let local = test.open.document.create_graph(GraphRef::Main).unwrap();
        let nested = GraphRef::Local(local);

        let root_node = NodeId::unique();
        let local_node = NodeId::unique();
        let add = |node_id| Intent::AddNode {
            pos: Vec2::ZERO,
            node_id,
            node: Node::new(NodeKind::Func(FuncId::unique())),
            bindings: vec![],
        };
        test.editor.intents.push(GraphRef::Main, add(root_node));
        test.editor.intents.push(nested, add(local_node));
        test.editor.drain_intents(&mut test.open);

        let doc = &test.open.document;
        let root = &doc.graph;
        let body = &doc.graph.find_graph(local).unwrap().body;
        assert!(
            root.find(root_node, NodeSearch::TopLevel).is_some(),
            "the Main-targeted intent landed in the root graph"
        );
        assert!(
            body.find(local_node, NodeSearch::TopLevel).is_some(),
            "the Local-targeted intent landed in the definition body"
        );
        assert!(
            root.find(local_node, NodeSearch::TopLevel).is_none()
                && body.find(root_node, NodeSearch::TopLevel).is_none(),
            "neither intent leaked into the other pane's graph"
        );

        // Two entries, not one: the second undo reaches back to the root
        // edit only after the nested one is undone.
        assert!(test.undo(), "undo the nested add");
        assert!(
            test.open
                .document
                .graph
                .find_graph(local)
                .unwrap()
                .body
                .find(local_node, NodeSearch::TopLevel)
                .is_none(),
        );
        assert!(
            test.open
                .document
                .graph
                .find(root_node, NodeSearch::TopLevel)
                .is_some(),
            "the root pane's edit survives the nested pane's undo"
        );
        assert!(test.undo(), "a second entry holds the root add");
        assert!(
            test.open
                .document
                .graph
                .find(root_node, NodeSearch::TopLevel)
                .is_none()
        );
    }

    #[test]
    fn a_document_global_intent_records_as_an_entry_of_its_own() {
        // A `DocIntent` resolves no graph, so it can neither join a graph's
        // entry nor let one continue across it. Three intents in one drain —
        // edit, dock op, edit, all raised while the same pane is focused —
        // must therefore become three entries that undo one at a time.
        use crate::core::document::dock::DockOp;

        let mut test = TestEditor::new(Document::default());
        let local = test.open.document.create_graph(GraphRef::Main).unwrap();
        let subgraph = TabRef::Graph(GraphRef::Local(local));
        let primary = test.open.document.layout.primary().id;
        test.open.document.layout.find_or_insert(subgraph, primary);
        let main_tab = test.open.document.layout.primary().active_tab();

        let first = NodeId::unique();
        let second = NodeId::unique();
        let add = |node_id| Intent::AddNode {
            pos: Vec2::ZERO,
            node_id,
            node: Node::new(NodeKind::Func(FuncId::unique())),
            bindings: vec![],
        };
        let activate = |tab| DocIntent::Dock(DockOp::ActivateTab { tab });
        test.editor.intents.push(GraphRef::Main, add(first));
        test.editor.intents.push_global(activate(subgraph));
        test.editor.intents.push(GraphRef::Main, add(second));
        test.editor.drain_intents(&mut test.open);

        let active = |test: &TestEditor| test.open.document.layout.primary().active_tab();
        let holds = |test: &TestEditor, id| {
            test.open
                .document
                .graph
                .find(id, NodeSearch::TopLevel)
                .is_some()
        };
        assert_eq!(active(&test), subgraph, "the dock op applied");
        assert!(
            holds(&test, first) && holds(&test, second),
            "both edits did"
        );

        assert!(test.undo(), "the last entry is the second add alone");
        assert!(
            holds(&test, first) && !holds(&test, second),
            "the add after the dock op didn't drag the one before it back",
        );
        assert_eq!(active(&test), subgraph, "and left the dock op standing");

        assert!(test.undo(), "the middle entry is the dock op alone");
        assert_eq!(active(&test), main_tab, "the activation reverted");
        assert!(holds(&test, first), "without touching the edit under it");

        assert!(test.undo(), "the first entry is the first add");
        assert!(!holds(&test, first));

        // Consecutive globals stay one entry *each*, which is what keeps a
        // tab-switch burst collapsing to one Ctrl+Z: a single-step entry
        // carries its gesture key, so the two queued together below merge
        // with each other, and the next drain's folds into the result.
        // Batched into one entry instead they would lose the key — a
        // multi-step entry never coalesces — and the burst would undo in
        // pieces.
        let more: Vec<TabRef> = (0..2)
            .map(|_| {
                let id = test.open.document.create_graph(GraphRef::Main).unwrap();
                let tab = TabRef::Graph(GraphRef::Local(id));
                test.open.document.layout.find_or_insert(tab, primary);
                tab
            })
            .collect();
        test.editor.intents.push_global(activate(subgraph));
        test.editor.intents.push_global(activate(more[0]));
        test.editor.drain_intents(&mut test.open);
        test.editor.intents.push_global(activate(more[1]));
        test.editor.drain_intents(&mut test.open);
        assert_eq!(active(&test), more[1], "the burst's last switch stands");

        assert!(test.undo(), "the whole burst is one entry");
        assert_eq!(
            active(&test),
            main_tab,
            "one undo reverts every switch in it, across both drains",
        );
        assert!(!test.undo(), "and nothing else was recorded");
    }

    /// A node context-menu pick acts on the pane the menu was raised in —
    /// both halves: the selection it reads and the graph it queues against.
    /// Reading `focused_target()` instead removed nothing, because the
    /// focused pane's selection is empty; queuing without that pane's scope
    /// aimed the removal at `Main`, which refuses a node it doesn't hold.
    #[test]
    fn a_node_menu_pick_acts_on_its_own_pane_not_the_focused_one() {
        use crate::gui::canvas::node_menu::NodeMenuAction;

        let mut test = TestEditor::new(Document::default());
        let local = test.open.document.create_graph(GraphRef::Main).unwrap();
        let nested = GraphRef::Local(local);

        // One selected node per pane, so acting on the wrong one is visible
        // rather than a silent no-op.
        let seed = |test: &mut TestEditor, target| {
            let scope = test.open.document.scope_mut(target).unwrap();
            let id = scope.graph.add(Node::new(NodeKind::Func(FuncId::unique())));
            scope.view.item_placements.insert(id, Vec2::ZERO);
            scope.view.selected = BTreeSet::from([id]);
            id
        };
        let root_node = seed(&mut test, GraphRef::Main);
        let local_node = seed(&mut test, nested);

        // Focus sits on the nested pane while the menu is raised over the
        // root one.
        test.open_graph(nested);
        assert_eq!(test.open.document.focused_target(), Some(nested));
        test.editor
            .apply_node_menu_action(&test.open, NodeMenuAction::Remove, GraphRef::Main);
        test.editor.drain_intents(&mut test.open);
        assert!(
            test.open
                .document
                .graph
                .find(root_node, NodeSearch::TopLevel)
                .is_none(),
            "the menu's own pane lost its selected node"
        );
        assert!(
            test.open
                .document
                .graph
                .find_graph(local)
                .unwrap()
                .body
                .find(local_node, NodeSearch::TopLevel)
                .is_some(),
            "the focused pane's selection was untouched"
        );

        // And the other way round: focus on Main, menu raised in the nested
        // pane. The intents must be queued against that pane too — aimed at
        // `Main` they'd name a node it doesn't hold and be refused.
        test.open_graph(GraphRef::Main);
        assert_eq!(test.open.document.focused_target(), Some(GraphRef::Main));
        test.editor
            .apply_node_menu_action(&test.open, NodeMenuAction::Remove, nested);
        test.editor.drain_intents(&mut test.open);
        assert!(
            test.open
                .document
                .graph
                .find_graph(local)
                .unwrap()
                .body
                .find(local_node, NodeSearch::TopLevel)
                .is_none(),
            "the nested pane's selected node was removed"
        );
    }

    /// Focus that merely follows a press into a pane is applied straight to
    /// the layout, never recorded.
    ///
    /// Recording it would spring a trap, because `navigate` runs undo, the
    /// dock scan, and the drain in that order within one frame: keyboard
    /// focus is sticky, so an undo restoring the old focus would be
    /// re-applied by the very next scan — the undo would appear to do
    /// nothing *and* `push_current` would discard the redo tail. It would
    /// also mean an undo entry per click into another pane.
    #[test]
    fn pointer_driven_pane_focus_stays_out_of_the_undo_history() {
        use crate::core::document::dock::{DockDrop, DockOp, SplitSide};

        let mut test = TestEditor::new(Document::default());
        let local = test.open.document.create_graph(GraphRef::Main).unwrap();
        let layout = &mut test.open.document.layout;
        let primary = layout.primary().id;
        let subgraph = TabRef::Graph(GraphRef::Local(local));
        layout.find_or_insert(subgraph, primary);
        layout.apply(DockOp::MoveTab {
            tab: subgraph,
            to: DockDrop::Split {
                group: primary,
                side: SplitSide::Right,
            },
        });
        assert_ne!(layout.focused, primary, "the split pane starts focused");

        // One real edit, then a pointer-driven focus change onto the other
        // pane — the exact order that makes a recorded focus entry bury the
        // edit one Ctrl+Z deeper.
        let node = NodeId::unique();
        test.editor.intents.push(
            GraphRef::Main,
            Intent::AddNode {
                pos: Vec2::ZERO,
                node_id: node,
                node: Node::new(NodeKind::Func(FuncId::unique())),
                bindings: vec![],
            },
        );
        test.editor.drain_intents(&mut test.open);
        test.editor.actions.push(UiAction::FocusPane(primary));
        test.editor.apply_view_actions(&mut test.open);
        test.editor.drain_intents(&mut test.open);
        assert_eq!(
            test.open.document.layout.focused, primary,
            "the press moved dock focus"
        );

        // The first undo reaches the *edit*, not a focus entry.
        assert!(test.undo());
        assert!(
            test.open
                .document
                .graph
                .find(node, NodeSearch::TopLevel)
                .is_none(),
            "one Ctrl+Z undoes the edit — no focus entry in front of it"
        );
        assert!(!test.undo(), "and nothing else was recorded");
        assert!(test.redo(), "the redo tail survived");
        assert!(
            test.open
                .document
                .graph
                .find(node, NodeSearch::TopLevel)
                .is_some()
        );
    }

    #[test]
    fn opening_a_graph_records_an_undoable_focus_switch() {
        let mut test = TestEditor::new(Document::default());
        let a = test.open.document.create_graph(GraphRef::Main).unwrap();
        let b = test.open.document.create_graph(GraphRef::Main).unwrap();
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
    fn passthrough_rewire_preserves_downstream_wiring() {
        let float_src = testing::with_stub_lambda(
            Func::new(FuncId::unique(), "fsrc").output(FuncOutput::new("o", DataType::Float)),
        );
        let string_src = testing::with_stub_lambda(
            Func::new(FuncId::unique(), "ssrc").output(FuncOutput::new("o", DataType::String)),
        );
        let float_sink = testing::with_stub_lambda(
            Func::new(FuncId::unique(), "fsink")
                .input(FuncInput::required("x", DataType::Float))
                .output(FuncOutput::new("o", DataType::Float)),
        );
        let pass_func = testing::with_stub_lambda(
            Func::new(FuncId::unique(), "pass")
                .input(FuncInput::required("x", DataType::Any))
                .wildcard_output("o", 0),
        );

        let mut graph = Graph::default();
        let fp = graph.add_func_node(&float_src);
        let sp = graph.add_func_node(&string_src);
        let pass = graph.add_func_node(&pass_func);
        let sink = graph.add_func_node(&float_sink);
        graph.set_input_binding(InputPort::new(pass, 0), Binding::bind(fp, 0));
        graph.set_input_binding(InputPort::new(sink, 0), Binding::bind(pass, 0));

        let mut test = TestEditor::new(graph.into());

        // Rewire the passthrough input to the String producer. The sink's
        // wire is now type-incompatible, but nothing severs it: type
        // mismatches degrade at flatten (drift tolerance) while the
        // authored wiring survives — and revives if the types line up
        // again. The edit is a single step, so undo touches only it.
        test.editor.apply_edit(
            &mut test.open,
            Intent::SetInput {
                input: InputPort::new(pass, 0),
                to: Some(Binding::bind(sp, 0)),
            },
        );
        assert_eq!(
            test.open
                .document
                .graph
                .bindings
                .get(&InputPort::new(sink, 0)),
            Some(&Binding::bind(pass, 0)),
            "the mismatched sink wire is preserved, not severed"
        );

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
            "the sink wire was never touched"
        );
    }
}
