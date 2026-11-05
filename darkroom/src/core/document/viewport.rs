//! [`Viewport`]: the affine camera a canvas or viewer looks through.

use glam::Vec2;
use palantir::Display;
use palantir::widget::domain::EPS;
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
/// one `ZoomRange` algebra serves both.
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

    /// This camera as a surface on `display` draws it: the pan landed where the display lands a
    /// translation ([`Display::land_translation`]), and a zoom within palantir's [`EPS`] of 1 at
    /// exactly 1, the tolerance its text draws a scale of 1 at. The stored camera stays exact, so
    /// a zoom about the pointer keeps its pivot, as palantir's scroll keeps its offset exact and
    /// draws it landed.
    pub(crate) fn landed(self, display: Display) -> Self {
        let zoom = if (self.zoom - 1.0).abs() < EPS {
            1.0
        } else {
            self.zoom
        };
        Self {
            pan: display.land_translation(self.pan),
            zoom,
        }
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

#[cfg(test)]
mod tests {
    use glam::{UVec2, Vec2};
    use palantir::{Display, Harness, SystemScale};

    use crate::core::document::viewport::Viewport;

    /// The display a harness at `scale` snapping as `pixel_snap` says hands its frames.
    fn display(scale: f32, pixel_snap: bool) -> Display {
        Harness::builder(UVec2::new(64, 64))
            .system_scale(SystemScale::new(scale).unwrap())
            .pixel_snap(pixel_snap)
            .build()
            .frame_value(|ui| ui.display())
    }

    /// The pan lands on whole physical px, half away from zero: at 2× 10.3 → 20.6 physical → 21
    /// → 10.5, and −4.3 → −4.5; at 1× 10.3 → 10 and −4.6 → −5. A zoom within palantir's 1e-4 of
    /// 1, 1.00005 or 0.99995, lands at exactly 1, and 1.0002 past it stays. A display that does
    /// not snap keeps the pan as it is but still lands the zoom.
    #[test]
    fn a_camera_lands_its_pan_on_physical_px_and_its_zoom_near_one_on_one() {
        let camera = |x: f32, y: f32, zoom: f32| Viewport {
            pan: Vec2::new(x, y),
            zoom,
        };
        for (label, display, stored, landed) in [
            (
                "2×",
                display(2.0, true),
                camera(10.3, -4.3, 1.00005),
                camera(10.5, -4.5, 1.0),
            ),
            (
                "1×",
                display(1.0, true),
                camera(10.3, -4.6, 0.99995),
                camera(10.0, -5.0, 1.0),
            ),
            (
                "past the tolerance",
                display(1.0, true),
                camera(3.0, 4.0, 1.0002),
                camera(3.0, 4.0, 1.0002),
            ),
            (
                "unsnapped",
                display(2.0, false),
                camera(10.3, -4.3, 1.00005),
                camera(10.3, -4.3, 1.0),
            ),
        ] {
            assert_eq!(stored.landed(display), landed, "{label}");
        }
    }
}
