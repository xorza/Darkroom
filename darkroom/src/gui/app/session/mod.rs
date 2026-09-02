//! An editing session: the open document and the UI showing it, plus the
//! per-frame pipeline that runs one against the other.
//!
//! The two are one unit — opening a different file replaces both, and a UI
//! that outlived its document would hold gesture state and cached geometry
//! keyed to nodes that no longer exist. Owning them together is also what lets
//! the pipeline run without a document borrow crossing into it: every mutation
//! is a call on [`OpenDocument`], which the session holds, and the UI below
//! ([`MainWindow`]) only ever reads a `&Document`.
//!
//! So the layering reads in one direction. [`App`] owns the session and the
//! runtime around it; the session decides *what to ask for and when*; the
//! document decides what an edit *means*; the UI decides what to *draw* and
//! what to ask for next.
//!
//! [`App`]: crate::gui::app::App

use crate::core::document::open_document::OpenDocument;
use crate::core::io::preferences::Preferences;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::file::FileCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::relayout::Relayout;
use crate::gui::requests::Requests;
use crate::gui::window::MainWindow;
use crate::gui::window::ctx::WindowCtx;
use palantir::{Shortcut, Ui};
use scenarium::Graph;

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
pub(crate) struct Session {
    /// The document being edited, its save path, and its undo history.
    pub(crate) open: OpenDocument,
    /// The panes showing it. Reset with the document rather than kept across
    /// one: its gesture state and `NodeId`-keyed caches only mean anything
    /// against the graph they were built from.
    main_window: MainWindow,
}

impl Session {
    /// The graph the runtime is compiled and run against — the one reach
    /// across this boundary that is not a frame concern, so it is named here
    /// rather than spelled out at each of `App`'s run commands.
    pub(crate) fn graph(&self) -> &Graph {
        &self.open.document.graph
    }

    /// Open `open` in a fresh UI.
    pub(crate) fn new(open: OpenDocument) -> Self {
        Self {
            open,
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
        ctx: AppCtx<'_>,
        preferences: &mut Preferences,
        requests: &mut Requests,
    ) -> Relayout {
        // The frame's relayout accumulator, owned here for exactly as long as
        // the frame it describes. Every pass that can strand
        // `CanvasGeometry`'s cross-frame caches reports upward into it, and it
        // is handed to `App` to spend — so there is no flag to reset, and none
        // to leak into the next frame.
        //
        // Three phases, each ending in a drain. That is the shape because a
        // phase reads the document and the next one must see what the previous
        // asked for; the drain between them is a document mutation, which is
        // why no two adjacent phases can collapse into one call on the UI.
        //
        // Each phase takes a `WindowCtx` composed right here, over the document
        // as the drain before it left it. That is the level of the context
        // chain carrying a document, and this is why it cannot be composed
        // once for the frame: the drains need the document exclusively, so a
        // longer-lived context would have nothing able to run between phases.
        //
        // 1. NAVIGATION — settle which tab is active, entirely from inputs
        //    available before the record: the undo/redo chords, and tab and
        //    chip clicks read off *last* frame's responses. Those responses
        //    are last frame's while the document they resolve against is this
        //    frame's, so a hit on a node the undo just removed simply finds
        //    nothing. It runs first so a switched-to tab records in the same
        //    present's Pass A, with no first-frame gap. A tab whose node the
        //    undo just removed is pruned by the mutation itself — see
        //    `OpenDocument::land`.
        let mut needs_relayout = self.apply_undo_redo(ui);
        self.main_window
            .scan_navigation(ui, WindowCtx::new(ctx, &self.open), requests);
        needs_relayout |= self.open.drain_requests(requests);

        // 2. PREPASS — reconcile pane visibility, rebuild the canvas's
        //    projection, then emit the input-derived graph mutations (drag,
        //    pan/zoom, connection commit). Drained before the record so Pass A
        //    sees the settled doc. Driven by the panes on screen, like the
        //    record below — a pane kind that grows input handling gets an arm
        //    there rather than another question here. A canvas that just
        //    became visible needs a relayout: it may never have recorded, and
        //    a dock op raises no geometry signal of its own.
        needs_relayout |= self
            .main_window
            .prepass(ui, WindowCtx::new(ctx, &self.open), requests);
        needs_relayout |= self.open.drain_requests(requests);

        // 3. RECORD — author the widget tree. The file/run/quit chords are
        //    read just ahead of it: unlike undo/redo they only queue an
        //    `AppCommand`, so they need no drain of their own and simply have
        //    to land before `App` takes the tier.
        self.menu_shortcut(ui, requests);
        self.main_window
            .frame(ui, WindowCtx::new(ctx, &self.open), preferences, requests);
        // Graph edits the record surfaced (node select, cache toggle, const
        // edit), plus the tab strip's dock ops.
        needs_relayout |= self.open.drain_requests(requests);

        // Resizes driven by something other than an `UndoStep` — the header's
        // elapsed-time label growing as a run reports — are not covered: they
        // leave `CanvasGeometry`'s offsets stale for one frame rather than
        // buying a pass.
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
    fn apply_undo_redo(&mut self, ui: &mut Ui) -> Relayout {
        let undo = ui.key_pressed(UNDO_SHORTCUT);
        let redo = ui.key_pressed(REDO_SHORTCUT);
        // The document owns its history and what a replay means; this layer
        // only says which direction the chord asked for.
        if undo {
            self.open.undo().relayout
        } else if redo {
            self.open.redo().relayout
        } else {
            Relayout::NotNeeded
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
            AppCommand::File(FileCommand::Open)
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
    /// stopped holding. Driven by `App::update` once a
    /// frame — [`Self::frame`] runs per *record pass*, so a sweep here would
    /// run twice on a frame carrying action input.
    pub(super) fn reconcile_caches(&mut self) {
        self.main_window.reconcile(&self.open.document);
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use palantir::{DockOp, Key, Modifiers};
    use scenarium::{Func, FuncId, Node, NodeId, NodeKind, testing};

    use crate::alloc_audit;
    use crate::core::document::TabRef;
    use crate::core::document::harness::DocFixture;
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::core::preview::preview_func;
    use crate::gui::app::commands::AppCommand;
    use crate::gui::app::commands::file::FileCommand;
    use crate::gui::app::commands::run::RunCommand;
    use crate::gui::app::session::harness::SessionHarness;
    use crate::gui::pane::graph::node::preview_row::preview_image_wid;
    use crate::gui::pane::graph::toolbar::internals::run_chip_wid;
    use crate::gui::pane::viewer::ImageViewer;
    use crate::gui::state::preview_store::StoredContent;
    use crate::gui::state::preview_store::internals::opaque_image_value;

    /// Frames to settle the scene before the window opens. The caches that
    /// grow once — text shaping, palantir's widget tables, the record store's
    /// arenas — do it inside these, so what the window sees is steady state.
    const SETTLE_FRAMES: u32 = 32;
    /// Long enough that a once-every-N-frames allocation lands inside the
    /// window rather than after it.
    const AUDITED_FRAMES: usize = 64;

    /// The record path performs no heap operation once the editor has settled.
    ///
    /// Strict zero, because every allocation on this path would be the
    /// editor's own: a `format!` for a label palantir was going to copy into
    /// its text arena anyway, or a `collect()` for a list that could have
    /// refilled a buffer the editor already owns. Both have appeared here
    /// before, and neither shows up in a profile — one frame's worth is far
    /// too small to see, and the cost is the tail it puts on the frames that
    /// happen to trip the allocator.
    ///
    /// The scene is the one the record path is widest over: a graph pane of
    /// nodes, a preview card holding an image, and the status bar's memory
    /// readout, which recomputes its whole line every frame.
    ///
    /// Each frame is audited on its own rather than summed, so a
    /// grow-on-the-Nth-frame allocation — a `Vec` doubling, a map rehash —
    /// fails on the frame that performed it.
    #[test]
    fn a_settled_frame_records_without_allocating() {
        let mut fixture = DocFixture::probes(6);
        let node = fixture.add(&preview_func(Default::default()));
        let mut test = SessionHarness::new(fixture);
        test.run_state
            .previews
            .ingest_preview(test.ui.ui(), node, opaque_image_value());
        // A reading, so the memory readout renders its longest form rather
        // than the absent-figure path.
        test.process_memory = 3 * 1024 * 1024;
        test.prime(SETTLE_FRAMES);

        for frame in 0..AUDITED_FRAMES {
            let allocations = alloc_audit::allocations(|| {
                let _ = test.frame();
            });
            assert_eq!(
                allocations, 0,
                "settled frame {frame} performed {allocations} heap operations"
            );
        }
    }

    /// Opening a viewer by clicking a preview card lands its tab *inside* the
    /// record that read the click. The tab is drawn by the pass after that one
    /// — a click makes the frame record twice — and a viewer uploads its
    /// full-resolution texture as it draws, so the image is on screen in the
    /// same frame the click opened it.
    #[test]
    fn a_viewer_opened_by_click_shows_its_image_in_that_same_frame() {
        let mut fixture = DocFixture::default();
        let node = fixture.add(&preview_func(Default::default()));
        let mut test = SessionHarness::new(fixture);
        test.run_state
            .previews
            .ingest_preview(test.ui.ui(), node, opaque_image_value());
        test.prime(2);

        let resident = |test: &SessionHarness| {
            let Some(StoredContent::Image(image)) = test.run_state.previews.entries.get(&node)
            else {
                panic!("the ingested image is the node's stored content");
            };
            image.is_full_resident()
        };
        assert!(
            !resident(&test),
            "a card-only preview never uploads its full-resolution source"
        );

        test.ui.click_on(preview_image_wid(node));
        let _ = test.frame();
        assert!(
            test.session
                .open
                .document
                .layout
                .all_tabs()
                .any(|t| t == TabRef::ImageViewer(node)),
            "the click opened the viewer tab within that same frame"
        );
        assert!(
            resident(&test),
            "and the pass that drew the new tab uploaded its texture — no \
             placeholder, and no waiting for the next frame"
        );
    }

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
        let mut test = SessionHarness::new(DocFixture::default());
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
        let mut test = SessionHarness::new(DocFixture::default());
        let node_id = NodeId::unique();

        test.apply(add(node_id));
        assert!(test.session.open.dirty, "adding a node is savable work");

        test.session.open.dirty = false;
        test.apply(GraphIntent::SetSelection {
            to: [node_id].into_iter().collect(),
        });
        assert!(!test.session.open.dirty, "selecting is navigation");
    }

    /// Pane arrangement is navigation: the op lands on the layout but
    /// records no undo step and doesn't flip the unsaved flag, so Ctrl+Z
    /// walks straight past it to the last graph edit and quitting after a
    /// rearrangement doesn't prompt.
    #[test]
    fn dock_ops_apply_without_entering_the_undo_history_or_dirtying() {
        let mut test = SessionHarness::new(DocFixture::default());
        let node_id = NodeId::unique();
        test.apply(add(node_id));

        let tab = TabRef::ImageViewer(node_id);
        test.session.open.dirty = false;
        test.requests.push_view(DockOp::OpenTab { tab });
        test.drain();
        assert!(
            test.session
                .open
                .document
                .layout
                .all_tabs()
                .any(|t| t == tab),
            "the viewer tab opened"
        );
        assert!(
            !test.session.open.dirty,
            "arranging panes is navigation, not savable work"
        );

        // One undo takes back the *node*, not the tab.
        assert!(test.undo(), "the node add is the only entry");
        assert_eq!(
            test.session.open.document.graph.len(),
            0,
            "the node came back out"
        );
        assert!(
            test.session
                .open
                .document
                .layout
                .all_tabs()
                .any(|t| t == tab),
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
        let mut test = SessionHarness::new(DocFixture::probes(1));
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
        // A node the graph actually holds: a viewer tab names the preview node
        // whose value it shows, and the drain below prunes a tab whose node is
        // gone — as it must, since that is what closes a viewer when its node
        // is deleted.
        let fixture = DocFixture::probes(1);
        let node_id = fixture.node(0);
        let mut test = SessionHarness::new(fixture);
        let tab = TabRef::ImageViewer(node_id);

        test.requests.push_view(DockOp::OpenTab { tab });
        test.requests.push_view(DockOp::OpenTab { tab });
        test.drain();
        assert_eq!(
            test.session
                .open
                .document
                .layout
                .all_tabs()
                .filter(|t| *t == tab)
                .count(),
            1,
            "one tab per node"
        );

        test.session
            .main_window
            .image_viewers
            .insert(node_id, ImageViewer::new(node_id));
        test.session
            .open
            .document
            .layout
            .apply(DockOp::CloseTab { tab });
        test.session.reconcile_caches();
        assert!(
            test.session.main_window.image_viewers.is_empty(),
            "closing the tab drops its navigation state"
        );
    }
}
