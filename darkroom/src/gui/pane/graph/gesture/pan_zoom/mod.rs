//! Canvas viewport gesture: middle-drag pan, wheel/pinch zoom-about-
//! cursor, and the zoom-factor math. Split out of `canvas` so the
//! orchestration there isn't tangled with the (independently testable)
//! viewport algebra. The gesture emits `GraphIntent::SetViewport`, so pan/zoom
//! rides the same undo path as every other edit.

use common::FloatExt;
use glam::Vec2;
use palantir::{Rect, ResponseState, Size, Ui};

use crate::core::document::Viewport;
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::frame::geometry::CanvasGeometry;
use crate::gui::pane::graph::gesture::slot::GestureSlot;
use crate::gui::pane::graph::{CanvasGesture, outer_canvas_widget_id};

/// Fold a live pan drag into `pan`: `anchor + delta` while the drag is
/// held; a missing delta after a latch is the release edge and drops the
/// anchor. A call before anything latched does nothing at all — neither
/// panning nor releasing.
///
/// Measured from the latch rather than integrated per frame, so a pan
/// lands exactly where the pointer says however many frames it took (no
/// per-frame rounding drift).
pub(super) fn fold_pan_drag(anchor: &mut GestureSlot<Vec2>, delta: Option<Vec2>, pan: &mut Vec2) {
    let Some(&start) = anchor.get() else {
        return;
    };
    match delta {
        Some(d) => *pan = start + d,
        None => anchor.clear(),
    }
}

/// Fold one frame's scroll/pinch deltas from `resp` into `v` — the
/// shared half of the canvas and image-viewer pan/zoom gestures (drag
/// panning stays with each caller, whose button policy differs):
/// two-finger `scroll_pixels` pan, wheel `scroll_lines` zoom-about-
/// cursor, and pinch `zoom_factor` zoom-about-cursor, clamped to the
/// caller's `[min_zoom, max_zoom]`.
pub(crate) fn fold_scroll_zoom(
    v: &mut Viewport,
    ui: &Ui,
    resp: &ResponseState,
    min_zoom: f32,
    max_zoom: f32,
) {
    if resp.scroll.pixels != Vec2::ZERO {
        v.pan -= resp.scroll.pixels;
    }
    if resp.scroll.lines.y.abs() > f32::EPSILON
        && let Some(pivot) = resp.pointer_local
    {
        let line_px = ui.theme.text.line_height_for(ui.theme.text.font_size_px);
        zoom_about(
            &mut v.pan,
            &mut v.zoom,
            pivot,
            scroll_to_zoom_factor(resp.scroll.lines.y * line_px),
            min_zoom,
            max_zoom,
        );
    }
    if (resp.scroll.zoom - 1.0).abs() > f32::EPSILON
        && let Some(pivot) = resp.pointer_local
    {
        zoom_about(
            &mut v.pan,
            &mut v.zoom,
            pivot,
            resp.scroll.zoom,
            min_zoom,
            max_zoom,
        );
    }
}

/// Bounds on the canvas zoom factor. Pinch / scroll-zoom deltas
/// multiply in; clamped each frame so pathological gestures can't
/// drive it to 0 (which would make the inverse transform explode) or
/// to a value so large that the world coordinates underflow.
///
/// Named apart from the image viewer's own (far wider)
/// `VIEWER_MIN_ZOOM`/`VIEWER_MAX_ZOOM` because both pairs are passed into the
/// shared [`fold_scroll_zoom`] / [`zoom_about`], where an unqualified
/// `MIN_ZOOM` at the call site wouldn't say which surface's range is in play.
const CANVAS_MIN_ZOOM: f32 = 0.1;
const CANVAS_MAX_ZOOM: f32 = 5.0;

/// Per-pixel base for converting wheel / touchpad scroll into a
/// multiplicative zoom factor. Tuned so a single classic wheel notch
/// (~16-20 logical px after palantir's line→pixel conversion) yields
/// roughly a 4-5% zoom step, while a fast touchpad swipe (~50-100 px
/// in one frame) stays a controlled ~13-22% step. Lower → slower
/// zoom, higher → snappier but jumps badly on touchpad.
const SCROLL_ZOOM_BASE: f32 = 1.0025;

/// Read the outer canvas's current-frame response, compute the
/// target viewport, and emit an `GraphIntent::SetViewport` when it
/// changed. The intent (not a direct write) is the only thing that
/// mutates the document's viewport — so pan/zoom rides the same
/// undo path as every other edit, and the undo stack coalesces a
/// continuous gesture into one entry via `GestureKey::Viewport`.
/// `pan_anchor` is the caller's drag-anchor slot (input bookkeeping,
/// one gesture's lifetime). Three independent sources:
///
/// - **Middle-button drag** (`Sense::DRAG` +
///   `Ui::drag_delta_by`): canvas pan. Anchor on `drag_started_by`,
///   then `pan = anchor + delta` until release. Left-drag is
///   intentionally NOT routed to pan so it stays free for future
///   rubber-band selection.
/// - **Scroll** (`Sense::SCROLL`): mouse wheel / touchpad swipe →
///   zoom-about-cursor (graph-editor convention: Figma / Blender
///   node editor / ComfyUI). Vertical delta only; horizontal is
///   ignored. Palantir ingests the scroll delta already-negated
///   so `+y` means "scroll content down" → zoom out, `-y` (wheel
///   up) → zoom in.
/// - **Pinch** (`Sense::PINCH`): zoom-about-cursor using the
///   `Response::pointer_local` pivot.
pub(crate) fn emit_pan_zoom(
    pan_anchor: &mut GestureSlot<Vec2>,
    ui: &Ui,
    graph_ctx: GraphCtx<'_>,
    gesture: Option<CanvasGesture>,
    out: &mut Intents,
) {
    let viewport = graph_ctx.viewport();
    let resp = ui.response_for(outer_canvas_widget_id());
    let mut v = viewport;
    // Pan latch comes from the central classification; continuation and
    // wheel/pinch zoom below read the response directly (not arbitration).
    if gesture == Some(CanvasGesture::Pan) {
        pan_anchor.latch(viewport.pan);
    }
    fold_pan_drag(pan_anchor, resp.middle.drag.delta(), &mut v.pan);
    fold_scroll_zoom(&mut v, ui, &resp, CANVAS_MIN_ZOOM, CANVAS_MAX_ZOOM);
    // Only emit when the gesture actually moved the viewport
    // (approx compare — exact float `!=` would emit on sub-epsilon
    // jitter). The `SetViewport` undo step is also `is_noop`-
    // filtered in `drain_intents`; this just skips the build on
    // idle frames.
    let unchanged = v.pan.approximately_eq(viewport.pan) && v.zoom.approximately_eq(viewport.zoom);
    if !unchanged {
        out.push(GraphIntent::SetViewport { to: v });
    }
}

/// Multiply `zoom` by `factor` while holding the pre-transform point
/// under `pivot_local` fixed in the pane. Operates on the caller's
/// local `(pan, zoom)` so a gesture can fold several inputs before
/// emitting/committing once. Standard zoom-about-cursor algebra: world
/// point under cursor = `(pivot - pan) / zoom`; choose new pan so that
/// same world point stays under the same screen pixel after scaling.
/// Clamps to `[min_zoom, max_zoom]` (callers pass their own range —
/// the canvas and the image viewer bound zoom differently); ignores
/// non-finite / non-positive factors. Shared with
/// [`crate::gui::pane::viewer`].
pub(crate) fn zoom_about(
    pan: &mut Vec2,
    zoom: &mut f32,
    pivot_local: Vec2,
    factor: f32,
    min_zoom: f32,
    max_zoom: f32,
) {
    if !factor.is_finite() || factor <= 0.0 {
        return;
    }
    let new_zoom = (*zoom * factor).clamp(min_zoom, max_zoom);
    let effective = new_zoom / *zoom;
    *pan = pivot_local - (pivot_local - *pan) * effective;
    *zoom = new_zoom;
}

/// Map a one-frame vertical scroll delta (in logical px, palantir's
/// "advance offset forward" sign convention — `+y` = scroll content
/// down) to a multiplicative zoom factor. Negative `delta_y` (wheel
/// up) zooms in (`factor > 1`); positive (wheel down) zooms out
/// (`factor < 1`). Pure function so it can be unit-tested without
/// spinning up a UI. Shared with [`crate::gui::pane::viewer`].
pub(super) fn scroll_to_zoom_factor(delta_y: f32) -> f32 {
    SCROLL_ZOOM_BASE.powf(-delta_y)
}

/// Breathing room (logical px) left on every side when fitting content
/// to the viewport, so framed nodes don't butt against the pane edge.
const FIT_MARGIN: f32 = 40.0;

/// A one-shot viewport-framing request from the graph toolbar. Each
/// resolves to an `GraphIntent::SetViewport`, so a reframe rides the same
/// undo path as a manual pan/zoom (and coalesces with it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ViewAction {
    /// Reset to 1:1 zoom, centered on all nodes (world origin when empty).
    Reset,
    /// Fit every node in the view.
    ShowAll,
    /// Fit the selected nodes; a no-op when nothing is selected.
    ShowSelected,
}

/// Compute the `SetViewport` intent a [`ViewAction`] implies, or `None`
/// when there's nothing to frame — `ShowSelected` with an empty
/// selection, no nodes to fit, or the viewport not yet measured. The
/// pane size comes from the outer canvas's `layout_rect`; node extents
/// come from `geometry`'s cross-frame size cache, position from
/// `SceneNode::pos`.
pub(crate) fn view_action_intent(
    ui: &Ui,
    geometry: &CanvasGeometry,
    graph_ctx: GraphCtx<'_>,
    action: ViewAction,
) -> Option<GraphIntent> {
    let vp = ui.response_for(outer_canvas_widget_id()).layout_rect?.size;
    let pane = Vec2::new(vp.w, vp.h);
    let to = match action {
        ViewAction::Reset => reset_target(geometry, graph_ctx, pane),
        ViewAction::ShowAll => fit_target(node_bounds(geometry, graph_ctx, false)?, pane),
        ViewAction::ShowSelected => {
            if graph_ctx.selected().is_empty() {
                return None;
            }
            fit_target(node_bounds(geometry, graph_ctx, true)?, pane)
        }
    };
    Some(GraphIntent::SetViewport { to })
}

/// 1:1 zoom, centered on all content (world origin when the graph is empty).
fn reset_target(geometry: &CanvasGeometry, graph_ctx: GraphCtx<'_>, pane: Vec2) -> Viewport {
    let pan = match node_bounds(geometry, graph_ctx, false) {
        Some(b) => pane * 0.5 - b.center(),
        None => Vec2::ZERO,
    };
    Viewport { pan, zoom: 1.0 }
}

/// World-space (inner-canvas pre-transform) bounding box of the framed
/// nodes — every node, or only the selected ones. Each node's rect comes
/// from [`CanvasGeometry::node_world_rect`] (current `SceneNode::pos` +
/// cached measured size) — NOT from live responses, because culled
/// off-screen nodes record none and would drop out of the fit entirely.
/// A node that has never measured counts as a point at its position;
/// the fold is manual min/max (not `Rect::union`, which treats a
/// zero-size rect as identity and would discard that point too).
/// `None` when no node qualifies.
fn node_bounds(
    geometry: &CanvasGeometry,
    graph_ctx: GraphCtx<'_>,
    selected_only: bool,
) -> Option<Rect> {
    let mut acc: Option<(Vec2, Vec2)> = None;
    for n in graph_ctx.nodes() {
        if selected_only && !graph_ctx.is_selected(n.id) {
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
/// shouldn't balloon), and clamped to `[CANVAS_MIN_ZOOM, CANVAS_MAX_ZOOM]`. Placing the
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
    let zoom = sx.min(sy).min(1.0).clamp(CANVAS_MIN_ZOOM, CANVAS_MAX_ZOOM);
    let pan = pane * 0.5 - bounds.center() * zoom;
    Viewport { pan, zoom }
}

#[cfg(test)]
mod tests;
