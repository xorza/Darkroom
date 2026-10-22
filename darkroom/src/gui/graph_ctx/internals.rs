//! The fixture a test reads a graph back through: everything a [`GraphCtx`]
//! composes, owned together so one can be handed out from a `&mut` borrow.
//!
//! One level up from [`DocFixture`], which owns the document and the library
//! this resolves them against — a test that needs no context at all stops
//! there. Everything that records a canvas builds on this.

use std::sync::Arc;

use scenarium::{Library, NodeId};

use crate::core::document::internals::{self, DocFixture};
use crate::core::open_document::OpenDocument;
use crate::core::wake;
use crate::gui::frame::app_ctx::AppCtx;
use crate::gui::frame::window_ctx::WindowCtx;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// An open document, and the theme / library / run a context resolves it
/// against.
///
/// The document is held the way the session holds it — as an [`OpenDocument`],
/// which is what a [`WindowCtx`] takes: a fixture that kept the bare
/// [`Document`](crate::core::document::Document) could not compose one. It
/// starts saved; a test about the unsaved-changes dot sets
/// [`OpenDocument::dirty`] on it.
#[derive(Debug)]
pub(crate) struct GraphCtxFixture {
    pub(crate) open: OpenDocument,
    pub(crate) library: Arc<Library>,
    pub(crate) run_state: RunState,
    pub(crate) theme: Theme,
}

impl Default for GraphCtxFixture {
    fn default() -> Self {
        Self {
            open: OpenDocument::default(),
            library: Arc::default(),
            run_state: RunState::new(wake::internals::noop()),
            theme: Theme::default(),
        }
    }
}

impl GraphCtxFixture {
    /// Takes a [`DocFixture`] or anything that converts into one — a
    /// [`TestGraph`](scenarium::internals::test_graph::TestGraph) goes straight in.
    pub(crate) fn over(fixture: impl Into<DocFixture>) -> Self {
        let DocFixture { doc, library } = fixture.into();
        Self {
            open: OpenDocument::over(doc),
            library: Arc::new(library),
            ..Self::default()
        }
    }

    /// Give the graph a committed selection.
    pub(crate) fn with_selection(mut self, selected: impl IntoIterator<Item = NodeId>) -> Self {
        self.open.document.main_view.selected.extend(selected);
        self
    }

    /// The `i`th node in placement order — [`DocFixture::node`] over the
    /// document this fixture took ownership of.
    pub(crate) fn node(&self, i: usize) -> NodeId {
        internals::nth_in_paint_order(&self.open.document.main_view, i)
    }

    /// The context over this fixture, derived from an [`AppCtx`] whose
    /// status-bar inputs sit at their empty defaults — no canvas reader
    /// sees them.
    pub(crate) fn graph_ctx(&self) -> GraphCtx<'_> {
        let Self {
            open,
            library,
            run_state,
            theme,
        } = self;
        let app = AppCtx::new(theme, library, run_state);
        GraphCtx::new(WindowCtx::new(app, open))
    }
}
