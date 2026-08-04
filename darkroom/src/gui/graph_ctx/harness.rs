//! The fixture a test reads a graph back through: everything a [`GraphCtx`]
//! composes, owned together so one can be handed out from a `&mut` borrow.
//!
//! One level up from [`DocFixture`], which owns the document and the library
//! this resolves them against — a test that needs no context at all stops
//! there. Everything that records a canvas builds on this.

use scenarium::{Library, NodeId, OutputTypes};

use crate::core::document::harness::DocFixture;
use crate::core::document::open_document::OpenDocument;
use crate::gui::app::ctx::{AppCtx, StatusInputs};
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;
use crate::gui::window::ctx::WindowCtx;

/// An open document, and the theme / library / run a context resolves it
/// against.
///
/// The document is held the way the session holds it — as an [`OpenDocument`],
/// which is what a [`WindowCtx`] takes: a fixture that kept the bare
/// [`Document`](crate::core::document::Document) could not compose one. It
/// starts saved; a test about the unsaved-changes dot sets
/// [`OpenDocument::dirty`] on it.
///
/// The output-type table starts empty: [`Self::graph_ctx`] resolves it, the
/// same way composing one does anywhere else — which is why that method takes
/// `&mut self`.
#[derive(Debug, Default)]
pub(crate) struct GraphCtxFixture {
    pub(crate) open: OpenDocument,
    pub(crate) library: Library,
    pub(crate) run_state: RunState,
    pub(crate) theme: Theme,
    output_types: OutputTypes,
}

impl GraphCtxFixture {
    /// Takes a [`DocFixture`] or anything that converts into one — a
    /// [`TestGraph`](scenarium::testing::graph::TestGraph) goes straight in.
    pub(crate) fn over(fixture: impl Into<DocFixture>) -> Self {
        let DocFixture { doc, library } = fixture.into();
        Self {
            open: OpenDocument::over(doc),
            library,
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
        self.open
            .document
            .main_view
            .paint_order()
            .get(i)
            .expect("the fixture placed that many nodes")
            .0
    }

    /// The context over this fixture, derived from an [`AppCtx`] whose
    /// status-bar inputs sit at their empty defaults — no canvas reader
    /// sees them.
    pub(crate) fn graph_ctx(&mut self) -> GraphCtx<'_> {
        let Self {
            open,
            library,
            run_state,
            theme,
            output_types,
        } = self;
        let app = AppCtx::new(theme, library, run_state, StatusInputs::default());
        GraphCtx::new(WindowCtx::new(app, open), output_types)
    }
}
