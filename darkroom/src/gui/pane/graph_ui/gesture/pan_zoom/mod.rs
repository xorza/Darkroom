//! Canvas viewport gesture: middle-drag pan, wheel/pinch zoom-about-cursor
//! within the canvas's [`ZoomRange`], and the toolbar's framings. Split out of
//! `canvas` so the orchestration there isn't tangled with the (independently
//! testable) viewport algebra. The gesture emits `GraphIntent::SetViewport`,
//! so pan/zoom rides the same undo path as every other edit.

pub(crate) mod camera_gesture;

use glam::Vec2;
use palantir::{Rect, Size, Ui};

use crate::core::document::viewport::Viewport;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry;
use crate::gui::widgets::zoom_range::ZoomRange;

/// The canvas's zoom range.
const CANVAS_ZOOM: ZoomRange = ZoomRange { min: 0.1, max: 5.0 };

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
