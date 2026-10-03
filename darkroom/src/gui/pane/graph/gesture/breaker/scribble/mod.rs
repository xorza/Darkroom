//! [`Scribble`]: the breaker's polyline, and the targets it crosses.

use glam::Vec2;
use palantir::Rect;
use scenarium::{InputPort, NodeId, Subscription};

/// Polyline samples closer than this (in inner-canvas world units)
/// are dropped — keeps the breaker from accumulating sub-pixel
/// duplicates on a slow drag.
const MIN_POINT_DISTANCE: f32 = 4.0;
/// Hard cap on the total polyline length. Once hit, further points
/// stop appending; the last segment is clamped to land exactly on
/// the limit. With [`MIN_POINT_DISTANCE`] spacing it holds a scribble to
/// 500 segments, which bounds the per-frame intersection work.
const MAX_BREAKER_LENGTH: f32 = 2000.0;
/// Bezier sampling resolution for hit-testing: 16 chords per wire, cheap
/// enough to redo every frame for every visible connection.
const BEZIER_SAMPLES: usize = 16;

/// The breaker scribble: its samples, how far they run, and the three target
/// sets a frame's probing marks against them. Lives in inner-canvas world
/// (pre-transform) coords so render inside the inner canvas can use the points
/// verbatim and intersection tests share the same frame as the cubic bezier
/// endpoints.
///
/// **Buffers, not gesture state.** This sits beside [`BreakerUI`](crate::gui::pane::graph::gesture::breaker::BreakerUI)'s slot
/// rather than inside it, so a gesture that ends returns four allocations to
/// the *next* one instead of to the allocator — the same bargain
/// `SelectionUI::swept` and `NodeUI::row_tracks` already make. What makes that
/// safe is the slot: [`BreakerUI::probe`](crate::gui::pane::graph::gesture::breaker::BreakerUI::probe) only hands these out while it is
/// latched, so between gestures the leftovers are unreachable rather than
/// merely stale. [`Self::restart`] is where a fresh gesture claims them.
///
/// The three `broken_*` collections are each filled by one render pass's
/// hit-test (via `BreakerProbe::mark_broken_*`) and drained by
/// `BreakerUI::apply` on release into the matching severing `GraphIntent`. All
/// three are cleared together at the start of every frame's probing
/// (`begin_frame`, called from `BreakerUI::probe`) rather than each
/// renderer clearing its own — every render pass visits its own targets at
/// most once per frame, so within-frame duplicates aren't possible either
/// way.
#[derive(Default, Debug)]
pub(crate) struct Scribble {
    pub(super) points: Vec<Vec2>,
    length: f32,
    /// The corners of the polyline's bounding box, kept as points arrive.
    /// Every segment lies inside it, so a target whose own box misses it
    /// cannot cross, and is rejected before any segment test.
    lo: Vec2,
    hi: Vec2,
    /// Target input ports whose data binding the breaker intersects this
    /// frame, drained on release into an unbound `GraphIntent::SetInput`.
    pub(super) broken: Vec<InputPort>,
    /// Nodes whose body rect the breaker crosses this frame, drained on
    /// release into `GraphIntent::RemoveNode`.
    pub(super) broken_nodes: Vec<NodeId>,
    /// Event subscriptions whose wire the breaker intersects this frame,
    /// drained on release into `SetSubscription { subscribe: false }`.
    pub(super) broken_subscriptions: Vec<Subscription>,
}

impl Scribble {
    /// Begin a fresh scribble at `p`, keeping every buffer's capacity. The
    /// one place a gesture's leftovers are dropped, so nothing downstream has
    /// to ask whether what it is reading belongs to this gesture or the last.
    pub(super) fn restart(&mut self, p: Vec2) {
        self.points.clear();
        self.points.push(p);
        self.length = 0.0;
        self.lo = p;
        self.hi = p;
        self.begin_frame();
    }

    /// Clear every `broken_*` collection at the start of a frame's probing.
    /// The single point where this happens — called once from
    /// [`BreakerUI::probe`](crate::gui::pane::graph::gesture::breaker::BreakerUI::probe) — rather than each renderer clearing its own
    /// field, which is easy to forget (and had been forgotten for two of
    /// the three).
    pub(super) fn begin_frame(&mut self) {
        self.broken.clear();
        self.broken_nodes.clear();
        self.broken_subscriptions.clear();
    }

    pub(super) fn add_point(&mut self, p: Vec2) {
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
        self.lo = self.lo.min(clamped);
        self.hi = self.hi.max(clamped);
    }

    /// Whether the box `min..=max` lies wholly outside the polyline's own, so
    /// nothing inside it can touch the polyline. Closed on both sides: a box
    /// that only touches the polyline's still counts as overlapping.
    fn misses(&self, min: Vec2, max: Vec2) -> bool {
        max.x < self.lo.x || min.x > self.hi.x || max.y < self.lo.y || min.y > self.hi.y
    }

    fn segments(&self) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
        self.points.windows(2).map(|w| (w[0], w[1]))
    }

    /// True if the breaker polyline crosses `rect`: either any sample
    /// falls inside, or any breaker segment crosses one of the four
    /// edges. `rect` is in the same frame as the polyline (inner-
    /// canvas pre-transform world coords).
    pub(super) fn intersects_rect(&self, rect: Rect) -> bool {
        if self.points.is_empty() {
            return false;
        }
        let min = rect.min;
        let max = rect.max();
        if self.misses(min, max) {
            return false;
        }
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
    pub(super) fn intersects_cubic(&self, p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2) -> bool {
        // A cubic stays inside its control points' box, and so does every
        // chord between two of its samples.
        if self.points.len() < 2
            || self.misses(p0.min(p1).min(p2).min(p3), p0.max(p1).max(p2).max(p3))
        {
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
mod tests;
