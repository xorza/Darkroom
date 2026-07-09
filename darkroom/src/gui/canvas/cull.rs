//! Record-time visibility culling for the graph canvas. Only nodes and
//! wires that intersect the viewport are recorded, so an off-screen
//! subgraph costs no measure/arrange/paint work. Pure world-space math;
//! [`crate::gui::canvas::GraphUI::frame`] resolves the visible world
//! rect once per frame and threads it into the node and wire draws.

use aperture::{Rect, Size};
use glam::Vec2;

use crate::core::document::Viewport;
use crate::gui::canvas::to_world;
use crate::gui::canvas::wire::CubicHandles;

/// World-space slack added around the viewport so paint that extends past
/// an element's layout rect (status-glow shadow, wire stroke width,
/// selection border) never pops at the screen edge.
const CULL_MARGIN: f32 = 16.0;

/// The world-space rect visible through the canvas, inflated by
/// [`CULL_MARGIN`]. `outer_local` is the outer canvas rect relative to
/// the inner canvas's pre-transform origin; [`to_world`] inverts the
/// inner transform for both corners.
pub(crate) fn visible_world_rect(outer_local: Rect, viewport: &Viewport) -> Rect {
    let min = to_world(outer_local.min, viewport);
    let max = to_world(outer_local.max(), viewport);
    Rect {
        min,
        size: Size::new(max.x - min.x, max.y - min.y),
    }
    .inflated(CULL_MARGIN)
}

/// Whether a node with world body `rect` can be visible. `rect` is `None`
/// for a node that has never measured
/// ([`crate::gui::canvas::geometry::CanvasGeometry::node_world_rect`]) —
/// record it so it gets a size. `visible` is `None` when the canvas
/// itself hasn't measured yet — no culling.
pub(crate) fn node_visible(visible: Option<Rect>, rect: Option<Rect>) -> bool {
    match (visible, rect) {
        (Some(vp), Some(rect)) => vp.intersects(rect),
        _ => true,
    }
}

/// Whether any part of a wire cubic can be visible. A bezier lies inside
/// the convex hull of its control points, so testing the hull's bounding
/// box is conservative (never culls a visible wire); stroke width is
/// covered by the viewport's [`CULL_MARGIN`].
pub(crate) fn wire_visible(
    visible: Option<Rect>,
    p0: Vec2,
    handles: &CubicHandles,
    p3: Vec2,
) -> bool {
    let Some(vp) = visible else {
        return true;
    };
    let min = p0.min(handles.p1).min(handles.p2).min(p3);
    let max = p0.max(handles.p1).max(handles.p2).max(p3);
    vp.intersects(Rect {
        min,
        size: Size::new(max.x - min.x, max.y - min.y),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_world_rect_inverts_the_canvas_transform() {
        // outer = pan + zoom * world, so world = (outer - pan) / zoom.
        // Screen 800x600 at pan (100, 50), zoom 2: world min =
        // ((0,0) - (100,50)) / 2 = (-50,-25), size = (800,600)/2 =
        // (400,300); the 16px margin then grows every side.
        let vp = Viewport {
            pan: Vec2::new(100.0, 50.0),
            zoom: 2.0,
        };
        let r = visible_world_rect(Rect::new(0.0, 0.0, 800.0, 600.0), &vp);
        assert_eq!(r.min, Vec2::new(-66.0, -41.0));
        assert_eq!(r.size, Size::new(432.0, 332.0));

        // Different pan/zoom → different world rect (params matter):
        // zoom 0.5 halves world density, so the same screen shows 4x the
        // area; pan 0 pins world min at the margin only.
        let vp = Viewport {
            pan: Vec2::ZERO,
            zoom: 0.5,
        };
        let r2 = visible_world_rect(Rect::new(0.0, 0.0, 800.0, 600.0), &vp);
        assert_eq!(r2.min, Vec2::new(-16.0, -16.0));
        assert_eq!(r2.size, Size::new(1632.0, 1232.0));
        assert_ne!(r.size, r2.size);

        // Degenerate zoom falls back to 1:1 like `to_world`.
        let vp = Viewport {
            pan: Vec2::ZERO,
            zoom: 0.0,
        };
        let r = visible_world_rect(Rect::new(0.0, 0.0, 100.0, 100.0), &vp);
        assert_eq!(r.size, Size::new(132.0, 132.0));
    }

    #[test]
    fn node_visible_cases() {
        let vp = Some(Rect::new(0.0, 0.0, 100.0, 100.0));
        let body = |x: f32, y: f32| Some(Rect::new(x, y, 10.0, 10.0));
        // Inside, straddling the edge, fully outside.
        assert!(node_visible(vp, body(50.0, 50.0)));
        assert!(node_visible(vp, body(-5.0, 95.0)));
        assert!(!node_visible(vp, body(200.0, 0.0)));
        assert!(!node_visible(vp, body(0.0, -50.0)));
        // Unmeasured node or unmeasured canvas: always record.
        assert!(node_visible(vp, None));
        assert!(!node_visible(vp, body(9999.0, 9999.0)));
        assert!(node_visible(None, body(9999.0, 9999.0)));
    }

    #[test]
    fn wire_visible_uses_the_control_hull() {
        let vp = Some(Rect::new(0.0, 0.0, 100.0, 100.0));
        let flat = |x0: f32, x1: f32| {
            // Level wire: zero-height hull box, handles between the ends.
            let h = CubicHandles {
                p1: Vec2::new(x0 + 10.0, 50.0),
                p2: Vec2::new(x1 - 10.0, 50.0),
            };
            (Vec2::new(x0, 50.0), h, Vec2::new(x1, 50.0))
        };

        // Both endpoints off-screen left/right, hull crosses the view.
        let (p0, h, p3) = flat(-50.0, 150.0);
        assert!(wire_visible(vp, p0, &h, p3));
        // Entirely right of the view.
        let (p0, h, p3) = flat(200.0, 300.0);
        assert!(!wire_visible(vp, p0, &h, p3));
        // Endpoints outside the view, but a handle drags the hull in —
        // the curve can bow into the viewport, so it must not cull.
        let p0 = Vec2::new(150.0, -50.0);
        let p3 = Vec2::new(150.0, 150.0);
        let h = CubicHandles {
            p1: Vec2::new(50.0, -50.0),
            p2: Vec2::new(50.0, 150.0),
        };
        assert!(wire_visible(vp, p0, &h, p3));
        // No viewport → no culling.
        let (p0, h, p3) = flat(200.0, 300.0);
        assert!(wire_visible(None, p0, &h, p3));
    }
}
