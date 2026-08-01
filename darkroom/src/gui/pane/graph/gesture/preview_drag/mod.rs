//! Ctrl+drag off an output port: spawn a preview node already reading that
//! port and drag it under the cursor in the same gesture.
//!
//! The one-gesture counterpart to the port menu's "Add preview"
//! ([`crate::gui::pane::graph::node::port_row`]), which both build their intents through
//! [`add_preview_intents`]. Once the node exists this is an ordinary node drag,
//! so the whole after-the-latch half is [`GroupDrag`]'s.
//!
//! [`ConnectionUI`](crate::gui::pane::graph::gesture::connection) drops the output column
//! from its own latch candidates under the same modifier
//! ([`preview_drag_modifier`]), so exactly one controller claims the press.

use palantir::Ui;
use scenarium::NodeId;

use crate::core::document::{PortKind, PortRef};
use crate::core::preview;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::ctx::CanvasCtx;
use crate::gui::pane::graph::frame::geometry::CanvasGeometry;
use crate::gui::pane::graph::gesture::drag_anchor::GroupDrag;
use crate::gui::pane::graph::node::port_row::{add_preview_intents, port_circle_wid};
use crate::gui::pane::graph::preview_drag_modifier;
use crate::gui::requests::Requests;

/// The in-flight spawn-and-place drag, or none.
#[derive(Default, Debug)]
pub(crate) struct PreviewDrag {
    drag: GroupDrag,
}

impl PreviewDrag {
    /// Swept once per frame over the whole scene: only one pointer drag can be
    /// in flight, and `PortRef` is document-unique, so the pane comes from the
    /// port's own node rather than from the caller.
    pub(crate) fn apply(&mut self, ui: &mut Ui, cx: CanvasCtx<'_>, out: &mut Requests) {
        let (graph_ctx, geometry) = (cx.graph_ctx(), cx.geometry());
        // A live drag owns the frame; only once it ends does the latch scan
        // below get a look at this frame's presses.
        if self.drag.advance(ui, graph_ctx, out) || !preview_drag_modifier(ui) {
            return;
        }
        let Some(port) = scan_output_drag_start(geometry, graph_ctx) else {
            return;
        };
        if !graph_ctx.contains(port.node_id) {
            return;
        }
        let Some(func) = preview::registered(graph_ctx.library()) else {
            return;
        };
        // One lookup gating the whole spawn: a port that hasn't measured has no
        // position to start from and no anchor to latch, so the node isn't
        // created at all — the next frame's press (by which time it has
        // measured) makes it properly, rather than stranding a card at the
        // canvas origin with no drag holding it.
        let Some(center) = geometry.ports.center(port) else {
            return;
        };

        let node_id = NodeId::unique();
        // Start it *at* the port so it visually grows out of the circle;
        // the drag below carries it from there.
        out.extend_graph(add_preview_intents(func, port, center, node_id));
        // A brand-new node is in no selection yet, so it drags alone. The
        // anchor is the port circle — the widget that owns this press; the node
        // itself has not been recorded yet and has no response to poll.
        self.drag
            .latch(node_id, vec![(node_id, center)], port_circle_wid(port));
    }
}

/// First output port whose circle began a drag this frame. Unfiltered by pane:
/// only one press exists, so the caller resolves the winner's pane once rather
/// than paying a lookup per candidate node.
fn scan_output_drag_start(geometry: &CanvasGeometry, graph_ctx: GraphCtx<'_>) -> Option<PortRef> {
    let keys = graph_ctx
        .nodes()
        .flat_map(|node| node.ports(PortKind::Output));
    geometry.ports.first_drag_started(keys)
}

#[cfg(test)]
mod tests;
