//! Fixtures every canvas test drives a real frame through: the context chain
//! over a document, and the assertions about what a pass emitted.
//!
//! Shared rather than per-file because the subject under test is the same
//! machinery in each case — a `UiHarness` frame over a `GraphUI` — and only
//! the gesture being driven differs. The document those frames run over, and
//! the libraries it resolves against, come from
//! [`crate::core::document::harness`].

use scenarium::{Library, OutputTypes};

use crate::core::document::Document;
use crate::core::edit::intent::sink::Queued;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::app::ctx::{AppCtx, StatusInputs};
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// The chain's root for a canvas test: the theme, library and run a context
/// resolves through, with the status-bar inputs at their empty defaults —
/// nothing under the canvas reads them.
pub(crate) fn app<'a>(
    theme: &'a Theme,
    library: &'a Library,
    run_state: &'a RunState,
) -> AppCtx<'a> {
    AppCtx::new(theme, library, run_state, StatusInputs::default())
}

/// The context a test reads a canvas back through, over the same document the
/// canvas drew.
///
/// `output_types` is scratch the composition fills — a fresh one per call is
/// correct, since several tests below edit their document between frames and
/// the context resolves against whichever one it is handed.
pub(crate) fn graph_ctx_for<'a>(
    app: AppCtx<'a>,
    doc: &'a Document,
    output_types: &'a mut OutputTypes,
) -> GraphCtx<'a> {
    GraphCtx::for_document(app, doc, output_types).expect("the fixture's document shows the graph")
}

/// The graph intent behind each queued item. The assertion belongs to every
/// canvas test: none of these widgets can reach the dock.
pub(crate) fn graph_intents(queued: &[Queued]) -> Vec<&GraphIntent> {
    queued
        .iter()
        .map(|item| match item {
            Queued::Graph(intent) => intent,
            Queued::Dock(intent) => panic!("a canvas widget raised {intent:?}"),
        })
        .collect()
}
