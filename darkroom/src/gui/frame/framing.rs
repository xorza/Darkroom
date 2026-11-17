//! [`Framing`]: a one-shot move of the graph's camera.

/// A one-shot move of a graph's camera. A descriptor resolved at its call site into a
/// `GraphIntent::SetViewport`, never queued itself, so a reframe moves the camera as a manual
/// pan/zoom does. Here and not in the canvas, which resolves it, as a
/// [`PaneAsk`](crate::gui::frame::pane_ask::PaneAsk) names it and this module imports no surface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Framing {
    /// 1:1 zoom, centered on all nodes (world origin when empty): Zoom to 100%.
    Actual,
    /// Fit every node in the view.
    Fit,
    /// Fit the selected nodes; a no-op when nothing is selected.
    Selection,
    /// One rung along the zoom ladder, about the pane's center.
    Step(ZoomStep),
}

/// A step along the zoom ladder.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ZoomStep {
    In,
    Out,
}
