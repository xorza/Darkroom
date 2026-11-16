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

use crate::core::edit::document_request::DocumentRequest;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::open_document::OpenDocument;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::FileCommand;
use crate::gui::frame::app_command::RunCommand;
use crate::gui::frame::doc_ctx::DocCtx;
use crate::gui::frame::framing::Framing;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::stage::Stage;
use crate::gui::main_window::MainWindow;
use palantir::{Shortcut, Ui};
use scenarium::{GraphId, GraphSet, Library};

use crate::gui::frame::app_ctx::AppCtx;
use crate::gui::frame::shortcuts;
use crate::gui::frame::window_parts::WindowParts;

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
    /// The graphs the runtime is compiled and run against — the one reach
    /// across this boundary that is not a frame concern, so it is named here
    /// rather than spelled out at each of `App`'s run commands.
    pub(crate) const fn graphs(&self) -> &GraphSet {
        &self.open.document().graphs
    }

    /// Open `open` in a fresh UI.
    pub(crate) fn new(open: OpenDocument) -> Self {
        Self {
            open,
            main_window: MainWindow::default(),
        }
    }

    /// The frame's input stage, once a frame ahead of the widgets: everything input means against
    /// the document, drained so the pass's record sees it settled. Leaves the app tier queued
    /// in the stage's requests for the shell.
    ///
    /// Each phase ends in a drain, as a phase reads the document and the next one must see what the
    /// previous asked for. So each takes a stage over a `GraphCtx` composed right here, over the
    /// document as the drain before it left it.
    ///
    /// 1. NAVIGATION: settle which tab is active: the undo/redo chords, and tab and chip clicks
    ///    read off last frame's responses, whose hit on a node the undo just removed finds nothing.
    ///    First, so a switched-to tab records in the same present's first pass.
    /// 2. INPUT: reconcile pane visibility, rebuild the canvas's projection, then emit the
    ///    input-derived graph mutations (drag, pan/zoom, wire commit). A canvas that just became
    ///    visible needs a relayout: it may never have recorded, and a dock op raises no geometry
    ///    signal of its own.
    ///
    /// What the stage stranded of the canvas's cached geometry it owes on its requests.
    pub(crate) fn input(&mut self, ui: &mut Ui, stage: &mut Stage<'_, AppCtx<'_>>) {
        debug_assert!(stage.out.is_empty(), "the last frame left requests behind");
        let library = stage.ctx.library();
        self.apply_undo_redo(ui, stage.out);
        self.main_window
            .scan_navigation(ui, &mut stage.with(DocCtx::new(stage.ctx, &self.open)));
        self.drain(library, stage.out);

        self.main_window
            .input(ui, &mut stage.with(DocCtx::new(stage.ctx, &self.open)));
        self.drain(library, stage.out);

        // The file/run/quit chords only queue an `AppCommand`, so they need no drain of their own.
        menu_shortcut(ui, stage.out);
    }

    /// Apply what the frame raised against the document, and owe what that strands. Each route an
    /// edit takes past the frame's drain sits beside this one, so none can leave a debt out.
    fn drain(&mut self, library: &Library, requests: &mut Requests) {
        let facts = self.open.drain_requests(requests.document(), library);
        requests.land(facts);
    }

    /// Apply one edit outside the frame's drain, as a blocking dialog's answer is.
    pub(crate) fn apply_edit(
        &mut self,
        graph: GraphId,
        intent: GraphIntent,
        library: &Library,
        requests: &mut Requests,
    ) {
        self.apply_edits(
            [DocumentRequest::Graph { graph, intent }],
            library,
            requests,
        );
    }

    /// [`Self::apply_edit`] for several edits, of graphs or of the set of them, as one undo
    /// entry.
    pub(crate) fn apply_edits(
        &mut self,
        edits: impl IntoIterator<Item = DocumentRequest>,
        library: &Library,
        requests: &mut Requests,
    ) {
        requests.land(self.open.apply_edits(edits, library));
    }

    /// Take back the last edit.
    ///
    /// The graph it changed comes to the front; the one it returns has no tab to show it.
    pub(crate) fn undo(&mut self, requests: &mut Requests) -> Option<GraphId> {
        let facts = self.open.undo();
        requests.owe_relayout_after(facts);
        self.open.bring_forward(facts.touched)
    }

    /// Replay the last edit taken back, the mirror of [`Self::undo`].
    pub(crate) fn redo(&mut self, requests: &mut Requests) -> Option<GraphId> {
        let facts = self.open.redo();
        requests.owe_relayout_after(facts);
        self.open.bring_forward(facts.touched)
    }

    /// Ready the panes for a pass with no input to read, ahead of its [`Self::record`]: the warmup,
    /// and a second pass, which reads the rects of the pass before it.
    pub(crate) fn settle(&mut self, ui: &Ui, stage: &mut Stage<'_, AppCtx<'_>>) {
        let library = stage.ctx.library();
        self.main_window
            .settle(ui, &mut stage.with(DocCtx::new(stage.ctx, &self.open)));
        self.drain(library, stage.out);
    }

    /// One record pass against `stage`'s context: authors the widget tree, draining what it raises
    /// against the document, and leaves the app tier queued in the requests for the shell.
    ///
    /// What the pass stranded of the canvas's cached geometry it owes on the requests. Resizes
    /// driven by something other than an `UndoStep`, as the header's elapsed-time label growing as a
    /// run reports, are not covered: they leave `CanvasGeometry`'s offsets stale for one frame rather
    /// than buying a pass.
    pub(crate) fn record(
        &mut self,
        ui: &mut Ui,
        stage: &mut Stage<'_, AppCtx<'_>>,
        parts: WindowParts<'_>,
    ) {
        debug_assert!(stage.out.is_empty(), "the last pass left requests behind");
        let library = stage.ctx.library();
        self.main_window.record(
            ui,
            &mut stage.with(DocCtx::new(stage.ctx, &self.open)),
            parts,
        );
        // Graph edits the record surfaced (cache toggle, const edit), plus the tab
        // strip's dock ops.
        self.drain(library, stage.out);
    }

    /// Ctrl+Z / Ctrl+Shift+Z.
    ///
    /// Read every frame, unconditionally: the read also keeps the chord subscribed, and palantir's
    /// keyboard wake-gate delivers an off-focus press only for a chord subscribed last frame.
    /// No focus test: Ctrl+Z is `KeyClass::Edit`, so a focused text field's scope takes it and
    /// the read answers `false` on its own.
    fn apply_undo_redo(&mut self, ui: &mut Ui, requests: &mut Requests) {
        let undo = ui.shortcut_pressed(shortcuts::UNDO);
        let redo = ui.shortcut_pressed(shortcuts::REDO);
        if undo {
            self.undo(requests);
        } else if redo {
            self.redo(requests);
        }
    }

    /// Frame `graph` in its canvas's next input stage; see [`ViewCommand::Frame`].
    ///
    /// [`ViewCommand::Frame`]: crate::gui::frame::app_command::ViewCommand::Frame
    pub(crate) fn frame_graph(&mut self, graph: GraphId, framing: Framing) {
        self.main_window.frame(graph, framing);
    }

    /// Release the canvas's `NodeId`-keyed caches for nodes the document has stopped holding.
    /// Driven once a frame by `App::sweep`: [`Self::record`] runs per record pass, so a sweep
    /// there would run twice on a frame carrying action input.
    pub(super) fn reconcile_caches(&mut self) {
        self.main_window.reconcile(&self.open);
    }
}

/// The global chords and the command each raises. Document file ops fire regardless of focus,
/// so Ctrl+S still saves while a node's const editor is focused (`TextEdit` binds none of these).
/// Palantir matches modifiers exactly, so Ctrl+Shift+S never also reads as Ctrl+S.
const MENU_CHORDS: [(Shortcut, AppCommand); 6] = [
    (shortcuts::NEW, AppCommand::File(FileCommand::New)),
    (shortcuts::OPEN, AppCommand::File(FileCommand::Open)),
    (shortcuts::SAVE_AS, AppCommand::File(FileCommand::SaveAs)),
    (shortcuts::SAVE, AppCommand::File(FileCommand::Save)),
    (shortcuts::RUN, AppCommand::Run(RunCommand::Once)),
    (shortcuts::QUIT, AppCommand::Quit),
];

/// Queue the command of the first [`MENU_CHORDS`] chord that fired, and read them all, so each stays
/// watched for the next frame. Read outside every scope, so a chord a focused field claims does not
/// reach them.
fn menu_shortcut(ui: &mut Ui, requests: &mut Requests) {
    let mut fired = None;
    for (chord, command) in &MENU_CHORDS {
        if ui.shortcut_pressed(*chord) {
            fired.get_or_insert(command);
        }
    }
    if let Some(command) = fired {
        requests.push_app(command.clone());
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
pub(crate) mod internals;

#[cfg(test)]
mod tests;
