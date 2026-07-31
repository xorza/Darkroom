//! Fixtures every canvas test drives a real frame through: a library to
//! project nodes from, the context chain over a document, and the assertions
//! about what a pass emitted.
//!
//! Shared rather than per-file because the subject under test is the same
//! machinery in each case — a `UiHarness` frame over a `GraphUI` — and only
//! the gesture being driven differs.

use glam::Vec2;
use scenarium::{
    DataType, Func, FuncId, FuncInput, FuncOutput, Library, OutputType, OutputTypes, testing,
};

use crate::core::document::{Document, GraphView};
use crate::core::edit::intent::sink::Queued;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::app::ctx::{AppCtx, StatusInputs};
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// One func with an input and an output, so a node projected from it
/// records the full body: header badges, both port columns, and the
/// const editor on the unbound input.
pub(crate) fn one_func_library() -> Library {
    let mut library = Library::default();
    library.add(testing::with_stub_lambda(
        Func::new(FuncId::unique(), "probe")
            .pure()
            .input(FuncInput::optional("a", DataType::Int))
            .output(FuncOutput::new("out", DataType::Int)),
    ));
    library
}

/// [`one_func_library`] plus a passthrough: one input and one wildcard output
/// mirroring it, so the projected type of its output is whatever the *graph*
/// wires in rather than anything its declaration states.
pub(crate) fn wildcard_library() -> Library {
    let mut library = one_func_library();
    let mut passthrough = Func::new(FuncId::unique(), "passthrough")
        .pure()
        .input(FuncInput::optional("in", DataType::Any));
    passthrough.outputs.push(FuncOutput {
        name: "out".to_owned(),
        description: None,
        ty: OutputType::Wildcard { mirrors: 0 },
    });
    library.add(testing::with_stub_lambda(passthrough));
    library
}

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

/// Spread the placements so no node lands off-viewport and gets culled —
/// `GraphView::for_graph` seeds every item at the origin.
pub(crate) fn spread(view: &mut GraphView) {
    for (i, pos) in view.item_placements.values_mut().enumerate() {
        *pos = Vec2::new(40.0 + i as f32 * 220.0, 40.0);
    }
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
