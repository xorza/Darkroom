//! The canvas's two widget ids and the coordinate space between them.
//!
//! A graph pane is two nested palantir canvases: an **outer** one that fills
//! the pane, paints the backdrop, and owns the input routing for everything a
//! node or port did not capture; and an **inner** one carrying the pan/zoom
//! [`TranslateScale`](palantir::TranslateScale), under which node bodies,
//! wires and shapes are recorded.
//!
//! Both ids and the transform between them live here rather than with
//! [`GraphUI`](crate::gui::pane::graph::GraphUI) because almost everything
//! under the pane needs one: the gestures poll the outer response, the
//! backdrop and the cull read its rect, and the wire draws convert the pointer
//! into world coords. Keeping them together is what stops two callers
//! disagreeing about which canvas they mean.

use glam::Vec2;
use palantir::{Ui, WidgetId};

use crate::core::document::Viewport;
use crate::gui::graph_ctx::GraphCtx;

/// Stable id for the outer (pan-capture) canvas — the rect the gestures poll,
/// the backdrop paints, and the cull measures against.
pub(crate) fn outer_canvas_widget_id() -> WidgetId {
    WidgetId::from_hash("graph.canvas.outer")
}

/// Stable id for the inner (transformed) canvas. Used as the widget seed and
/// for resolving the canvas's pre-transform origin in connection draws.
pub(super) fn inner_canvas_widget_id() -> WidgetId {
    WidgetId::from_hash("graph.canvas.inner")
}

/// Outer-canvas-local coords → inner-canvas pre-transform world coords. The
/// inner canvas applies `TranslateScale::new(pan, zoom)`, so
/// `outer = pan + zoom * world`.
pub(super) fn to_world(outer_local: Vec2, viewport: &Viewport) -> Vec2 {
    (outer_local - viewport.pan) / viewport.zoom
}

/// The pointer in inner-canvas world coords, or `None` when it's off-window.
/// Where an in-flight wire's free end sits before it snaps to a target;
/// `canvas_origin` is the inner canvas's pre-transform origin.
pub(super) fn pointer_world(
    ui: &mut Ui,
    graph_ctx: GraphCtx<'_>,
    canvas_origin: Vec2,
) -> Option<Vec2> {
    ui.pointer_pos()
        .map(|p| to_world(p - canvas_origin, &graph_ctx.viewport()))
}
