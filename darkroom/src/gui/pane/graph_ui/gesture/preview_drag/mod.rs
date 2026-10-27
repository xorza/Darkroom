//! Ctrl+drag off an output port: spawn a preview node already reading that
//! port and drag it under the cursor in the same gesture.
//!
//! The one-gesture counterpart to the port menu's "Add preview"
//! ([`crate::gui::pane::graph_ui::node_ui::port_row`]), which both build their intents through
//! [`PreviewSpawn::intents`].
//! Once the node exists this is an ordinary node drag, so the whole after-the-latch half is
//! [`GroupDrag`](crate::gui::pane::graph_ui::gesture::group_drag::GroupDrag)'s.
//!
//! [`WireUi`](crate::gui::pane::graph_ui::gesture::wire_ui) drops the output column
//! from its own latch candidates under the same modifier ([`modifier_held`]), so exactly one
//! controller claims the press.

use std::sync::Arc;

use glam::Vec2;
use palantir::Ui;
use scenarium::{Binding, Func, InputPort, NodeId, OutputPort};

use crate::core::document::item_id::ItemId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::core::preview_sink;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::gesture::group_drag::Grab;
use crate::gui::pane::graph_ui::wid;

/// Whether the modifier reserving an output-port drag for spawning a preview is held. The one
/// place that chord is decided: [`start`] claims an output drag under it, and `WireUi` drops
/// the output column from its latch candidates under the same condition, so the two cannot drift
/// into both claiming, or neither.
pub(crate) const fn modifier_held(ui: &mut Ui) -> bool {
    ui.modifiers().ctrl
}

/// A Ctrl+drag that started on an output port this frame: the grab of the preview node it
/// spawns, for the canvas's one group drag to latch, and the edits that spawn it, which join the
/// drag's gesture so an Esc takes the spawn back with the drag: `darkroom/AGENTS.md`, "Undo", "A
/// held gesture is one entry, and Esc takes it back whole".
#[derive(Debug)]
pub(crate) struct PreviewStart {
    pub(crate) grab: Grab,
    pub(crate) spawn: [GraphIntent; 2],
}

/// The preview a Ctrl+drag off an output port starts this frame, or `None`. Asked only while no
/// drag is in flight: one pointer holds one drag.
pub(crate) fn start(ui: &mut Ui, ctx: CanvasCtx<'_>) -> Option<PreviewStart> {
    let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
    if !modifier_held(ui) {
        return None;
    }
    let port = geometry.ports.started_drag()?;
    let output = port.output_port()?;
    if !graph_ctx.contains(port.node_id) {
        return None;
    }
    let func = preview_sink::registered(graph_ctx.library())?;
    // One lookup gating the whole spawn: a port that hasn't measured has no
    // position to start from and no anchor to latch, so the node isn't
    // created at all — the next frame's press (by which time it has
    // measured) makes it properly, rather than stranding a card at the
    // canvas origin with no drag holding it.
    let center = geometry.ports.center(port)?;
    let node_id = NodeId::unique();
    // A brand-new node is in no selection yet, so it drags alone. The
    // anchor is the port circle — the widget that owns this press; the node
    // itself has not been recorded yet and has no response to poll. Started
    // *at* the port, so it visually grows out of the circle.
    Some(PreviewStart {
        grab: Grab {
            item: ItemId::Node(node_id),
            members: Arc::from([DragStart {
                item: ItemId::Node(node_id),
                pos: center,
            }]),
            handle: wid::port_circle(port),
        },
        spawn: PreviewSpawn {
            port: output,
            pos: center,
            node_id,
        }
        .intents(func),
    })
}

/// A preview node `node_id` to spawn at `pos`, already reading `port`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PreviewSpawn {
    pub(crate) port: OutputPort,
    pub(crate) pos: Vec2,
    pub(crate) node_id: NodeId,
}

impl PreviewSpawn {
    /// The two intents that spawn the preview `func` declares. Emitted
    /// together so one undo removes node *and* wire.
    pub(crate) fn intents(self, func: &Func) -> [GraphIntent; 2] {
        let Self { port, pos, node_id } = self;
        [
            GraphIntent::AddNode {
                pos,
                node_id,
                node: func.into(),
                bindings: func.default_bindings(node_id).collect(),
            },
            GraphIntent::SetInput {
                input: InputPort::new(node_id, 0),
                to: Some(Binding::Wire(port)),
            },
        ]
    }
}

#[cfg(test)]
mod tests;
