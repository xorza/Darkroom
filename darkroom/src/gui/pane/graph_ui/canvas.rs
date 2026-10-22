//! The canvas's two widget ids and the coordinate space between them.
//!
//! A graph pane is two nested palantir canvases: an **outer** one that fills
//! the pane, paints the backdrop, and owns the input routing for everything a
//! node or port did not capture; and an **inner** one carrying the pan/zoom
//! [`TranslateScale`](palantir::TranslateScale), under which node bodies,
//! wires and shapes are recorded.
//!
//! Both ids live here rather than with
//! [`GraphUi`](crate::gui::pane::graph_ui::GraphUi) because almost everything
//! under the pane needs one: the gestures poll the outer response, and the
//! backdrop and the cull read its rect. Keeping them together is what stops
//! two callers disagreeing about which canvas they mean. The transform
//! *between* them is [`Viewport::to_world`](crate::core::document::viewport::Viewport::to_world),
//! on the camera itself.

use glam::Vec2;
use palantir::{Ui, WidgetId};

use crate::gui::graph_ctx::GraphCtx;

/// Stable id for the outer (pan-capture) canvas — the rect the gestures poll,
/// the backdrop paints, and the cull measures against.
pub(crate) fn outer_wid() -> WidgetId {
    WidgetId::from_hash("graph.canvas.outer")
}

/// Stable id for the inner (transformed) canvas. Used as the widget seed and
/// for resolving the canvas's pre-transform origin in wire draws.
pub(super) fn inner_wid() -> WidgetId {
    WidgetId::from_hash("graph.canvas.inner")
}

/// The pointer in canvas-world coords, or `None` off-window. `canvas_origin`
/// is the inner canvas's pre-transform origin.
pub(crate) fn world_pointer(
    ui: &mut Ui,
    graph_ctx: GraphCtx<'_>,
    canvas_origin: Vec2,
) -> Option<Vec2> {
    ui.pointer_pos()
        .map(|p| graph_ctx.viewport().to_world(p - canvas_origin))
}
