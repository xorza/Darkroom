//! The viewer's half of the affine-camera algebra: texture texels → logical
//! px → the pane-local rect a [`Viewport`] paints into, plus the two framing
//! answers (fit, zoom-about-center) built on it.
//!
//! The other half is [`ZoomRange`], the pan/zoom folding the viewer shares
//! with the graph canvas. Split off here
//! so the algebra can be read — and tested — without the widget tree around
//! it: everything below is a pure function of sizes and viewports.

use glam::Vec2;
use palantir::{Rect, Size};

use crate::core::document::viewport::Viewport;
use crate::gui::widgets::zoom_range::ZoomRange;

/// Viewer zoom range — far wider than the canvas's: out to overview a
/// full-sensor frame in a small pane, in for pixel peeping.
pub(super) const VIEWER_ZOOM: ZoomRange = ZoomRange {
    min: 0.02,
    max: 32.0,
};

/// The pane-local rect a viewport paints the texture into.
pub(super) fn draw_rect(image: Vec2, v: Viewport) -> Rect {
    Rect {
        min: v.pan,
        size: Size::new(image.x * v.zoom, image.y * v.zoom),
    }
}

/// Aspect-preserving fit of `image` (its 1:1 logical footprint) in `pane`
/// (`ImageFit::Contain` semantics, upscaling small images too), as an
/// explicit viewport so the drawn fit and the gesture math can't drift.
pub(super) fn fit_viewport(image: Vec2, pane: Vec2) -> Viewport {
    let zoom = (pane.x / image.x).min(pane.y / image.y);
    Viewport {
        pan: (pane - image * zoom) * 0.5,
        zoom,
    }
}

/// The viewport at `zoom` that keeps the texel under the pane center
/// fixed — the button sibling of the cursor-anchored wheel zoom.
pub(super) fn zoom_about_pane_center(mut v: Viewport, zoom: f32, pane: Vec2) -> Viewport {
    let factor = zoom / v.zoom;
    VIEWER_ZOOM.zoom_about(&mut v, pane * 0.5, factor);
    v
}

#[cfg(test)]
mod tests;
