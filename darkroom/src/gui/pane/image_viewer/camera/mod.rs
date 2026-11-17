//! The viewer's half of the affine-camera algebra: texture texels → logical
//! px → the pane-local rect a [`Viewport`] paints into, plus the two framing
//! answers (fit, zoom-about-center) built on it.
//!
//! The other half is [`ZoomRange`], the pan/zoom folding the viewer shares with the graph
//! canvas. Everything below is a pure function of sizes and viewports, so it is tested
//! without the widget tree.

use glam::{USizeVec2, UVec2, Vec2};
use palantir::{Rect, Size};

use crate::core::document::viewport::Viewport;
use crate::gui::widgets::zoom_range::ZoomRange;

/// Viewer zoom range, far wider than the canvas's: out to overview a full-sensor frame in a
/// small pane, in for pixel peeping.
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

/// The source pixel painted at pane-local `at`, for an image of `native` pixels painted at its
/// 1:1 logical footprint `image` through `v`, or `None` off the image. Read through `at`'s share
/// of the drawn rect, so a texture downscaled below its source still names the source's pixel.
pub(super) fn native_pixel(at: Vec2, image: Vec2, v: Viewport, native: UVec2) -> Option<USizeVec2> {
    let rect = draw_rect(image, v);
    let share = (at - rect.min) / Vec2::new(rect.size.w, rect.size.h);
    if !(share.cmpge(Vec2::ZERO).all() && share.cmplt(Vec2::ONE).all()) {
        return None;
    }
    // A share just under one can round up to `native` in f32: the last pixel is the one under it.
    let pixel = (share * native.as_vec2())
        .floor()
        .as_uvec2()
        .min(native - UVec2::ONE);
    Some(pixel.as_usizevec2())
}

/// Aspect-preserving fit of `image` (its 1:1 logical footprint) in `pane`
/// (`Fit::Contain` semantics, upscaling small images too), as an
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
