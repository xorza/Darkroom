//! [`Viewport`]: the affine camera a canvas or viewer looks through.

use glam::Vec2;
use serde::{Deserialize, Serialize};

/// An affine 2D camera: `local = pan + zoom * content`. One value shared by
/// the persisted per-graph
/// [`GraphView`](crate::core::document::graph_view::GraphView), the
/// `GraphCtx` a canvas reads it through, and the `SetViewport` edit, so the
/// three can't drift on field names or semantics.
///
/// The graph canvas reads `pan` as a canvas-local px offset and `zoom` as a
/// world scale factor. The image viewer reuses the same algebra over a
/// different content space — `pan` is the image's top-left offset in
/// pane-local px, `zoom` is display px per texel — which is why
/// `pan_zoom::zoom_about` and `fold_scroll_zoom` serve both.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Viewport {
    pub(crate) pan: Vec2,
    pub(crate) zoom: f32,
}

impl Viewport {
    /// Invert the camera: a point in the surface's local space → the point of
    /// *content* under it. The exact inverse of the `local = pan + zoom *
    /// content` this type documents, so it is defined here rather than
    /// re-derived by each reader.
    ///
    /// The graph canvas calls its content space "world" — node positions, wire
    /// endpoints and the cull region all live in it — and is the only caller
    /// today; the image viewer needs the forward direction instead.
    pub(crate) fn to_world(self, local: Vec2) -> Vec2 {
        (local - self.pan) / self.zoom
    }

    /// Whether this is a *persistable graph* camera — what
    /// `GraphViewValidationError::InvalidViewport` reports on. Only the
    /// document's own viewport round-trips through serde and so needs
    /// checking on the way back in; the image viewer's is rebuilt from the
    /// pane every session and never validated.
    pub(crate) fn is_valid(self) -> bool {
        self.pan.is_finite() && self.zoom.is_finite() && self.zoom > 0.0
    }
}

impl Default for Viewport {
    /// Origin pan, 1:1 zoom.
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}
