//! Canvas viewport gesture: middle-drag pan, wheel/pinch zoom-about-
//! cursor, and the zoom-factor math. Split out of `canvas` so the
//! orchestration there isn't tangled with the (independently testable)
//! viewport algebra. The gesture emits `GraphIntent::SetViewport`, so pan/zoom
//! rides the same undo path as every other edit.

pub(crate) mod camera_gesture;

use glam::Vec2;
use palantir::{Rect, ResponseState, Size, Ui, ZoomFactor};

use crate::core::document::viewport::Viewport;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry;

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

/// The canvas's zoom range.
const CANVAS_ZOOM: ZoomRange = ZoomRange { min: 0.1, max: 5.0 };

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
            let line_px = text.line_height_for(text.font_size);
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
    /// scaling. Clamps to this range; ignores non-finite / non-positive
    /// factors. Shared with [`crate::gui::pane::image_viewer`].
    pub(crate) fn zoom_about(self, v: &mut Viewport, pivot_local: Vec2, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let new_zoom = self.clamp(v.zoom * factor);
        let effective = new_zoom / v.zoom;
        v.pan = pivot_local - (pivot_local - v.pan) * effective;
        v.zoom = new_zoom;
    }

    /// `zoom` held to this range.
    const fn clamp(self, zoom: f32) -> f32 {
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

/// Breathing room (logical px) left on every side when fitting content
/// to the viewport, so framed nodes don't butt against the pane edge.
const FIT_MARGIN: f32 = 40.0;

/// One of the graph toolbar's one-shot framings. A descriptor resolved at
/// its call site into a `GraphIntent::SetViewport`, never queued itself, so
/// a reframe rides the same undo path as a manual pan/zoom, as an undo entry
/// of its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Framing {
    /// Reset to 1:1 zoom, centered on all nodes (world origin when empty).
    Reset,
    /// Fit every node in the view.
    ShowAll,
    /// Fit the selected nodes; a no-op when nothing is selected.
    ShowSelected,
}

impl Framing {
    /// The `SetViewport` intent this framing implies on the canvas `cx`, or
    /// `None` when there's nothing to frame — `ShowSelected` with an empty
    /// selection, no nodes to fit, or the viewport not yet measured. The pane
    /// size comes from the outer canvas's `layout_rect`; node extents come
    /// from the geometry's cross-frame size cache, position from
    /// `NodeCtx::pos`.
    pub(crate) fn intent(self, ui: &Ui, cx: CanvasCtx<'_>) -> Option<GraphIntent> {
        let (geometry, graph_ctx) = (cx.geometry(), cx.graph_ctx());
        let vp = ui.response_for(canvas::outer_wid()).layout_rect?.size;
        let pane = Vec2::new(vp.w, vp.h);
        let to = match self {
            Self::Reset => reset_target(geometry, graph_ctx, pane),
            Self::ShowAll => fit_target(node_bounds(geometry, graph_ctx, Bounded::All)?, pane),
            Self::ShowSelected => {
                if graph_ctx.selected().is_empty() {
                    return None;
                }
                fit_target(node_bounds(geometry, graph_ctx, Bounded::Selected)?, pane)
            }
        };
        Some(GraphIntent::SetViewport { to, gesture: None })
    }
}

/// 1:1 zoom, centered on all content (world origin when the graph is empty).
fn reset_target(geometry: &CanvasGeometry, graph_ctx: GraphCtx<'_>, pane: Vec2) -> Viewport {
    let pan = match node_bounds(geometry, graph_ctx, Bounded::All) {
        Some(b) => pane * 0.5 - b.center(),
        None => Vec2::ZERO,
    };
    Viewport { pan, zoom: 1.0 }
}

/// Which nodes a framing bounds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Bounded {
    All,
    Selected,
}

/// World-space (inner-canvas pre-transform) bounding box of the framed
/// nodes — every node, or only the selected ones. Each node's rect comes
/// from [`CanvasGeometry::node_world_rect`] (current `NodeCtx::pos` +
/// cached measured size) — NOT from live responses, because culled
/// off-screen nodes record none and would drop out of the fit entirely.
/// A node that has never measured counts as a point at its position;
/// the fold is manual min/max (not `Rect::union`, which treats a
/// zero-size rect as identity and would discard that point too).
/// `None` when no node qualifies.
fn node_bounds(
    geometry: &CanvasGeometry,
    graph_ctx: GraphCtx<'_>,
    bounded: Bounded,
) -> Option<Rect> {
    let mut acc: Option<(Vec2, Vec2)> = None;
    for n in graph_ctx.nodes() {
        if bounded == Bounded::Selected && !graph_ctx.is_selected(n.id) {
            continue;
        }
        let rect = geometry.node_world_rect(n).unwrap_or(Rect {
            min: n.pos,
            size: Size::ZERO,
        });
        acc = Some(match acc {
            Some((min, max)) => (min.min(rect.min), max.max(rect.max())),
            None => (rect.min, rect.max()),
        });
    }
    acc.map(|(min, max)| Rect {
        min,
        size: Size::new(max.x - min.x, max.y - min.y),
    })
}

/// Fit `bounds` (world coords) centered in a `viewport`-sized pane,
/// leaving [`FIT_MARGIN`] on every side. The scale is the tighter of the
/// two per-axis fits, never magnified past 1:1 (a lone small node
/// shouldn't balloon), and clamped to [`CANVAS_ZOOM`]. Placing the
/// bbox center at the viewport center uses the same `outer_local = pan +
/// scale * world` mapping the inner-canvas transform applies.
fn fit_target(bounds: Rect, pane: Vec2) -> Viewport {
    let avail_x = (pane.x - 2.0 * FIT_MARGIN).max(1.0);
    let avail_y = (pane.y - 2.0 * FIT_MARGIN).max(1.0);
    // A sub-pixel extent (single node, or a flat row/column) doesn't
    // constrain its axis — treat it as unbounded so the other axis wins.
    let sx = if bounds.size.w > 1.0 {
        avail_x / bounds.size.w
    } else {
        f32::INFINITY
    };
    let sy = if bounds.size.h > 1.0 {
        avail_y / bounds.size.h
    } else {
        f32::INFINITY
    };
    let zoom = CANVAS_ZOOM.clamp(sx.min(sy).min(1.0));
    let pan = pane * 0.5 - bounds.center() * zoom;
    Viewport { pan, zoom }
}

#[cfg(test)]
mod tests;
