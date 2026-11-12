//! [`ZoomRange`]: the zoom algebra a pannable, zoomable surface folds its
//! scroll and pinch input through — the graph canvas and the image viewer
//! alike, each with its own range.

use common::Libm;
use glam::Vec2;
use palantir::{Modifiers, ResponseState, ScrollDelta, Size, Ui, ZoomFactor};

use crate::core::document::viewport::Viewport;

/// Whether `resp` carries any scroll, touchpad or pinch input this frame.
/// Palantir reports exactly zero, and a unit zoom, for none, so the tests are
/// exact.
pub(crate) fn scrolled(resp: &ResponseState) -> bool {
    resp.scroll.pixels != Vec2::ZERO
        || resp.scroll.notches != Vec2::ZERO
        || resp.scroll.zoom != ZoomFactor::ONE
}

/// What a scroll does on a surface: `darkroom/AGENTS.md`, "One scroll rule".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScrollAction {
    Pan,
    /// The command key is held.
    Zoom,
}

/// One frame's scroll and pinch input on a surface, as [`ZoomRange::fold_scroll`] reads it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScrollFrame {
    delta: ScrollDelta,
    pivot: Option<Vec2>,
    pane: Size,
    line_px: f32,
    action: ScrollAction,
}

impl ScrollFrame {
    /// The frame `resp` reports, under the modifiers held now.
    pub(crate) fn of(ui: &Ui, resp: &ResponseState) -> Self {
        Self {
            delta: resp.scroll,
            pivot: resp.pointer_local,
            pane: resp.layout_rect.map_or(Size::ZERO, |rect| rect.size),
            line_px: ui.theme().text.line_height(),
            action: if ui.peek_modifiers().contains(Modifiers::CTRL) {
                ScrollAction::Zoom
            } else {
                ScrollAction::Pan
            },
        }
    }
}

/// The zoom factors a surface allows. Zoom deltas multiply in, clamped so a gesture can't drive
/// the zoom to 0 (the inverse transform would explode) or so large the world coordinates underflow.
///
/// A type rather than a pair of bounds: the canvas and the image viewer bound zoom differently
/// and pass their range into the same folding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ZoomRange {
    pub(crate) min: f32,
    pub(crate) max: f32,
}

impl ZoomRange {
    /// Fold one frame's scroll and pinch input into `v`, clamped to this range: a scroll pans, a
    /// scroll under the command key and a pinch zoom about the cursor. Drag panning stays with
    /// each caller, whose button policy differs.
    pub(crate) fn fold_scroll(self, v: &mut Viewport, frame: ScrollFrame) {
        let ScrollFrame {
            delta,
            pivot,
            pane,
            line_px,
            action,
        } = frame;
        match action {
            ScrollAction::Pan => v.pan -= delta.pan(line_px, pane),
            ScrollAction::Zoom => {
                let delta_y = delta.pixels.y + delta.notches.y * line_px;
                if delta_y != 0.0
                    && let Some(pivot) = pivot
                {
                    self.zoom_about(v, pivot, scroll_to_zoom_factor(delta_y));
                }
            }
        }
        if delta.zoom != ZoomFactor::ONE
            && let Some(pivot) = pivot
        {
            self.zoom_about(v, pivot, delta.zoom.get());
        }
    }

    /// Multiply `v`'s zoom by `factor`, keeping the world point under `pivot_local` under the same
    /// pane pixel. Clamps to this range widened to hold `v`'s own zoom, so a zoom outside it — a
    /// fit to a tiny or a huge image — steps back toward it and never against the step's
    /// direction. Non-finite and non-positive factors are ignored.
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

/// Per-pixel base turning a scroll under the command key into a zoom factor. Tuned so a classic
/// wheel notch (~16-20 logical px) steps the zoom ~4-5% and a fast touchpad swipe (~50-100 px in
/// one frame) ~13-22%; higher is snappier but jumps on a touchpad.
const SCROLL_ZOOM_BASE: f32 = 1.0025;

/// Map a one-frame vertical scroll delta (logical px, `+y` = scroll content down) to a zoom
/// factor: negative (wheel up) zooms in, positive (wheel down) zooms out.
fn scroll_to_zoom_factor(delta_y: f32) -> f32 {
    SCROLL_ZOOM_BASE.libm_powf(-delta_y)
}

#[cfg(test)]
mod tests;
