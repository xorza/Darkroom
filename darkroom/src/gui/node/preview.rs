//! The built-in `Preview` node's inline value view, rendered in the node body
//! beneath the ports. Preview accepts any type (a generic caching tap), so
//! what's shown adapts to the fetched value: an image renders as a thumbnail
//! (`preview_slot`), any other type as its formatted text (`preview_text`).
//! As it records, the node registers itself in the frame's value-request
//! registry (`ValueRequests::watch`), so its output value is fetched on
//! demand.
//!
//! The image thumbnail opens the full-resolution viewer on click, scanned
//! from last frame's responses ([`emit_preview_node_opens`]) like every other
//! canvas gesture, so the record only draws — a text preview isn't a click
//! target (there's no full-resolution view to open for a non-image value).
//! Size is hardcoded for now; a per-node size control lived here previously
//! and can return.

use aperture::{Spacing, Ui, WidgetId};
use scenarium::graph::NodeId;

use crate::core::document::{PortKind, PortRef};
use crate::gui::UiAction;
use crate::gui::node::RecordCtx;
use crate::gui::node_values::{preview_slot, preview_text};
use crate::gui::scene::{Scene, SceneNode};

/// The inline image slot's fixed width and height cap. Width is constant so
/// the node never reflows sideways; height adapts to the image's own aspect
/// ratio up to this cap (a portrait image pillarboxes once it would exceed a
/// square), so a wide landscape image isn't padded out to a fixed box.
/// Hardcoded to a large preview for now.
const PREVIEW_WIDTH: f32 = 288.0;
const PREVIEW_MAX_HEIGHT: f32 = PREVIEW_WIDTH;
/// Inset around the slot so it isn't flush against the ports/edges.
const PREVIEW_MARGIN: f32 = 6.0;

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

/// The Preview node's body section, inset by [`PREVIEW_MARGIN`]: the fetched
/// output value rendered as an image thumbnail when it is one, its formatted
/// text otherwise, or an empty framed placeholder when the node hasn't run
/// yet.
pub(crate) fn preview_section(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode) {
    // Recording the slot IS the request: ask for this node's output value so
    // the preview fills in (only the visible/recorded preview nodes register).
    rcx.value_requests.watch(node.id);
    let value = rcx
        .run_state
        .values(node.id)
        .and_then(|v| v.outputs.first());
    let image = value.and_then(|pv| pv.preview.as_ref());
    match (value, image) {
        // A fetched value that isn't an image: show its text instead of an
        // empty image box.
        (Some(pv), None) => preview_text(
            ui,
            rcx.theme,
            preview_node_text_wid(node.id),
            &pv.text,
            PREVIEW_WIDTH,
            Spacing::all(PREVIEW_MARGIN),
        ),
        // An image value, or nothing fetched yet (an empty placeholder).
        _ => preview_slot(
            ui,
            rcx.theme,
            preview_node_image_wid(node.id),
            image,
            PREVIEW_WIDTH,
            PREVIEW_MAX_HEIGHT,
            Spacing::all(PREVIEW_MARGIN),
        ),
    }
}

/// Stable id for a Preview node's inline thumbnail (its click opens the viewer).
pub(crate) fn preview_node_image_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.preview_image", node_id))
}

/// Stable id for a Preview node's inline text fallback. Distinct from
/// [`preview_node_image_wid`] so a text preview never satisfies
/// [`emit_preview_node_opens`]'s click check — it isn't sensing clicks at all.
fn preview_node_text_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.preview_text", node_id))
}
