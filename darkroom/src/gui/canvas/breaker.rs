use glam::Vec2;
use palantir::{LineCap, LineJoin, PointerButton, PolylineColors, Rect, Shape, Ui};
use scenarium::graph::{Binding, InputPort, Subscription};
use scenarium::prelude::NodeId;

use crate::core::edit::intent::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::{CanvasGesture, outer_canvas_widget_id, to_world};
use crate::gui::scene::Scene;

/// Per-frame bundle threaded through node and connection rendering.
/// Carries `canvas_origin` (subtracted from `layout_rect` to convert
/// surface-space rects into the inner canvas's pre-transform frame,
/// matching the breaker's polyline) and the optional active gesture.
/// Passed as `&mut BreakerProbe<'_>` so Rust auto-reborrows at each
/// nested call.
pub(crate) struct BreakerProbe<'a> {
    pub(crate) origin: Vec2,
    pub(crate) state: Option<&'a mut BreakerState>,
}

impl BreakerProbe<'_> {
    /// True if the active breaker polyline crosses the cubic `p0..p3`. A
    /// no-op (false) when no breaker gesture is live, so wire renderers can
    /// call it unconditionally before deciding whether to record a cut.
    pub(crate) fn crosses_cubic(&self, p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2) -> bool {
        self.state
            .as_deref()
            .is_some_and(|b| b.intersects_cubic(p0, p1, p2, p3))
    }
}

/// Polyline samples closer than this (in inner-canvas world units)
/// are dropped — keeps the breaker from accumulating sub-pixel
/// duplicates on a slow drag.
const MIN_POINT_DISTANCE: f32 = 4.0;
/// Hard cap on the total polyline length. Once hit, further points
/// stop appending; the last segment is clamped to land exactly on
/// the limit. Matches the deprecated breaker.
const MAX_BREAKER_LENGTH: f32 = 2000.0;
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
    pub(crate) points: Vec<Vec2>,
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
    pub(crate) broken: Vec<InputPort>,
    /// Nodes whose body rect the breaker crosses this frame. Filled
    /// by `NodeUI::draw_all`, drained on release into
    /// `Intent::RemoveNode`. Same one-visit-per-node guarantee.
    pub(crate) broken_nodes: Vec<NodeId>,
    /// Event subscriptions whose wire the breaker intersects this frame.
    /// Filled by `EventConnectionUI::draw`, drained on release into
    /// `Intent::Unsubscribe`. Same one-visit-per-edge guarantee as `broken`.
    pub(crate) broken_subscriptions: Vec<Subscription>,
}

impl BreakerState {
    pub(crate) fn start(p: Vec2, button: PointerButton) -> Self {
        Self {
            points: vec![p],
            length: 0.0,
            button,
            broken: Vec::new(),
            broken_nodes: Vec::new(),
            broken_subscriptions: Vec::new(),
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

/// Owns the active connection-breaker gesture (RMB / Cmd+LMB drag on
/// the outer canvas). The state is `Option<BreakerState>` rather than
/// flat fields so the absence of a gesture is one variant and the
/// gesture can be cancelled by a single assignment. Hands out a
/// `BreakerProbe` to the canvas record so node and connection draws
/// can flag intersections inline.
#[derive(Default, Debug)]
pub(crate) struct BreakerUI {
    state: Option<BreakerState>,
}

impl BreakerUI {
    /// Drive the gesture from the outer canvas response: start, extend,
    /// release. On release, drain `broken` / `broken_nodes` into
    /// `RemoveNode` + `SetInput { to: None }` intents. `RemoveNode`
    /// supersedes any per-input unbind on the same target — the undo
    /// step already detaches incoming edges, so emitting both would
    /// log a redundant history entry. Esc cancels without emitting.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        gesture: Option<CanvasGesture>,
        out: &mut Vec<Intent>,
    ) {
        let resp = ui.response_for(outer_canvas_widget_id());
        // The classifier resolves RMB-drag vs Ctrl+LMB-drag and hands back
        // the latching button, which the gesture polls for continuation.
        if let Some(CanvasGesture::Breaker(button)) = gesture
            && self.state.is_none()
            && let Some(p) = resp.pointer_local
        {
            self.state = Some(BreakerState::start(to_world(p, scene), button));
        }
        if self.state.is_some() && ui.escape_pressed() {
            self.state = None;
            return;
        }
        let button = self.state.as_ref().map(|b| b.button);
        match (
            self.state.as_mut(),
            button.and_then(|b| resp.drag_delta_by(b)),
        ) {
            (Some(b), Some(_)) => {
                if let Some(p) = resp.pointer_local {
                    b.add_point(to_world(p, scene));
                }
            }
            (Some(b), None) => {
                let doomed_nodes = std::mem::take(&mut b.broken_nodes);
                for &node_id in &doomed_nodes {
                    out.push(Intent::RemoveNode { node_id });
                }
                for addr in b.broken.drain(..) {
                    if doomed_nodes.contains(&addr.node_id) {
                        continue;
                    }
                    out.push(Intent::SetInput {
                        node_id: addr.node_id,
                        input_idx: addr.port_idx,
                        to: Binding::None,
                    });
                }
                // A removed node already drops its subscriptions (RemoveNode's
                // undo step captures every edge touching it), so skip any
                // whose emitter or subscriber is doomed to avoid redundant
                // history.
                for s in b.broken_subscriptions.drain(..) {
                    if doomed_nodes.contains(&s.emitter) || doomed_nodes.contains(&s.subscriber) {
                        continue;
                    }
                    out.push(Intent::Unsubscribe {
                        emitter: s.emitter,
                        event_idx: s.event_idx,
                        subscriber: s.subscriber,
                    });
                }
                self.state = None;
            }
            _ => {}
        }
    }

    /// Hand the active state to inline intersection consumers (node
    /// body hit-test + connection draw). Borrow lives until the
    /// returned `BreakerProbe` is dropped.
    pub(crate) fn probe(&mut self, origin: Vec2) -> BreakerProbe<'_> {
        BreakerProbe {
            origin,
            state: self.state.as_mut(),
        }
    }

    /// Paint the polyline. No-op when no gesture is active or the
    /// polyline has < 2 samples (a `start` with no `add_point`).
    pub(crate) fn draw(&self, ui: &mut Ui, ctx: &AppContext<'_>) {
        let Some(b) = self.state.as_ref() else {
            return;
        };
        if b.points.len() < 2 {
            return;
        }
        ui.add_shape(Shape::Polyline {
            points: &b.points,
            colors: PolylineColors::Single(ctx.theme.breaker_stroke),
            width: ctx.theme.breaker_stroke_width,
            cap: LineCap::Round,
            join: LineJoin::Round,
        });
    }
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
        assert_eq!(b.points.len(), 1, "sub-4px samples must be dropped");
        b.add_point(Vec2::new(10.0, 0.0));
        assert_eq!(b.points.len(), 2);
    }

    #[test]
    fn add_point_caps_total_length() {
        // Past MAX_BREAKER_LENGTH the last segment is clamped and
        // further pushes are no-ops. Hand-computed: starting at 0 and
        // pushing (3000, 0) has seg = 3000 > remaining = 2000, so t =
        // 2000/3000 and the appended point lands at exactly
        // (2000, 0) — the cap.
        let mut b = BreakerState::start(Vec2::ZERO, PointerButton::Right);
        b.add_point(Vec2::new(3000.0, 0.0));
        assert_eq!(b.points.len(), 2);
        assert!((b.points[1].x - MAX_BREAKER_LENGTH).abs() < 1e-4);
        let before = b.points.len();
        b.add_point(Vec2::new(4000.0, 0.0));
        assert_eq!(b.points.len(), before, "no append past cap");
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
