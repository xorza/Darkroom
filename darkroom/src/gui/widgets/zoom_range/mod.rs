//! [`ZoomRange`]: the zoom algebra a pannable, zoomable surface folds its
//! scroll and pinch input through — the graph canvas and the image viewer
//! alike, each with its own range.

use glam::Vec2;
use palantir::{ResponseState, Ui, ZoomFactor};

use crate::core::document::viewport::Viewport;

/// Whether `resp` carries any scroll, touchpad or pinch input this frame.
/// Palantir reports exactly zero, and a unit zoom, for none, so the tests are
/// exact.
pub(crate) fn scrolled(resp: &ResponseState) -> bool {
    resp.scroll.pixels != Vec2::ZERO
        || resp.scroll.lines.y != 0.0
        || resp.scroll.zoom != ZoomFactor::ONE
}

/// The zoom factors a surface allows. Pinch / scroll-zoom deltas multiply in;
/// clamped each frame so pathological gestures can't drive the zoom to 0
/// (which would make the inverse transform explode) or to a value so large
/// that the world coordinates underflow.
///
/// A type rather than a pair of bounds, because the canvas and the image viewer
/// bound zoom differently and both pass their range into the same folding: a
/// named range says which surface's is in play.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ZoomRange {
    pub(crate) min: f32,
    pub(crate) max: f32,
}

impl ZoomRange {
    /// Fold one frame's scroll/pinch deltas from `resp` into `v` — the
    /// shared half of the canvas and image-viewer pan/zoom gestures (drag
    /// panning stays with each caller, whose button policy differs):
    /// two-finger `scroll_pixels` pan, wheel `scroll_lines` zoom-about-
    /// cursor, and pinch `zoom_factor` zoom-about-cursor, clamped to this
    /// range.
    pub(crate) fn fold_scroll(self, v: &mut Viewport, ui: &Ui, resp: &ResponseState) {
        if resp.scroll.pixels != Vec2::ZERO {
            v.pan -= resp.scroll.pixels;
        }
        if resp.scroll.lines.y != 0.0
            && let Some(pivot) = resp.pointer_local
        {
            let text = &ui.theme().text;
            let line_px = text.line_height();
            self.zoom_about(
                v,
                pivot,
                scroll_to_zoom_factor(resp.scroll.lines.y * line_px),
            );
        }
        if resp.scroll.zoom != ZoomFactor::ONE
            && let Some(pivot) = resp.pointer_local
        {
            self.zoom_about(v, pivot, resp.scroll.zoom.get());
        }
    }

    /// Multiply `v`'s zoom by `factor` while holding the pre-transform point
    /// under `pivot_local` fixed in the pane, so a gesture can fold several
    /// inputs before emitting/committing once. Standard zoom-about-cursor
    /// algebra: world point under cursor = `(pivot - pan) / zoom`; choose new
    /// pan so that same world point stays under the same screen pixel after
    /// scaling. Clamps to this range, widened to hold `v`'s own zoom, so a
    /// zoom outside it — a fit to a tiny or a huge image — steps back toward it
    /// and never against the step's direction; ignores non-finite /
    /// non-positive factors.
    pub(crate) fn zoom_about(self, v: &mut Viewport, pivot_local: Vec2, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let new_zoom = (v.zoom * factor).clamp(self.min.min(v.zoom), self.max.max(v.zoom));
        let effective = new_zoom / v.zoom;
        v.pan = pivot_local - (pivot_local - v.pan) * effective;
        v.zoom = new_zoom;
    }

    /// `zoom` held to this range.
    pub(crate) const fn clamp(self, zoom: f32) -> f32 {
        zoom.clamp(self.min, self.max)
    }
}

/// Per-pixel base for converting wheel / touchpad scroll into a
/// multiplicative zoom factor. Tuned so a single classic wheel notch
/// (~16-20 logical px after palantir's line→pixel conversion) yields
/// roughly a 4-5% zoom step, while a fast touchpad swipe (~50-100 px
/// in one frame) stays a controlled ~13-22% step. Lower → slower
/// zoom, higher → snappier but jumps badly on touchpad.
const SCROLL_ZOOM_BASE: f32 = 1.0025;

/// Map a one-frame vertical scroll delta (in logical px, palantir's
/// "advance offset forward" sign convention — `+y` = scroll content
/// down) to a multiplicative zoom factor. Negative `delta_y` (wheel
/// up) zooms in (`factor > 1`); positive (wheel down) zooms out
/// (`factor < 1`). Pure function so it can be unit-tested without
/// spinning up a UI. [`ZoomRange::fold_scroll`] is its one reader, for the canvas
/// and the viewer alike.
fn scroll_to_zoom_factor(delta_y: f32) -> f32 {
    SCROLL_ZOOM_BASE.powf(-delta_y)
}

#[cfg(test)]
mod tests;
