//! The per-frame GUI edit pipeline over a borrowed [`OpenDocument`].
//!
//! `Editor` owns the undo history, the GUI tree, and transient gesture
//! state. The canvas's own projection of the graph lives with the canvas
//! that draws it. [`App`] lends it the open document for each operation and
//! the frame's [`AppCtx`] to read the rest through, keeping document,
//! runtime and run-state ownership on the shell.
//!
//! [`App`]: crate::gui::app::App

use crate::core::document::open_document::OpenDocument;
use crate::core::document::{Document, TabRef};
use crate::core::edit::action_stack::ActionStack;
use crate::core::edit::intent::apply::commit_intent;
use crate::core::edit::intent::types::{GraphIntent, Refusal, UndoStep};
use crate::core::io::preferences::Preferences;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::file::FileCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::requests::{DocumentRequest, Requests};
use crate::gui::window::MainWindow;
use palantir::{Shortcut, Ui};

use crate::gui::app::ctx::AppCtx;

#[cfg(test)]
pub(crate) mod harness;

/// Byte budget for the undo history's packed buffer (~1 MiB). Bounds
/// memory rather than entry count — a single large edit can't be
/// undone away, but the oldest entries drop once the buffer overflows.
const UNDO_HISTORY_BYTES: usize = 1 << 20;

const UNDO_SHORTCUT: Shortcut = Shortcut::ctrl('Z');
const REDO_SHORTCUT: Shortcut = Shortcut::ctrl_shift('Z');
const NEW_SHORTCUT: Shortcut = Shortcut::ctrl('N');
const OPEN_SHORTCUT: Shortcut = Shortcut::ctrl('O');
const SAVE_SHORTCUT: Shortcut = Shortcut::ctrl('S');
const SAVE_AS_SHORTCUT: Shortcut = Shortcut::ctrl_shift('S');
const RUN_SHORTCUT: Shortcut = Shortcut::ctrl('R');
/// ⌘Q on macOS, Ctrl+Q elsewhere. Routes through `AppCommand::Quit` →
/// `App::guard_discard`, so it prompts to save when the document is dirty
/// — same path as File ▸ Quit. (palantir drops winit's default macOS menu
/// so ⌘Q reaches us instead of hard-terminating.)
const QUIT_SHORTCUT: Shortcut = Shortcut::ctrl('Q');

/// What applying one or more [`UndoStep`]s obliges the frame to do, folded
/// off the steps' own predicates. Accumulated as a value rather than written
/// straight onto [`Editor`] because undo/redo replay folds its steps from a
/// callback while `action_stack` itself is mutably borrowed — so both the
/// commit path and the replay path fold here and hand the result to
/// [`Editor::absorb_signals`], and a new signal is added in one place.
#[derive(Default, Debug)]
struct StepSignals {
    geometry_stale: bool,
    dirtied: bool,
}

impl StepSignals {
    fn fold(&mut self, step: &UndoStep) {
        self.geometry_stale |= step.invalidates_cached_geometry();
        self.dirtied |= step.dirties_document();
    }
}

#[derive(Debug)]
pub(crate) struct Editor {
    action_stack: ActionStack,
    main_window: MainWindow,
    /// Per-frame accumulator: set by any step that strands
    /// `CanvasGeometry`'s cross-frame caches (see
    /// `invalidates_cached_geometry`) and by `GraphUI::sync_visibility` for
    /// a canvas that has never recorded, then consumed once at the end of
    /// `frame` as a single `ui.request_relayout()`. Reset at the top of
    /// every frame. A plain side-effect field rather than a `bool` threaded
    /// back through every helper's return.
    needs_relayout: bool,
}

impl Editor {
    /// Build fresh GUI editing state for an open document.
    pub(crate) fn new() -> Self {
        Self {
            action_stack: ActionStack::new(UNDO_HISTORY_BYTES),
            main_window: MainWindow::default(),
            needs_relayout: false,
        }
    }

    /// Apply a single `intent` and record it as its own undo entry. For
    /// edits raised *outside* the frame's intent drain — e.g. a file-picker
    /// result `App` handles after the record. No-ops (and self-cancelling
    /// steps) are dropped, like the in-frame drain.
    pub(super) fn apply_edit(&mut self, open: &mut OpenDocument, intent: GraphIntent) {
        self.commit_batch(open, [DocumentRequest::Graph(intent)]);
    }

    /// Apply `queued`, each request according to its tier: a graph edit is
    /// built, applied, and recorded; a view op goes straight to the layout.
    /// The app tier never reaches here — it stays queued for the shell.
    ///
    /// Nothing raised here can legitimately be malformed. Widgets read every
    /// identity they emit out of the live document, so the worst they build
    /// is stale — which refuses [`Refusal::Quiet`]ly and is dropped. A
    /// [`Refusal::Invalid`] is therefore our own bug, and it panics in every
    /// build rather than going to a log nobody reads.
    ///
    /// No-op and stale intents are dropped per-intent, and an empty batch
    /// records nothing. A *run* of intents becomes one undo entry, so a
    /// gesture that emits N of them is still one Ctrl+Z.
    ///
    /// Returns nothing: what the caller would do with the outcome is a
    /// [`StepSignals`] effect, landed by [`Self::absorb_signals`] alongside
    /// the other two rather than handed back to be acted on separately.
    fn commit_batch(
        &mut self,
        open: &mut OpenDocument,
        queued: impl IntoIterator<Item = DocumentRequest>,
    ) {
        let mut batch = Vec::new();
        let mut signals = StepSignals::default();
        for item in queued {
            let intent = match item {
                DocumentRequest::Graph(intent) => intent,
                // Applied straight to the layout: pane arrangement is
                // navigation, so it records no step, raises no signal, and
                // breaks no run of graph edits around it.
                DocumentRequest::View(op) => {
                    open.document.apply_dock_op(op);
                    continue;
                }
            };
            let step = match commit_intent(intent, &mut open.document) {
                Ok(step) => step,
                Err(Refusal::Quiet) => continue,
                Err(Refusal::Invalid(reason)) => {
                    panic!("a widget built a malformed intent: {reason}")
                }
            };
            signals.fold(&step);
            batch.push(step);
        }
        self.action_stack.push_current(&batch);
        batch.clear();
        self.absorb_signals(open, signals);
    }

    /// Land folded [`StepSignals`] on the frame's accumulators. The one place
    /// each signal's *effect* is spelled out, for both the commit path and
    /// undo/redo replay.
    fn absorb_signals(&mut self, open: &mut OpenDocument, signals: StepSignals) {
        self.needs_relayout |= signals.geometry_stale;
        // A content edit (or an undone/redone one) leaves the doc differing
        // from the last save — barring the exact round-trip back to it, where
        // we accept a stale "dirty" rather than tracking saved state precisely.
        open.dirty |= signals.dirtied;
    }

    /// Run one frame of the edit pipeline against `ctx` — the frame's
    /// read-only world — draining everything it raises against the document
    /// and leaving the app tier queued in `requests` for the shell.
    ///
    /// The frame splits into a **navigation phase** (settle which tab is
    /// active, from frame-top inputs) and an **edit phase** (mutate the
    /// graph), because input that switches tabs comes from *last* frame's
    /// click responses and must resolve before anything edits or records.
    pub(crate) fn frame(
        &mut self,
        ui: &mut Ui,
        open: &mut OpenDocument,
        ctx: AppCtx<'_>,
        preferences: &mut Preferences,
        requests: &mut Requests,
    ) {
        requests.clear();
        self.needs_relayout = false;

        // Settle the active tab entirely from frame-top inputs (keyboard
        // undo/redo + last-frame click responses). `navigate` reads *last*
        // frame's projection to resolve tab/chip clicks, so it must run
        // before this frame's rebuild. After it, the active tab is fixed.
        self.navigate(ui, open, ctx, requests);

        // Tabs are settled: drop viewer state for closed tabs. A tab-lifetime
        // question, not a node-liveness one — which is why it belongs to the
        // frame rather than to `App`'s once-a-frame sweep.
        self.sync_image_viewers(open);
        // A canvas that just appeared or disappeared drops its tab-local
        // gesture state and needs a relayout — it may never have recorded,
        // and a dock op raises no geometry signal of its own.
        self.needs_relayout |= self.main_window.graph_ui.sync_visibility(&open.document);

        // Prepass rebuilds the canvas's projection, then emits input-derived
        // graph mutations (drag, pan/zoom, connection commit) drained *before*
        // the record so Pass A sees the settled doc. Driven by the panes on
        // screen, like the record pass below — a pane kind that grows input
        // handling gets an arm there rather than another question here.
        self.main_window.prepass(ui, ctx, &open.document, requests);
        self.drain_requests(open, requests);

        self.menu_shortcut(ui, requests);
        self.main_window
            .frame(ui, ctx, &open.document, preferences, requests);

        // Post-record drain — graph edits the record surfaced (node select,
        // cache toggle, const edit), plus the tab strip's dock ops.
        self.drain_requests(open, requests);

        // Sole consumption point for the frame's accumulated signal (edits,
        // tab switch, undo/redo), and darkroom's only `request_relayout`.
        // Resizes driven by something other than an `UndoStep` — the
        // header's elapsed-time label growing as a run reports — are not
        // covered: they leave `CanvasGeometry`'s offsets stale for one
        // frame rather than buying a pass.
        if self.needs_relayout {
            ui.request_relayout();
        }
    }

    /// Settle which tab is active for this frame, from inputs all available
    /// before the record: keyboard undo/redo and tab clicks read from *last*
    /// frame's responses.
    ///
    /// Done up front so the edit pipeline runs against a settled document
    /// and a switched-to tab records in the same present's Pass A.
    fn navigate(
        &mut self,
        ui: &mut Ui,
        open: &mut OpenDocument,
        ctx: AppCtx<'_>,
        requests: &mut Requests,
    ) {
        self.apply_undo_redo(ui, open);
        // Surface tab clicks from last frame's responses. Those responses are
        // last frame's; the document they resolve against is this frame's,
        // so a hit on a node the undo above removed simply finds nothing.
        self.main_window
            .scan_navigation(ui, ctx, &open.document, requests);
        // Dock ops apply straight to the layout — drain them.
        self.drain_requests(open, requests);
        // A tab whose node is gone can't stay open.
        open.document.reconcile_with_graph();
    }

    /// Ctrl+Z / Ctrl+Shift+Z. Replays undo/redo against the document
    /// (each entry carries its own graph target).
    ///
    /// The chords are sampled via `key_pressed` *every frame,
    /// unconditionally* — that call both reads the press and keeps the
    /// chord subscribed, and palantir's keyboard wake-gate only delivers
    /// an off-focus press when its chord was subscribed last frame
    /// (subscriptions clear each frame).
    ///
    /// No focus test: Ctrl+Z is `KeyClass::Edit`, so while a text field
    /// holds focus palantir grants it to that field's scope and this
    /// read answers `false` on its own.
    fn apply_undo_redo(&mut self, ui: &mut Ui, open: &mut OpenDocument) {
        let undo = ui.key_pressed(UNDO_SHORTCUT);
        let redo = ui.key_pressed(REDO_SHORTCUT);
        // Folded into a value first: the replay callback runs while
        // `action_stack` is mutably borrowed, so it can't touch `self`.
        let mut signals = StepSignals::default();
        let mut on_step = |step: &UndoStep| signals.fold(step);
        if undo {
            self.action_stack.undo(&mut open.document, &mut on_step);
        } else if redo {
            self.action_stack.redo(&mut open.document, &mut on_step);
        }
        self.absorb_signals(open, signals);
    }

    /// Queue the [`AppCommand`] for whichever of Ctrl+N / Ctrl+O / Ctrl+S /
    /// Ctrl+Shift+S / Ctrl+R / Ctrl+Q fired.
    ///
    /// Document file ops are **global** — they fire regardless of
    /// focus, so Ctrl+S still saves while a node's value editor is
    /// focused (TextEdit doesn't bind S/O/N, so nothing is stolen).
    /// Every chord is sampled with `key_pressed` each frame so all
    /// stay subscribed for palantir's wake-gate (sampling them all up
    /// front, not short-circuited, so one chord firing doesn't drop
    /// the others' subscription that frame). Save-As (Ctrl+Shift+S) is
    /// checked before Save (Ctrl+S) so the shift variant wins its
    /// combo. Theme actions are menu-only — no shortcut.
    fn menu_shortcut(&self, ui: &mut Ui, requests: &mut Requests) {
        let new = ui.key_pressed(NEW_SHORTCUT);
        let open = ui.key_pressed(OPEN_SHORTCUT);
        let save_as = ui.key_pressed(SAVE_AS_SHORTCUT);
        let save = ui.key_pressed(SAVE_SHORTCUT);
        let run = ui.key_pressed(RUN_SHORTCUT);
        let quit = ui.key_pressed(QUIT_SHORTCUT);
        let command = if new {
            AppCommand::File(FileCommand::New)
        } else if open {
            AppCommand::File(FileCommand::Load)
        } else if save_as {
            AppCommand::File(FileCommand::SaveAs)
        } else if save {
            AppCommand::File(FileCommand::Save)
        } else if run {
            AppCommand::Run(RunCommand::Once)
        } else if quit {
            AppCommand::Quit
        } else {
            return;
        };
        requests.push_app(command);
    }

    /// Drain `requests`, landing each on its tier, and push the whole frame's
    /// resulting steps onto the undo stack as a single batch entry — so a
    /// gesture that emits N intents (a breaker swipe deleting K nodes and
    /// unbinding M ports) is one Cmd-Z. Marks the projection dirty when
    /// anything applied (so the pre-record rebuild folds the change in) and
    /// accumulates the relayout signal.
    fn drain_requests(&mut self, open: &mut OpenDocument, requests: &mut Requests) {
        // Called three times a frame and usually with nothing queued.
        if requests.is_empty() {
            return;
        }
        self.commit_batch(open, requests.drain_document());
    }

    /// Release the canvas's `NodeId`-keyed caches for nodes the document has
    /// stopped holding. Driven by [`App::reconcile_derived_state`] once a
    /// frame — `Editor::frame` runs per *record pass*, so a sweep here would
    /// run twice on a frame carrying action input.
    ///
    /// [`App::reconcile_derived_state`]: crate::gui::app::App
    pub(super) fn reconcile_caches(&mut self, document: &Document) {
        self.main_window.reconcile(document);
    }

    /// Keep the viewer tabs in step with the document by dropping navigation
    /// state whose tab closed.
    fn sync_image_viewers(&mut self, open: &OpenDocument) {
        let layout = &open.document.layout;
        self.main_window
            .image_viewers
            .retain(|port, _| layout.all_tabs().any(|t| t == TabRef::ImageViewer(*port)));
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use palantir::{Key, Modifiers};
    use scenarium::{Func, FuncId, Node, NodeId, NodeKind, testing};

    use crate::core::document::TabRef;
    use crate::core::document::dock::DockOp;
    use crate::core::document::harness::DocFixture;
    use crate::core::edit::intent::types::GraphIntent;
    use crate::gui::app::commands::AppCommand;
    use crate::gui::app::commands::file::FileCommand;
    use crate::gui::app::commands::run::RunCommand;
    use crate::gui::app::editor::harness::EditorHarness;
    use crate::gui::pane::graph::toolbar::internals::run_chip_wid;
    use crate::gui::pane::viewer::ImageViewer;

    fn func_node() -> Node {
        let func = testing::with_stub_lambda(Func::new(FuncId::unique(), "probe"));
        Node::from(&func)
    }

    fn add(node_id: NodeId) -> GraphIntent {
        GraphIntent::AddNode {
            pos: Vec2::ZERO,
            node_id,
            node: func_node(),
            bindings: vec![],
        }
    }

    /// A widget cannot legitimately build a malformed intent — it reads
    /// every identity it emits out of the live document — so one is our own
    /// bug and fails loudly in every build, rather than being reported back
    /// to a caller that no longer exists.
    #[test]
    #[should_panic(expected = "a widget built a malformed intent")]
    fn a_widget_built_malformed_intent_is_a_bug_not_a_refusal() {
        let mut test = EditorHarness::new(DocFixture::default());
        test.apply(GraphIntent::AddNode {
            pos: Vec2::ZERO,
            node_id: NodeId::nil(),
            node: Node::new(NodeKind::Func(FuncId::unique())),
            bindings: vec![],
        });
    }

    /// The exit prompt's signal: content edits flip `dirty`, navigation
    /// doesn't.
    #[test]
    fn dirty_flag_tracks_content_edits_not_navigation() {
        let mut test = EditorHarness::new(DocFixture::default());
        let node_id = NodeId::unique();

        test.apply(add(node_id));
        assert!(test.open.dirty, "adding a node is savable work");

        test.open.dirty = false;
        test.apply(GraphIntent::SetSelection {
            to: [node_id].into_iter().collect(),
        });
        assert!(!test.open.dirty, "selecting is navigation");
    }

    /// Pane arrangement is navigation: the op lands on the layout but
    /// records no undo step and doesn't flip the unsaved flag, so Ctrl+Z
    /// walks straight past it to the last graph edit and quitting after a
    /// rearrangement doesn't prompt.
    #[test]
    fn dock_ops_apply_without_entering_the_undo_history_or_dirtying() {
        let mut test = EditorHarness::new(DocFixture::default());
        let node_id = NodeId::unique();
        test.apply(add(node_id));

        let tab = TabRef::ImageViewer(node_id);
        test.open.dirty = false;
        test.requests.push_view(DockOp::OpenTab { tab });
        test.drain();
        assert!(
            test.open.document.layout.all_tabs().any(|t| t == tab),
            "the viewer tab opened"
        );
        assert!(
            !test.open.dirty,
            "arranging panes is navigation, not savable work"
        );

        // One undo takes back the *node*, not the tab.
        assert!(test.undo(), "the node add is the only entry");
        assert_eq!(test.open.document.graph.len(), 0, "the node came back out");
        assert!(
            test.open.document.layout.all_tabs().any(|t| t == tab),
            "undo leaves the layout alone"
        );
        assert!(!test.undo(), "the dock op recorded nothing of its own");
    }

    /// Two surfaces answering the same frame both reach `App`, in the order
    /// they answered. The single-slot arbitration this replaced kept the
    /// first claim and dropped the rest, so a Ctrl+S that landed on the frame
    /// a run chip was clicked simply did not save.
    #[test]
    fn a_chord_and_a_click_on_one_frame_both_reach_the_app() {
        let mut test = EditorHarness::new(DocFixture::probes(1));
        // Two frames so the toolbar chip has a rect to aim at, and so the
        // Ctrl+S chord is subscribed for palantir's keyboard wake-gate.
        test.prime(2);

        test.ui.set_modifiers(Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        });
        test.ui.key(Key::Char('S'));
        test.ui.click_on(run_chip_wid());

        let commands = test.frame();
        assert!(
            matches!(
                commands[..],
                [
                    AppCommand::File(FileCommand::Save),
                    AppCommand::Run(RunCommand::Once)
                ]
            ),
            "the chord is raised before the record, the chip during it: {commands:?}"
        );
    }

    /// Viewer tabs dedupe per node, and their navigation state is dropped
    /// once the tab closes.
    #[test]
    fn image_viewer_tabs_dedupe_per_node_and_prune_state_on_close() {
        let mut test = EditorHarness::new(DocFixture::default());
        let node_id = NodeId::unique();
        let tab = TabRef::ImageViewer(node_id);

        test.requests.push_view(DockOp::OpenTab { tab });
        test.requests.push_view(DockOp::OpenTab { tab });
        test.drain();
        assert_eq!(
            test.open
                .document
                .layout
                .all_tabs()
                .filter(|t| *t == tab)
                .count(),
            1,
            "one tab per node"
        );

        test.editor
            .main_window
            .image_viewers
            .insert(node_id, ImageViewer::new(node_id));
        test.open.document.layout.apply(DockOp::CloseTab { tab });
        test.editor.sync_image_viewers(&test.open);
        assert!(
            test.editor.main_window.image_viewers.is_empty(),
            "closing the tab drops its navigation state"
        );
    }
}
