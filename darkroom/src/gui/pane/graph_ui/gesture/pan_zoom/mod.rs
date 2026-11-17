//! Canvas viewport gesture: middle-drag and scroll pan, command-scroll and pinch
//! zoom-about-cursor within the canvas's [`ZoomRange`], and the camera commands' framings. The
//! gesture emits `GraphIntent::SetViewport`, the one way the document's camera moves.

pub(crate) mod camera_gesture;

use std::f32::consts::{FRAC_1_SQRT_2, SQRT_2};

use glam::Vec2;
use palantir::{Rect, Size, Ui};

use crate::core::document::item_id::ItemId;
use crate::core::document::viewport::Viewport;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::framing::{Framing, ZoomStep};
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::scene::canvas_geometry::CanvasGeometry;
use crate::gui::widgets::zoom_range::ZoomRange;

/// The canvas's zoom range.
const CANVAS_ZOOM: ZoomRange = ZoomRange { min: 0.1, max: 5.0 };

/// Breathing room (logical px) left on every side when fitting content to the viewport.
const FIT_MARGIN: f32 = 40.0;

/// The zooms Zoom In and Zoom Out step between: `2^(n/2)` inside [`CANVAS_ZOOM`], and its two
/// ends. Each rung is a power of two or one times `√2`, so none carries a rounding of the rung
/// before it.
const ZOOM_LADDER: [f32; 13] = [
    CANVAS_ZOOM.min,
    0.125,
    0.25 * FRAC_1_SQRT_2,
    0.25,
    0.5 * FRAC_1_SQRT_2,
    0.5,
    FRAC_1_SQRT_2,
    1.0,
    SQRT_2,
    2.0,
    2.0 * SQRT_2,
    4.0,
    CANVAS_ZOOM.max,
];

/// How near a rung, as a share of it, a zoom counts as on it: a scroll leaves the zoom anywhere,
/// and a step shorter than a thousandth of it would move a 1000 px pane's edge by under a pixel,
/// a press that shows nothing.
const RUNG_SLACK: f32 = 1e-3;

impl Framing {
    /// The `SetViewport` intent this framing implies on the canvas, or `None` when there's
    /// nothing to frame: `Selection` with an empty selection, no nodes to fit, a step past the
    /// ladder's end, or the viewport not yet measured. The pane size comes from the outer canvas's `layout_rect`;
    /// node extents come from the geometry's cross-frame size cache, position from
    /// `NodeCtx::pos`.
    pub(crate) fn intent(
        self,
        ui: &Ui,
        graph_ctx: GraphCtx<'_>,
        geometry: &CanvasGeometry,
    ) -> Option<GraphIntent> {
        let vp = ui
            .response_for(canvas::outer_wid(graph_ctx.graph()))
            .layout_rect?
            .size;
        let pane = Vec2::new(vp.w, vp.h);
        let to = match self {
            Self::Actual => actual_target(geometry, graph_ctx, pane),
            Self::Fit => fit_target(node_bounds(geometry, graph_ctx, Bounded::All)?, pane),
            Self::Selection => {
                if graph_ctx.selected().is_empty() {
                    return None;
                }
                fit_target(node_bounds(geometry, graph_ctx, Bounded::Selected)?, pane)
            }
            Self::Step(step) => step_target(graph_ctx.view().viewport, step, pane)?,
        };
        Some(GraphIntent::SetViewport { to })
    }
}

/// 1:1 zoom, centered on all content (world origin when the graph is empty).
fn actual_target(geometry: &CanvasGeometry, graph_ctx: GraphCtx<'_>, pane: Vec2) -> Viewport {
    let pan = match node_bounds(geometry, graph_ctx, Bounded::All) {
        Some(b) => pane * 0.5 - b.center(),
        None => Vec2::ZERO,
    };
    Viewport { pan, zoom: 1.0 }
}

/// `from` one rung along [`ZOOM_LADDER`], the content at the pane's center held there: `In` to
/// the lowest rung above the zoom, `Out` to the highest below it, so a step from between two
/// rungs lands on one and two opposite steps return. `None` at the ladder's end.
fn step_target(from: Viewport, step: ZoomStep, pane: Vec2) -> Option<Viewport> {
    let zoom = match step {
        ZoomStep::In => ZOOM_LADDER
            .into_iter()
            .find(|&rung| rung > from.zoom * (1.0 + RUNG_SLACK)),
        ZoomStep::Out => ZOOM_LADDER
            .into_iter()
            .rfind(|&rung| rung < from.zoom * (1.0 - RUNG_SLACK)),
    }?;
    let mut to = from;
    CANVAS_ZOOM.zoom_about(&mut to, pane * 0.5, zoom / from.zoom);
    // The rung itself, not the product that reached it.
    to.zoom = zoom;
    Some(to)
}

/// Which nodes a framing bounds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Bounded {
    All,
    Selected,
}

/// World-space (inner-canvas pre-transform) bounding box of the framed nodes, every node or
/// only the selected ones. Each node's rect comes from [`CanvasGeometry::node_world_rect`]
/// (current `NodeCtx::pos` plus cached measured size), not from live responses, as culled
/// off-screen nodes record none and would drop out of the fit. A node that has never measured
/// counts as a point at its position; the fold is manual min/max, as `Rect::union` treats a
/// zero-size rect as identity and would discard that point. `None` when no node qualifies.
fn node_bounds(
    geometry: &CanvasGeometry,
    graph_ctx: GraphCtx<'_>,
    bounded: Bounded,
) -> Option<Rect> {
    let mut acc: Option<(Vec2, Vec2)> = None;
    for n in graph_ctx.nodes() {
        if bounded == Bounded::Selected && !graph_ctx.is_selected(ItemId::Node(n.node_id)) {
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
    acc.map(|(min, max)| Rect::from_min_max(min, max))
}

/// Fit `bounds` (world coords) centered in a `viewport`-sized pane, leaving [`FIT_MARGIN`] on
/// every side. The scale is the tighter of the two per-axis fits, never magnified past 1:1 (a
/// lone small node shouldn't balloon), and clamped to [`CANVAS_ZOOM`]. The bbox center lands at
/// the viewport center through the inner-canvas transform's `outer_local = pan + scale * world`.
fn fit_target(bounds: Rect, pane: Vec2) -> Viewport {
    let avail_x = (pane.x - 2.0 * FIT_MARGIN).max(1.0);
    let avail_y = (pane.y - 2.0 * FIT_MARGIN).max(1.0);
    // A sub-pixel extent (single node, or a flat row/column) doesn't constrain its axis, so the
    // other axis wins.
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
