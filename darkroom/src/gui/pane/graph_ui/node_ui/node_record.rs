//! [`NodeRecord`]: one node body's record pass.

use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::draw_ctx::DrawCtx;

/// One node body as the record pass draws it: the node, and the stage of the canvas pass it sits
/// in, whose requests its widgets raise edits into.
///
/// A bundle because every helper under the body takes both. `Ui` stays
/// outside it: palantir hands each nested panel's body a `Ui` of its own, so a
/// context holding one could not ride into the closures that record the
/// children.
#[derive(Debug)]
pub(super) struct NodeRecord<'a, 'o> {
    pub(super) node: NodeCtx<'a>,
    pub(super) stage: Stage<'o, DrawCtx<'a>>,
}
