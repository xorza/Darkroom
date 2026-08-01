//! The canvas's two widget ids and the coordinate space between them.
//!
//! A graph pane is two nested palantir canvases: an **outer** one that fills
//! the pane, paints the backdrop, and owns the input routing for everything a
//! node or port did not capture; and an **inner** one carrying the pan/zoom
//! [`TranslateScale`](palantir::TranslateScale), under which node bodies,
//! wires and shapes are recorded.
//!
//! Both ids live here rather than with
//! [`GraphUI`](crate::gui::pane::graph::GraphUI) because almost everything
//! under the pane needs one: the gestures poll the outer response, and the
//! backdrop and the cull read its rect. Keeping them together is what stops
//! two callers disagreeing about which canvas they mean. The transform
//! *between* them is [`Viewport::to_world`](crate::core::document::Viewport::to_world),
//! on the camera itself.

use palantir::WidgetId;

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
