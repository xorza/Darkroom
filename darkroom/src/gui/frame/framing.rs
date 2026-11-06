//! [`Framing`]: a one-shot move of the graph's camera.

/// One of the graph toolbar's one-shot framings. A descriptor resolved at
/// its call site into a `GraphIntent::SetViewport`, never queued itself, so
/// a reframe moves the camera as a manual pan/zoom does. Here and not in the canvas, which resolves
/// it, as an [`AppCommand`](crate::gui::frame::app_command::AppCommand) names it and this module
/// imports no surface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Framing {
    /// Reset to 1:1 zoom, centered on all nodes (world origin when empty).
    Reset,
    /// Fit every node in the view.
    ShowAll,
    /// Fit the selected nodes; a no-op when nothing is selected.
    ShowSelected,
}
