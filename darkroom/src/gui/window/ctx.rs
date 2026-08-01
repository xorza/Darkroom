//! The window level of the UI's context chain.

use crate::core::document::Document;
use crate::gui::app::ctx::AppCtx;

/// The frame's read-only world *and* the document it is showing — what every
/// surface at window level reads. `Copy` (an [`AppCtx`], itself shared refs,
/// plus one more), so a phase takes one parameter rather than two that must be
/// kept in step by hand.
///
/// **The middle of the context chain.** [`AppCtx`] above knows nothing about a
/// document; [`GraphCtx`](crate::gui::graph_ctx::GraphCtx) below adds the
/// resolved output types a canvas reads, and derives itself from this one
/// rather than restating what is already here.
///
/// **Composed per phase, never per frame.** The document reference cannot
/// outlive the phase that took it: the session drains the frame's requests
/// into the document between phases, and that mutation needs the document
/// exclusively. So one of these is built at each call into
/// [`MainWindow`](crate::gui::window::MainWindow), over the document as it
/// stands at that instant. It is also why the chain's root stops at `AppCtx`:
/// `App` composes that once for the whole frame, and a document in it would
/// pin the session it lives in for the same span — leaving no drain able to
/// run.
#[derive(Copy, Clone, Debug)]
pub(crate) struct WindowCtx<'a> {
    app: AppCtx<'a>,
    doc: &'a Document,
}

impl<'a> WindowCtx<'a> {
    pub(crate) fn new(app: AppCtx<'a>, doc: &'a Document) -> Self {
        Self { app, doc }
    }

    /// The document this phase is reading — the graph, and the pane
    /// arrangement around it.
    pub(crate) fn document(self) -> &'a Document {
        self.doc
    }

    /// The frame's world without the document: the theme, the func library,
    /// the last run's projections, the status-bar inputs.
    ///
    /// Handed out whole rather than re-exposed accessor by accessor, because
    /// the window is the last level that may name [`AppCtx`] — the status bar
    /// takes one directly, which is how it says it reads nothing
    /// document-shaped. Below here every reader goes through a `GraphCtx`
    /// accessor instead.
    pub(crate) fn app(self) -> AppCtx<'a> {
        self.app
    }
}
