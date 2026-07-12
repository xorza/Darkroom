//! The built-in `Preview` node's inline image view: the fetched preview
//! thumbnail rendered in the node body beneath the ports. Reuses the inspector's
//! value pipeline — the node registers itself in the run-state watch registry
//! (see [`preview_watch_nodes`]), so its output value is fetched on demand — and
//! the inspector's thumbnail renderer ([`image_preview`]).
//!
//! The thumbnail opens the full-resolution viewer on click, scanned from last
//! frame's responses ([`emit_preview_node_opens`]) like every other canvas
//! gesture, so the record only draws. Size is hardcoded for now; a per-node size
//! control lived here previously and can return.

use aperture::{Spacing, Ui, WidgetId};
use scenarium::graph::NodeId;

use crate::core::document::{PortKind, PortRef};
use crate::gui::UiAction;
use crate::gui::node::RecordCtx;
use crate::gui::node_values::preview_slot;
use crate::gui::scene::{Scene, SceneNode};

/// The inline preview slot's fixed size (a 3:2 box). Constant whether or not the
/// node holds an image, so the node never reflows when its output arrives; the
/// image is letterboxed inside. Hardcoded to a large preview for now.
const PREVIEW_WIDTH: f32 = 288.0;
const PREVIEW_HEIGHT: f32 = PREVIEW_WIDTH * 2.0 / 3.0;
/// Inset around the slot so it isn't flush against the ports/edges.
const PREVIEW_MARGIN: f32 = 6.0;

/// Preview nodes in the scene, for the frame loop to fetch runtime values for —
/// the node's inline view needs its output value even with no inspector open.
pub(crate) fn preview_watch_nodes(scene: &Scene) -> impl Iterator<Item = NodeId> + '_ {
    scene.nodes.iter().filter(|n| n.preview).map(|n| n.id)
}

/// Surface clicks on a Preview node's inline thumbnail as
/// [`UiAction::OpenImageViewer`] on its output port — the same full-resolution
/// viewer the inspector's thumbnails open.
pub(crate) fn emit_preview_node_opens(ui: &Ui, scene: &Scene, actions: &mut Vec<UiAction>) {
    for node in scene.nodes.iter().filter(|n| n.preview) {
        if ui.response_for(preview_node_image_wid(node.id)).clicked {
            actions.push(UiAction::OpenImageViewer(PortRef {
                node_id: node.id,
                kind: PortKind::Output,
                port_idx: 0,
            }));
        }
    }
}

/// The Preview node's body section: a fixed-size image slot (inset by
/// [`PREVIEW_MARGIN`]) holding the fetched output thumbnail, or an empty framed
/// box when the node hasn't run or its output isn't an image. The slot's size is
/// constant, so the node doesn't reflow when the image arrives.
pub(crate) fn preview_section(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode) {
    let handle = rcx
        .run_state
        .values(node.id)
        .and_then(|v| v.outputs.first())
        .and_then(|pv| pv.preview.as_ref());
    preview_slot(
        ui,
        rcx.theme,
        preview_node_image_wid(node.id),
        handle,
        PREVIEW_WIDTH,
        PREVIEW_HEIGHT,
        Spacing::all(PREVIEW_MARGIN),
    );
}

/// Stable id for a Preview node's inline thumbnail (its click opens the viewer).
pub(crate) fn preview_node_image_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.preview_image", node_id))
}
