//! The viewer's half of the affine-camera algebra: texture texels → logical
//! px → the pane-local rect a [`Viewport`] paints into, plus the two framing
//! answers (fit, zoom-about-center) built on it.
//!
//! The other half lives in [`crate::gui::pane::graph::gesture::pan_zoom`],
//! which owns the shared pan/zoom folding both surfaces call. Split off here
//! so the algebra can be read — and tested — without the widget tree around
//! it: everything below is a pure function of sizes and viewports.

use glam::Vec2;
use palantir::{Rect, Size};

use crate::core::document::Viewport;
use crate::gui::pane::graph::gesture::pan_zoom::zoom_about;

/// Viewer zoom bounds — far wider than the canvas's
/// (`pan_zoom::CANVAS_MIN_ZOOM`/`CANVAS_MAX_ZOOM`): out to overview a
/// full-sensor frame in a small pane, in for pixel peeping. Named apart
/// from the canvas pair because both are passed into the same shared
/// `fold_scroll_zoom` / `zoom_about`, where an unqualified `MIN_ZOOM` at the
/// call site wouldn't say which surface's range is in play.
pub(super) const VIEWER_MIN_ZOOM: f32 = 0.02;
pub(super) const VIEWER_MAX_ZOOM: f32 = 32.0;

/// The pane-local rect a viewport paints the texture into.
pub(super) fn draw_rect(img: Vec2, v: Viewport) -> Rect {
    Rect {
        min: v.pan,
        size: Size::new(img.x * v.zoom, img.y * v.zoom),
    }
}

/// Aspect-preserving fit of `img` (its 1:1 logical footprint) in `pane`
/// (`ImageFit::Contain` semantics, upscaling small images too), as an
/// explicit viewport so the drawn fit and the gesture math can't drift.
pub(super) fn fit_viewport(img: Vec2, pane: Vec2) -> Viewport {
    let zoom = (pane.x / img.x).min(pane.y / img.y);
    Viewport {
        pan: (pane - img * zoom) * 0.5,
        zoom,
    }
}

/// The viewport at `zoom` that keeps the texel under the pane center
/// fixed — the button sibling of the cursor-anchored wheel zoom.
pub(super) fn zoom_about_pane_center(mut v: Viewport, zoom: f32, pane: Vec2) -> Viewport {
    let factor = zoom / v.zoom;
    zoom_about(
        &mut v.pan,
        &mut v.zoom,
        pane * 0.5,
        factor,
        VIEWER_MIN_ZOOM,
        VIEWER_MAX_ZOOM,
    );
    v
}

#[cfg(test)]
mod tests;
