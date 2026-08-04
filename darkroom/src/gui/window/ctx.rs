//! The window level of the UI's context chain.

use crate::core::document::Document;
use crate::core::document::open_document::OpenDocument;
use crate::gui::app::ctx::AppCtx;

/// The frame's read-only world *and* the open document it is showing — what
/// every surface at window level reads. `Copy` (an [`AppCtx`], itself shared
/// refs, plus one more), so a phase takes one parameter rather than two that
/// must be kept in step by hand.
///
/// It takes the [`OpenDocument`] rather than the bare [`Document`] because the
/// window shows both halves of it: the graph and its pane arrangement, and the
/// unsaved-changes flag — a fact about the document *and* the file behind it,
/// which is why no `Document` carries one. Read-only, so nothing at this level
/// can reach the edit history the pair also holds.
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
    open: &'a OpenDocument,
}

impl<'a> WindowCtx<'a> {
    pub(crate) fn new(app: AppCtx<'a>, open: &'a OpenDocument) -> Self {
        Self { app, open }
    }

    /// The document this phase is reading — the graph, and the pane
    /// arrangement around it. The projection nearly every reader wants; the
    /// tab strip, which also shows [`OpenDocument::dirty`], takes the pair
    /// through [`Self::open`] instead.
    pub(crate) fn document(self) -> &'a Document {
        &self.open.document
    }

    /// The open document whole — its content *and* the file it came from.
    pub(crate) fn open(self) -> &'a OpenDocument {
        self.open
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
