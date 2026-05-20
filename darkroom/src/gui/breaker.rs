use glam::Vec2;
use palantir::{PointerButton, Rect};
use scenarium::graph::PortAddress;
use scenarium::prelude::NodeId;

/// Per-frame bundle threaded through node and connection rendering.
/// Carries `canvas_origin` (subtracted from `layout_rect` to convert
/// surface-space rects into the inner canvas's pre-transform frame,
/// matching the breaker's polyline) and the optional active gesture.
/// Passed as `&mut BreakerProbe<'_>` so Rust auto-reborrows at each
/// nested call.
pub(crate) struct BreakerProbe<'a> {
    pub origin: Vec2,
    pub state: Option<&'a mut BreakerState>,
}

/// Polyline samples closer than this (in inner-canvas world units)
/// are dropped — keeps the breaker from accumulating sub-pixel
/// duplicates on a slow drag.
const MIN_POINT_DISTANCE: f32 = 4.0;
/// Hard cap on the total polyline length. Once hit, further points
/// stop appending; the last segment is clamped to land exactly on
/// the limit. Matches the deprecated breaker.
const MAX_BREAKER_LENGTH: f32 = 900.0;
/// Bezier sampling resolution for hit-testing. 16 segments matches
/// the deprecated implementation's `ensure_sampled` density and is
/// cheap enough to redo every frame for every visible connection.
const BEZIER_SAMPLES: usize = 16;

/// Active connection-breaker gesture. Lives in inner-canvas world
/// (pre-transform) coords so render inside the inner canvas can use
/// the points verbatim and intersection tests share the same frame
/// as the cubic bezier endpoints.
#[derive(Debug)]
pub(crate) struct BreakerState {
    points: Vec<Vec2>,
    length: f32,
    /// Mouse button that latched this gesture. The release-detection
    /// check polls `drag_delta_by(button)`, so a Cmd+LMB-launched
    /// breaker must keep reading the Left button, not Right.
    pub(crate) button: PointerButton,
    /// Target input ports whose data binding the breaker intersects
    /// this frame. Filled by `draw_connections`, drained on release
    /// into `Intent::SetInput { to: Binding::None }`. A `Vec` is
    /// enough — each connection is visited exactly once per frame,
    /// so within-frame duplicates aren't possible.
    pub(crate) broken: Vec<PortAddress>,
    /// Nodes whose body rect the breaker crosses this frame. Filled
    /// by `NodeUI::draw_all`, drained on release into
    /// `Intent::RemoveNode`. Same one-visit-per-node guarantee.
    pub(crate) broken_nodes: Vec<NodeId>,
}

impl BreakerState {
    pub(crate) fn start(p: Vec2, button: PointerButton) -> Self {
        Self {
            points: vec![p],
            length: 0.0,
            button,
            broken: Vec::new(),
            broken_nodes: Vec::new(),
        }
    }

    pub(crate) fn add_point(&mut self, p: Vec2) {
        let last = *self.points.last().unwrap();
        let seg = last.distance(p);
        if seg <= MIN_POINT_DISTANCE {
            return;
        }
        let remaining = MAX_BREAKER_LENGTH - self.length;
        if remaining <= 0.0 {
            return;
        }
        let (clamped, added) = if seg <= remaining {
            (p, seg)
        } else {
            let t = remaining / seg;
            (last + (p - last) * t, remaining)
        };
        self.points.push(clamped);
        self.length += added;
    }

    pub(crate) fn points(&self) -> &[Vec2] {
        &self.points
    }

    fn segments(&self) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
        self.points.windows(2).map(|w| (w[0], w[1]))
    }

    /// True if the breaker polyline crosses `rect`: either any sample
    /// falls inside, or any breaker segment crosses one of the four
    /// edges. `rect` is in the same frame as the polyline (inner-
    /// canvas pre-transform world coords).
    pub(crate) fn intersects_rect(&self, rect: Rect) -> bool {
        if self.points.is_empty() {
            return false;
        }
        let min = rect.min;
        let max = rect.max();
        let inside = |p: Vec2| p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y;
        if self.points.iter().any(|&p| inside(p)) {
            return true;
        }
        let edges = [
            (Vec2::new(min.x, min.y), Vec2::new(max.x, min.y)),
            (Vec2::new(max.x, min.y), Vec2::new(max.x, max.y)),
            (Vec2::new(max.x, max.y), Vec2::new(min.x, max.y)),
            (Vec2::new(min.x, max.y), Vec2::new(min.x, min.y)),
        ];
        for (a, b) in self.segments() {
            for &(e0, e1) in &edges {
                if segments_intersect(a, b, e0, e1) {
                    return true;
                }
            }
        }
        false
    }

    /// True if any cubic-bezier sample-segment crosses any breaker
    /// segment. Samples the bezier into `BEZIER_SAMPLES` chords; this
    /// runs once per connection per frame while the gesture is
    /// active, so we don't cache.
    pub(crate) fn intersects_cubic(&self, p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2) -> bool {
        if self.points.len() < 2 {
            return false;
        }
        let mut prev = p0;
        for i in 1..=BEZIER_SAMPLES {
            let t = i as f32 / BEZIER_SAMPLES as f32;
            let next = cubic_point(p0, p1, p2, p3, t);
            for (b0, b1) in self.segments() {
                if segments_intersect(prev, next, b0, b1) {
                    return true;
                }
            }
            prev = next;
        }
        false
    }
}

fn cubic_point(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, t: f32) -> Vec2 {
    let u = 1.0 - t;
    let uu = u * u;
    let tt = t * t;
    p0 * (uu * u) + p1 * (3.0 * uu * t) + p2 * (3.0 * u * tt) + p3 * (tt * t)
}

/// Standard 2D segment–segment intersection: proper-crossing only
/// (no collinear-overlap), which is enough for "did the breaker
/// scribble cross this wire?".
fn segments_intersect(a1: Vec2, a2: Vec2, b1: Vec2, b2: Vec2) -> bool {
    let o1 = orient(a1, a2, b1);
    let o2 = orient(a1, a2, b2);
    let o3 = orient(b1, b2, a1);
    let o4 = orient(b1, b2, a2);
    (o1 * o2 < 0.0) && (o3 * o4 < 0.0)
}

fn orient(p: Vec2, q: Vec2, r: Vec2) -> f32 {
    (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_point_skips_short_segments() {
        // Samples below MIN_POINT_DISTANCE are dropped — a slow drag
        // that crawls 1px/frame must not accumulate one point per frame.
        let mut b = BreakerState::start(Vec2::ZERO, PointerButton::Right);
        b.add_point(Vec2::new(1.0, 0.0));
        b.add_point(Vec2::new(2.0, 0.0));
        b.add_point(Vec2::new(3.0, 0.0));
        assert_eq!(b.points().len(), 1, "sub-4px samples must be dropped");
        b.add_point(Vec2::new(10.0, 0.0));
        assert_eq!(b.points().len(), 2);
    }

    #[test]
    fn add_point_caps_total_length() {
        // Past MAX_BREAKER_LENGTH the last segment is clamped and
        // further pushes are no-ops. Hand-computed: starting at 0
        // and pushing (1000, 0) lands the second point at exactly
        // (900, 0) — the cap.
        let mut b = BreakerState::start(Vec2::ZERO, PointerButton::Right);
        b.add_point(Vec2::new(1000.0, 0.0));
        assert_eq!(b.points().len(), 2);
        assert!((b.points()[1].x - MAX_BREAKER_LENGTH).abs() < 1e-4);
        let before = b.points().len();
        b.add_point(Vec2::new(2000.0, 0.0));
        assert_eq!(b.points().len(), before, "no append past cap");
    }

    #[test]
    fn intersects_cubic_diagonal_through_straight_wire() {
        // Straight horizontal cubic from (0,0) to (100,0). A breaker
        // segment crossing it transversely must register. Vertical
        // breaker at x=50, y from -10 to +10 — proper crossing at
        // (50, 0), nowhere near a cubic endpoint (which would be a
        // degenerate "touch at vertex" the strict-crossing test
        // intentionally rejects).
        let mut b = BreakerState::start(Vec2::new(50.0, -10.0), PointerButton::Right);
        b.add_point(Vec2::new(50.0, 10.0));
        assert!(b.intersects_cubic(
            Vec2::new(0.0, 0.0),
            Vec2::new(33.0, 0.0),
            Vec2::new(66.0, 0.0),
            Vec2::new(100.0, 0.0),
        ));
    }

    #[test]
    fn intersects_cubic_misses_parallel_polyline() {
        // Breaker runs parallel to the wire well below it — no crossing.
        let mut b = BreakerState::start(Vec2::new(0.0, 50.0), PointerButton::Right);
        b.add_point(Vec2::new(100.0, 50.0));
        assert!(!b.intersects_cubic(
            Vec2::new(0.0, 0.0),
            Vec2::new(33.0, 0.0),
            Vec2::new(66.0, 0.0),
            Vec2::new(100.0, 0.0),
        ));
    }

    #[test]
    fn intersects_cubic_empty_breaker_is_false() {
        // Single-point breaker (no segments yet) can't intersect.
        let b = BreakerState::start(Vec2::ZERO, PointerButton::Right);
        assert!(!b.intersects_cubic(
            Vec2::ZERO,
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(3.0, 0.0),
        ));
    }
}
