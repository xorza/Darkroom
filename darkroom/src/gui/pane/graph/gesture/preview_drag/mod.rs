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

use std::sync::Arc;

use palantir::Ui;
use scenarium::NodeId;

use crate::core::document::PortKind;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::core::preview;
use crate::gui::pane::graph::ctx::CanvasCtx;
use crate::gui::pane::graph::gesture::canvas_gesture::preview_drag_modifier;
use crate::gui::pane::graph::gesture::drag_anchor::GroupDrag;
use crate::gui::pane::graph::node::port_row::{add_preview_intents, port_circle_wid};
use crate::gui::requests::Requests;

/// The in-flight spawn-and-place drag, or none.
#[derive(Default, Debug)]
pub(crate) struct PreviewDrag {
    drag: GroupDrag,
}

impl PreviewDrag {
    /// Drop the in-flight spawn-and-place drag.
    pub(crate) fn reset(&mut self) {
        self.drag.reset();
    }

    /// Whether a spawned preview is being dragged.
    pub(crate) fn in_flight(&self) -> bool {
        self.drag.in_flight()
    }

    /// Advance the drag in flight, or latch a new one off a Ctrl+drag on an
    /// output port. Swept once per frame over the whole scene: only one
    /// pointer drag can be in flight.
    pub(crate) fn apply(&mut self, ui: &mut Ui, cx: CanvasCtx<'_>, out: &mut Requests) {
        let (graph_ctx, geometry) = (cx.graph_ctx(), cx.geometry());
        // A live drag owns the frame; only once it ends does the latch scan
        // below get a look at this frame's presses.
        if self.drag.advance(ui, graph_ctx, cx.cancelled(), out) || !preview_drag_modifier(ui) {
            return;
        }
        let Some(port) = geometry
            .ports
            .started_drag()
            .filter(|port| port.kind == PortKind::Output)
        else {
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
        let members = Arc::from([DragStart {
            node: node_id,
            pos: center,
        }]);
        self.drag
            .latch(node_id, members, port_circle_wid(port), out);
    }
}

#[cfg(test)]
mod tests;
