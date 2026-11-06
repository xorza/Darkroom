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

use crate::core::open_document::OpenDocument;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::FileCommand;
use crate::gui::frame::app_command::RunCommand;
use crate::gui::frame::input_end::InputEnd;
use crate::gui::frame::relayout::Relayout;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::window_ctx::WindowCtx;
use crate::gui::main_window::MainWindow;
use crate::gui::pane::graph_ui::gesture::pan_zoom::Framing;
use palantir::{Shortcut, Ui, UpdateCtx};
use scenarium::Graph;
use std::mem;

use crate::gui::frame::app_ctx::AppCtx;
use crate::gui::frame::frame_writes::FrameWrites;
use crate::gui::frame::shortcuts;

#[derive(Debug)]
pub(crate) struct Session {
    /// Whether an input stage ran since the last record pass, so the next pass needs no settling.
    input_ran: bool,
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
    pub(crate) const fn graph(&self) -> &Graph {
        &self.open.document.graph
    }

    /// Open `open` in a fresh UI.
    pub(crate) fn new(open: OpenDocument) -> Self {
        Self {
            input_ran: false,
            open,
            main_window: MainWindow::default(),
        }
    }

    /// The frame's input stage, once a frame before any record: everything input means against
    /// the document, drained so the first record pass sees it settled. Leaves the app tier queued
    /// in `requests` for the shell.
    ///
    /// Each phase ends in a drain, as a phase reads the document and the next one must see what the
    /// previous asked for. So each takes a `WindowCtx` composed right here, over the document as
    /// the drain before it left it.
    ///
    /// 1. NAVIGATION: settle which tab is active: the undo/redo chords, and tab and chip clicks
    ///    read off last frame's responses, whose hit on a node the undo just removed finds nothing.
    ///    First, so a switched-to tab records in the same present's first pass.
    /// 2. PREPASS: reconcile pane visibility, rebuild the canvas's projection, then emit the
    ///    input-derived graph mutations (drag, pan/zoom, wire commit). A canvas that just became
    ///    visible needs a relayout: it may never have recorded, and a dock op raises no geometry
    ///    signal of its own.
    ///
    /// Returns what the frame's records may read of the input, and whether the stage stranded the
    /// canvas's cached geometry, which `App` spends in its record, so the whole app requests a
    /// relayout from exactly one place.
    #[must_use]
    pub(crate) fn input(
        &mut self,
        update_ctx: &mut UpdateCtx<'_>,
        ctx: AppCtx<'_>,
        requests: &mut Requests,
    ) -> InputEnd {
        debug_assert!(requests.is_empty(), "the last frame left requests behind");
        let mut needs_relayout = self.apply_undo_redo(update_ctx);
        self.main_window
            .scan_navigation(update_ctx, WindowCtx::new(ctx, &self.open), requests);
        needs_relayout |=
            Relayout::after(self.open.drain_requests(requests.document(), ctx.library()));

        let end = self
            .main_window
            .prepass(update_ctx, WindowCtx::new(ctx, &self.open), requests);
        needs_relayout |= end.relayout;
        needs_relayout |=
            Relayout::after(self.open.drain_requests(requests.document(), ctx.library()));

        // The file/run/quit chords only queue an `AppCommand`, so they need no drain of their own.
        menu_shortcut(update_ctx, requests);
        self.input_ran = true;
        InputEnd {
            outcome: end.outcome,
            relayout: needs_relayout,
        }
    }

    /// One record pass against `ctx`: authors the widget tree, draining what it raises against the
    /// document, and leaves the app tier queued in the `writes` requests for the shell.
    ///
    /// A pass that no input stage ran just before settles first: the one past the first frame's
    /// warmup, and a second pass, each of which reads the rects of the pass before it.
    ///
    /// Returns whether the pass stranded the canvas's cached geometry. Resizes driven by something
    /// other than an `UndoStep`, as the header's elapsed-time label growing as a run reports, are
    /// not covered: they leave `CanvasGeometry`'s offsets stale for one frame rather than buying a
    /// pass.
    #[must_use]
    pub(crate) fn record(
        &mut self,
        ui: &mut Ui,
        ctx: AppCtx<'_>,
        writes: FrameWrites<'_>,
    ) -> Relayout {
        let FrameWrites {
            preferences,
            requests,
        } = writes;
        debug_assert!(requests.is_empty(), "the last pass left requests behind");
        let mut needs_relayout = Relayout::NotNeeded;
        if !mem::take(&mut self.input_ran) {
            needs_relayout |=
                self.main_window
                    .settle(ui, WindowCtx::new(ctx, &self.open), requests);
            needs_relayout |=
                Relayout::after(self.open.drain_requests(requests.document(), ctx.library()));
        }
        self.main_window.frame(
            ui,
            WindowCtx::new(ctx, &self.open),
            FrameWrites {
                preferences,
                requests,
            },
        );
        // Graph edits the record surfaced (node select, cache toggle, const edit), plus the tab
        // strip's dock ops.
        needs_relayout |=
            Relayout::after(self.open.drain_requests(requests.document(), ctx.library()));
        needs_relayout
    }

    /// Ctrl+Z / Ctrl+Shift+Z. Replays undo/redo against the document
    /// (each entry carries its own graph target).
    ///
    /// The chords are read *every frame, unconditionally* — that call both
    /// reads the press and keeps the chord subscribed, and palantir's
    /// keyboard wake-gate only delivers an off-focus press when its chord
    /// was subscribed last frame (subscriptions clear each frame).
    ///
    /// No focus test: Ctrl+Z is `KeyClass::Edit`, so while a text field
    /// holds focus palantir grants it to that field's scope and this
    /// read answers `false` on its own.
    #[must_use]
    fn apply_undo_redo(&mut self, update_ctx: &mut UpdateCtx<'_>) -> Relayout {
        let undo = update_ctx.shortcut_pressed(shortcuts::UNDO);
        let redo = update_ctx.shortcut_pressed(shortcuts::REDO);
        // The document owns its history and what a replay means; this layer
        // only says which direction the chord asked for.
        if undo {
            Relayout::after(self.open.undo())
        } else if redo {
            Relayout::after(self.open.redo())
        } else {
            Relayout::NotNeeded
        }
    }

    /// Frame the graph in the canvas's next prepass; see [`ViewCommand::Frame`].
    ///
    /// [`ViewCommand::Frame`]: crate::gui::frame::app_command::ViewCommand::Frame
    pub(crate) const fn frame_graph(&mut self, framing: Framing) {
        self.main_window.graph_ui.request_framing(framing);
    }

    /// Release the canvas's `NodeId`-keyed caches for nodes the document has
    /// stopped holding. Driven by `App::update` once a
    /// frame — [`Self::frame`] runs per *record pass*, so a sweep here would
    /// run twice on a frame carrying action input.
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

/// Queue the command of the first [`MENU_CHORDS`] chord that fired. The search stops at the
/// first match, and the chords after it stay watched: palantir records the frame again with no
/// input, and that pass reads them all (`Ui::shortcut_pressed`). Read outside every scope, so a
/// chord a focused field claims does not reach them.
fn menu_shortcut(update_ctx: &mut UpdateCtx<'_>, requests: &mut Requests) {
    if let Some((_, command)) = MENU_CHORDS
        .iter()
        .find(|(chord, _)| update_ctx.shortcut_pressed(*chord))
    {
        requests.push_app(command.clone());
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
pub(crate) mod internals;

#[cfg(test)]
mod tests;
