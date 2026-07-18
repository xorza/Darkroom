//! Record-time visibility culling for the graph canvas. Only items that
//! intersect the viewport are recorded, so an off-screen
//! graph costs no measure/arrange/paint work. Pure world-space math;
//! [`crate::gui::canvas::GraphUI::frame`] resolves one [`CullRegion`] per
//! frame and threads the same policy through every recorded canvas item.

use aperture::{Rect, Size};
use glam::Vec2;

use crate::core::document::Viewport;
use crate::gui::canvas::to_world;
use crate::gui::canvas::wire::CubicHandles;

/// World-space slack added around the viewport so paint that extends past
/// an element's layout rect (status-glow shadow, wire stroke width,
/// selection border) never pops at the screen edge.
const CULL_MARGIN: f32 = 16.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct CullRegion {
    visible: Option<Rect>,
}

impl CullRegion {
    pub(crate) fn from_canvas(
        outer_screen: Option<Rect>,
        canvas_origin: Vec2,
        viewport: &Viewport,
    ) -> Self {
        let visible = outer_screen.map(|outer| {
            let outer_local = Rect {
                min: outer.min - canvas_origin,
                size: outer.size,
            };
            let min = to_world(outer_local.min, viewport);
            let max = to_world(outer_local.max(), viewport);
            Rect {
                min,
                size: Size::new(max.x - min.x, max.y - min.y),
            }
            .inflated(CULL_MARGIN)
        });
        Self { visible }
    }

    /// Unmeasured nodes stay recorded until their size becomes known.
    pub(crate) fn keeps_node(self, rect: Option<Rect>) -> bool {
        rect.is_none_or(|rect| self.keeps_rect(rect))
    }

    pub(crate) fn keeps_wire(self, p0: Vec2, handles: &CubicHandles, p3: Vec2) -> bool {
        // A cubic stays inside its control-point hull, so this bound is conservative.
        let min = p0.min(handles.p1).min(handles.p2).min(p3);
        let max = p0.max(handles.p1).max(handles.p2).max(p3);
        self.keeps_rect(Rect {
            min,
            size: Size::new(max.x - min.x, max.y - min.y),
        })
    }

    pub(crate) fn keeps_pin(
        self,
        card: Rect,
        port_center: Vec2,
        handles: &CubicHandles,
        wire_end: Vec2,
    ) -> bool {
        self.keeps_rect(card) || self.keeps_wire(port_center, handles, wire_end)
    }

    fn keeps_rect(self, rect: Rect) -> bool {
        self.visible.is_none_or(|visible| visible.intersects(rect))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::canvas::wire::cubic_handles;

    #[test]
    fn region_inverts_the_canvas_transform() {
        // outer = pan + zoom * world, so world = (outer - pan) / zoom.
        // Screen 800x600 at pan (100, 50), zoom 2: world min =
        // ((0,0) - (100,50)) / 2 = (-50,-25), size = (800,600)/2 =
        // (400,300); the 16px margin then grows every side.
        let vp = Viewport {
            pan: Vec2::new(100.0, 50.0),
            zoom: 2.0,
        };
        let region = CullRegion::from_canvas(
            Some(Rect::new(40.0, 30.0, 800.0, 600.0)),
            Vec2::new(40.0, 30.0),
            &vp,
        );
        let r = region.visible.unwrap();
        assert_eq!(r.min, Vec2::new(-66.0, -41.0));
        assert_eq!(r.size, Size::new(432.0, 332.0));

        // Different pan/zoom → different world rect (params matter):
        // zoom 0.5 halves world density, so the same screen shows 4x the
        // area; pan 0 pins world min at the margin only.
        let vp = Viewport {
            pan: Vec2::ZERO,
            zoom: 0.5,
        };
        let r2 = CullRegion::from_canvas(Some(Rect::new(0.0, 0.0, 800.0, 600.0)), Vec2::ZERO, &vp)
            .visible
            .unwrap();
        assert_eq!(r2.min, Vec2::new(-16.0, -16.0));
        assert_eq!(r2.size, Size::new(1632.0, 1232.0));
        assert_ne!(r.size, r2.size);

        let unmeasured = CullRegion::from_canvas(None, Vec2::new(50.0, 25.0), &vp);
        assert_eq!(unmeasured.visible, None);
    }

    #[test]
    fn node_cull_cases() {
        let region = CullRegion {
            visible: Some(Rect::new(0.0, 0.0, 100.0, 100.0)),
        };
        let body = |x: f32, y: f32| Some(Rect::new(x, y, 10.0, 10.0));
        // Inside, straddling the edge, fully outside.
        assert!(region.keeps_node(body(50.0, 50.0)));
        assert!(region.keeps_node(body(-5.0, 95.0)));
        assert!(!region.keeps_node(body(200.0, 0.0)));
        assert!(!region.keeps_node(body(0.0, -50.0)));
        // Unmeasured node or unmeasured canvas: always record.
        assert!(region.keeps_node(None));
        assert!(!region.keeps_node(body(9999.0, 9999.0)));
        assert!(CullRegion { visible: None }.keeps_node(body(9999.0, 9999.0)));
    }

    #[test]
    fn wire_cull_uses_the_control_hull() {
        let region = CullRegion {
            visible: Some(Rect::new(0.0, 0.0, 100.0, 100.0)),
        };
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
        assert!(region.keeps_wire(p0, &h, p3));
        // Entirely right of the view.
        let (p0, h, p3) = flat(200.0, 300.0);
        assert!(!region.keeps_wire(p0, &h, p3));
        // Endpoints outside the view, but a handle drags the hull in —
        // the curve can bow into the viewport, so it must not cull.
        let p0 = Vec2::new(150.0, -50.0);
        let p3 = Vec2::new(150.0, 150.0);
        let h = CubicHandles {
            p1: Vec2::new(50.0, -50.0),
            p2: Vec2::new(50.0, 150.0),
        };
        assert!(region.keeps_wire(p0, &h, p3));
        // No viewport → no culling.
        let (p0, h, p3) = flat(200.0, 300.0);
        assert!(CullRegion { visible: None }.keeps_wire(p0, &h, p3));
    }

    #[test]
    fn pin_cull_keeps_either_visible_part() {
        let region = CullRegion {
            visible: Some(Rect::new(0.0, 0.0, 100.0, 100.0)),
        };
        let cases = [
            (
                "card intersects while wire stays left",
                Vec2::new(-500.0, 50.0),
                Vec2::new(-250.0, 25.0),
                true,
            ),
            (
                "wire crosses while card stays right",
                Vec2::new(50.0, 50.0),
                Vec2::new(200.0, 50.0),
                true,
            ),
            (
                "wire and card both stay right",
                Vec2::new(500.0, 50.0),
                Vec2::new(700.0, 50.0),
                false,
            ),
        ];

        for (label, port_center, top_left, expected) in cases {
            let handles = cubic_handles(port_center, top_left);
            let card = Rect::new(top_left.x, top_left.y, 280.0, 200.0);
            assert_eq!(
                region.keeps_pin(card, port_center, &handles, top_left),
                expected,
                "{label}"
            );
        }
    }
}
