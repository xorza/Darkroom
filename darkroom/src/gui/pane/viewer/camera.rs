//! The viewer's half of the affine-camera algebra: texture texels → logical
//! px → the pane-local rect a [`Viewport`] paints into, plus the two framing
//! answers (fit, zoom-about-center) built on it.
//!
//! The other half lives in [`crate::gui::pane::graph::gesture::pan_zoom`],
//! which owns the shared pan/zoom folding both surfaces call. Split off here
//! so the algebra can be read — and tested — without the widget tree around
//! it: everything below is a pure function of sizes and viewports.

use glam::{UVec2, Vec2};
use palantir::{Rect, Size};

use crate::core::document::Viewport;
use crate::gui::pane::graph::gesture::pan_zoom::zoom_about;

/// Viewer zoom bounds — far wider than the canvas's
/// (`pan_zoom::CANVAS_MIN_ZOOM`/`CANVAS_MAX_ZOOM`): out to overview a
/// texture-capped 8k frame in a small pane, in for pixel peeping. Named apart
/// from the canvas pair because both are passed into the same shared
/// `fold_scroll_zoom` / `zoom_about`, where an unqualified `MIN_ZOOM` at the
/// call site wouldn't say which surface's range is in play.
pub(super) const VIEWER_MIN_ZOOM: f32 = 0.02;
pub(super) const VIEWER_MAX_ZOOM: f32 = 32.0;

/// A texture's logical footprint when each texel occupies one physical pixel.
pub(super) fn logical_image_size(texels: UVec2, display_scale: f32) -> Vec2 {
    texels.as_vec2() / display_scale
}

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
mod tests {
    use super::*;

    #[test]
    fn fit_viewport_centers_and_scales_like_contain() {
        // 400×200 texture in an 800×800 pane: width binds at zoom 2 —
        // Contain upscales. pan = ((800,800) - (800,400)) / 2 = (0, 200).
        let v = fit_viewport(Vec2::new(400.0, 200.0), Vec2::new(800.0, 800.0));
        assert_eq!(v.zoom, 2.0);
        assert_eq!(v.pan, Vec2::new(0.0, 200.0));

        // 4000×2000 in 1000×1000: zoom 0.25, pan = (0, (1000-500)/2) = (0, 250).
        let v = fit_viewport(Vec2::new(4000.0, 2000.0), Vec2::new(1000.0, 1000.0));
        assert_eq!(v.zoom, 0.25);
        assert_eq!(v.pan, Vec2::new(0.0, 250.0));

        // Height-bound case: 200×400 in 800×400 → zoom 1, pan = (300, 0).
        let v = fit_viewport(Vec2::new(200.0, 400.0), Vec2::new(800.0, 400.0));
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.pan, Vec2::new(300.0, 0.0));

        // The drawn rect covers exactly pan..pan+img*zoom.
        let r = draw_rect(Vec2::new(200.0, 400.0), v);
        assert_eq!(r.min, Vec2::new(300.0, 0.0));
        assert_eq!(r.size, Size::new(200.0, 400.0));

        // The same 400x200 fit in an 800x800 logical pane on a 2x display
        // occupies 1600x800 physical px, so its magnification is 4x while
        // the logical draw rect remains exactly 800x400.
        let img = logical_image_size(UVec2::new(400, 200), 2.0);
        assert_eq!(img, Vec2::new(200.0, 100.0));
        let v = fit_viewport(img, Vec2::new(800.0, 800.0));
        assert_eq!(v.zoom, 4.0);
        assert_eq!(v.pan, Vec2::new(0.0, 200.0));
        assert_eq!(draw_rect(img, v).size, Size::new(800.0, 400.0));
    }

    #[test]
    fn zoom_about_pane_center_keeps_center_texel() {
        // Start from the fit of 400×200 in an 800×800 pane: zoom 2,
        // pan (0, 200). The texel under the pane center (400, 400) is
        // ((400 - 0)/2, (400 - 200)/2) = (200, 100) — the image center.
        let fit = fit_viewport(Vec2::new(400.0, 200.0), Vec2::new(800.0, 800.0));
        assert_eq!(fit.zoom, 2.0);

        // Zoom to 100%: pan' = center - texel·1 = (400-200, 400-100).
        let pane = Vec2::new(800.0, 800.0);
        let v = zoom_about_pane_center(fit, 1.0, pane);
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.pan, Vec2::new(200.0, 300.0));

        // The invariant holds for an arbitrary target too: zoom 4 →
        // pan' = center - texel·4 = (400-800, 400-400) = (-400, 0).
        let v = zoom_about_pane_center(fit, 4.0, pane);
        assert_eq!(v.zoom, 4.0);
        assert_eq!(v.pan, Vec2::new(-400.0, 0.0));

        // At 2x display scale, 1:1 physical magnification draws each texel
        // into half a logical pixel, which composes back to one physical px.
        let img_2x = logical_image_size(UVec2::new(400, 200), 2.0);
        let fit_2x = fit_viewport(img_2x, Vec2::new(800.0, 800.0));
        let v = zoom_about_pane_center(fit_2x, 1.0, pane);
        assert_eq!(v.pan, Vec2::new(300.0, 350.0));
        assert_eq!(draw_rect(img_2x, v).size, Size::new(200.0, 100.0));
    }
}
