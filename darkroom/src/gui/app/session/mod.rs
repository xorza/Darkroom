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
use crate::gui::frame::relayout::Relayout;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::window_ctx::WindowCtx;
use crate::gui::main_window::MainWindow;
use crate::gui::pane::graph_ui::gesture::pan_zoom::Framing;
use palantir::Ui;
use scenarium::Graph;

use crate::gui::frame::app_ctx::AppCtx;
use crate::gui::frame::frame_writes::FrameWrites;
use crate::gui::frame::shortcuts;

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
    pub(crate) const fn graph(&self) -> &Graph {
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
    /// and leaving the app tier queued in the `writes` requests for the shell.
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
        writes: FrameWrites<'_>,
    ) -> Relayout {
        let FrameWrites {
            preferences,
            requests,
        } = writes;
        // Each pass drains the document tier and its caller the app tier, so a second pass of the
        // frame inherits nothing the first ran.
        debug_assert!(requests.is_empty(), "the last pass left requests behind");
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
        needs_relayout |=
            Relayout::after(self.open.drain_requests(requests.document(), ctx.library()));

        // 2. PREPASS — reconcile pane visibility, rebuild the canvas's
        //    projection, then emit the input-derived graph mutations (drag,
        //    pan/zoom, wire commit). Drained before the record so Pass A
        //    sees the settled document. Driven by the panes on screen, like the
        //    record below — a pane kind that grows input handling gets an arm
        //    there rather than another question here. A canvas that just
        //    became visible needs a relayout: it may never have recorded, and
        //    a dock op raises no geometry signal of its own.
        needs_relayout |= self
            .main_window
            .prepass(ui, WindowCtx::new(ctx, &self.open), requests);
        needs_relayout |=
            Relayout::after(self.open.drain_requests(requests.document(), ctx.library()));

        // 3. RECORD — author the widget tree. The file/run/quit chords are
        //    read just ahead of it: unlike undo/redo they only queue an
        //    `AppCommand`, so they need no drain of their own and simply have
        //    to land before `App` takes the tier.
        menu_shortcut(ui, requests);
        self.main_window.frame(
            ui,
            WindowCtx::new(ctx, &self.open),
            FrameWrites {
                preferences,
                requests,
            },
        );
        // Graph edits the record surfaced (node select, cache toggle, const
        // edit), plus the tab strip's dock ops.
        needs_relayout |=
            Relayout::after(self.open.drain_requests(requests.document(), ctx.library()));

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
        let undo = ui.shortcut_pressed(shortcuts::UNDO);
        let redo = ui.shortcut_pressed(shortcuts::REDO);
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

/// Queue the [`AppCommand`] for whichever of Ctrl+N / Ctrl+O / Ctrl+S /
/// Ctrl+Shift+S / Ctrl+R / Ctrl+Q fired.
///
/// Document file ops are **global** — they fire regardless of
/// focus, so Ctrl+S still saves while a node's const editor is
/// focused (`TextEdit` doesn't bind S/O/N, so nothing is stolen).
/// Every chord is sampled with `key_pressed` each frame so all
/// stay subscribed for palantir's wake-gate (sampling them all up
/// front, not short-circuited, so one chord firing doesn't drop
/// the others' subscription that frame). Palantir matches modifiers
/// exactly, so Ctrl+Shift+S never also reads as Ctrl+S.
fn menu_shortcut(ui: &mut Ui, requests: &mut Requests) {
    let new = ui.shortcut_pressed(shortcuts::NEW);
    let open = ui.shortcut_pressed(shortcuts::OPEN);
    let save_as = ui.shortcut_pressed(shortcuts::SAVE_AS);
    let save = ui.shortcut_pressed(shortcuts::SAVE);
    let run = ui.shortcut_pressed(shortcuts::RUN);
    let quit = ui.shortcut_pressed(shortcuts::QUIT);
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

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
pub(crate) mod internals;

#[cfg(test)]
mod tests;
