//! [`NodeRecord`]: one node body's record pass.

use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::draw_ctx::DrawCtx;

/// One node body as the record pass draws it: the node, the canvas pass it
/// sits in, and the queue its widgets raise edits into.
///
/// A bundle because every helper under the body takes all three. `Ui` stays
/// outside it: palantir hands each nested panel's body a `Ui` of its own, so a
/// context holding one could not ride into the closures that record the
/// children.
#[derive(Debug)]
pub(super) struct NodeRecord<'a, 'o> {
    pub(super) node: NodeCtx<'a>,
    pub(super) draw: DrawCtx<'a>,
    pub(super) out: &'o mut Requests,
}
