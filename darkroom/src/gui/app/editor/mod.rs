//! The per-frame GUI edit pipeline over a borrowed [`OpenDocument`].
//!
//! `Editor` owns the GUI tree and transient gesture state. The canvas's own
//! projection of the graph lives with the canvas that draws it, and every
//! *mutation* lives on [`OpenDocument`] — this layer decides what to ask for
//! and when, never what an edit means. [`App`] lends it the open document for
//! each operation and the frame's [`AppCtx`] to read the rest through, keeping
//! document, runtime and run-state ownership on the shell.
//!
//! [`App`]: crate::gui::app::App

use crate::core::document::Document;
use crate::core::document::open_document::OpenDocument;
use crate::core::io::preferences::Preferences;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::file::FileCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::requests::Requests;
use crate::gui::window::MainWindow;
use palantir::{Shortcut, Ui};

use crate::gui::app::ctx::AppCtx;

#[cfg(test)]
pub(crate) mod harness;

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

#[derive(Debug)]
pub(crate) struct Editor {
    main_window: MainWindow,
}

impl Editor {
    /// Build fresh GUI editing state for an open document.
    pub(crate) fn new() -> Self {
        Self {
            main_window: MainWindow::default(),
        }
    }

    /// Run one frame of the edit pipeline against `ctx` — the frame's
    /// read-only world — draining everything it raises against the document
    /// and leaving the app tier queued in `requests` for the shell.
    ///
    /// The frame splits into a **navigation phase** (settle which tab is
    /// active, from frame-top inputs) and an **edit phase** (mutate the
    /// graph), because input that switches tabs comes from *last* frame's
    /// click responses and must resolve before anything edits or records.
    ///
    /// Returns whether the pass stranded the canvas's cached geometry. `App`
    /// spends it once the command tier has run too, so the whole app requests
    /// a relayout from exactly one place.
    #[must_use]
    pub(crate) fn frame(
        &mut self,
        ui: &mut Ui,
        open: &mut OpenDocument,
        ctx: AppCtx<'_>,
        preferences: &mut Preferences,
        requests: &mut Requests,
    ) -> bool {
        requests.clear();

        // The frame's relayout accumulator, owned here for exactly as long as
        // the frame it describes. Every pass that can strand
        // `CanvasGeometry`'s cross-frame caches reports upward into it, and it
        // is handed to `App` to spend — so there is no flag to reset, and none
        // to leak into the next frame.
        //
        // Settle the active tab entirely from frame-top inputs (keyboard
        // undo/redo + last-frame click responses). `navigate` reads *last*
        // frame's projection to resolve tab/chip clicks, so it must run
        // before this frame's rebuild. After it, the active tab is fixed.
        let mut needs_relayout = self.navigate(ui, open, ctx, requests);

        // Prepass reconciles pane visibility, rebuilds the canvas's
        // projection, then emits input-derived graph mutations (drag,
        // pan/zoom, connection commit) drained *before* the record so Pass A
        // sees the settled doc. Driven by the panes on screen, like the record
        // pass below — a pane kind that grows input handling gets an arm there
        // rather than another question here. A canvas that just became
        // visible needs a relayout: it may never have recorded, and a dock op
        // raises no geometry signal of its own.
        needs_relayout |= self.main_window.prepass(ui, ctx, &open.document, requests);
        needs_relayout |= open.drain_requests(requests);

        self.menu_shortcut(ui, requests);
        self.main_window
            .frame(ui, ctx, &open.document, preferences, requests);

        // Post-record drain — graph edits the record surfaced (node select,
        // cache toggle, const edit), plus the tab strip's dock ops.
        needs_relayout |= open.drain_requests(requests);

        // Resizes driven by something other than an `UndoStep` — the header's
        // elapsed-time label growing as a run reports — are not covered: they
        // leave `CanvasGeometry`'s offsets stale for one frame rather than
        // buying a pass.
        needs_relayout
    }

    /// Settle which tab is active for this frame, from inputs all available
    /// before the record: keyboard undo/redo and tab clicks read from *last*
    /// frame's responses.
    ///
    /// Done up front so the edit pipeline runs against a settled document
    /// and a switched-to tab records in the same present's Pass A.
    #[must_use]
    fn navigate(
        &mut self,
        ui: &mut Ui,
        open: &mut OpenDocument,
        ctx: AppCtx<'_>,
        requests: &mut Requests,
    ) -> bool {
        let mut needs_relayout = self.apply_undo_redo(ui, open);
        // Surface tab clicks from last frame's responses. Those responses are
        // last frame's; the document they resolve against is this frame's,
        // so a hit on a node the undo above removed simply finds nothing.
        self.main_window
            .scan_navigation(ui, ctx, &open.document, requests);
        // Dock ops apply straight to the layout — drain them.
        needs_relayout |= open.drain_requests(requests);
        // A tab whose node is gone can't stay open.
        open.document.reconcile_with_graph();
        needs_relayout
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
    #[must_use]
    fn apply_undo_redo(&mut self, ui: &mut Ui, open: &mut OpenDocument) -> bool {
        let undo = ui.key_pressed(UNDO_SHORTCUT);
        let redo = ui.key_pressed(REDO_SHORTCUT);
        // The document owns its history and what a replay means; this layer
        // only says which direction the chord asked for.
        if undo {
            open.undo().geometry_stale
        } else if redo {
            open.redo().geometry_stale
        } else {
            false
        }
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

    /// Release the canvas's `NodeId`-keyed caches for nodes the document has
    /// stopped holding. Driven by [`App::reconcile_derived_state`] once a
    /// frame — `Editor::frame` runs per *record pass*, so a sweep here would
    /// run twice on a frame carrying action input.
    ///
    /// [`App::reconcile_derived_state`]: crate::gui::app::App
    pub(super) fn reconcile_caches(&mut self, document: &Document) {
        self.main_window.reconcile(document);
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
        test.editor.reconcile_caches(&test.open.document);
        assert!(
            test.editor.main_window.image_viewers.is_empty(),
            "closing the tab drops its navigation state"
        );
    }
}
