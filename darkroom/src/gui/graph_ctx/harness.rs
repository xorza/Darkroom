//! The fixture a test reads a graph back through: everything a [`GraphCtx`]
//! composes, owned together so one can be handed out from a `&mut` borrow.
//!
//! One level up from [`DocFixture`], which owns the document and the library
//! this resolves them against — a test that needs no context at all stops
//! there. Everything that records a canvas builds on this.

use scenarium::{NodeId, OutputTypes};

use crate::core::document::harness::DocFixture;
use crate::gui::app::ctx::{AppCtx, StatusInputs};
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;
use crate::gui::window::ctx::WindowCtx;

/// A document, and the theme / library / run a context resolves it against.
///
/// The output-type table starts empty: [`Self::graph_ctx`] resolves it, the
/// same way composing one does anywhere else — which is why that method takes
/// `&mut self`.
#[derive(Debug, Default)]
pub(crate) struct GraphCtxFixture {
    pub(crate) fixture: DocFixture,
    pub(crate) run_state: RunState,
    pub(crate) theme: Theme,
    output_types: OutputTypes,
}

impl GraphCtxFixture {
    /// Takes a [`DocFixture`] or anything that converts into one — a
    /// [`TestGraph`](scenarium::testing::graph::TestGraph) goes straight in.
    pub(crate) fn over(fixture: impl Into<DocFixture>) -> Self {
        Self {
            fixture: fixture.into(),
            ..Self::default()
        }
    }

    /// Give the graph a committed selection.
    pub(crate) fn with_selection(mut self, selected: impl IntoIterator<Item = NodeId>) -> Self {
        self.fixture.doc.main_view.selected.extend(selected);
        self
    }

    /// The context over this fixture, derived from an [`AppCtx`] whose
    /// status-bar inputs sit at their empty defaults — no canvas reader
    /// sees them.
    pub(crate) fn graph_ctx(&mut self) -> GraphCtx<'_> {
        let Self {
            fixture,
            run_state,
            theme,
            output_types,
        } = self;
        let app = AppCtx::new(theme, &fixture.library, run_state, StatusInputs::default());
        GraphCtx::new(WindowCtx::new(app, &fixture.doc), output_types)
    }
}
