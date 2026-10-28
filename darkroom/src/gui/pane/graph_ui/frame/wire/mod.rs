//! [`Wire`]: one wire's cubic, as the cull, the probes and the paint all take it.

use glam::Vec2;
use palantir::Rect;

/// Chords a wire is sampled into for a hit test: 16, cheap enough to redo every frame for every
/// visible wire.
pub(crate) const BEZIER_SAMPLES: usize = 16;

/// Minimum length of a wire's bezier control handles, so a short or backward
/// link still bows out into a readable curve.
const MIN_HANDLE: f32 = 30.0;

/// Upper bound on the *vertical-gap* term of [`Wire::data`]'s handle
/// length, so a tall forward span bows into a gentle S rather than a huge loop.
const MAX_HANDLE: f32 = 120.0;

/// Gain on [`Wire::data`]'s *backward-reach* term: `reach = BACKREACH_GAIN * sqrt(distance)`.
/// A square-root law (not linear, not a fixed cap) so the loop keeps growing as the far end
/// moves further left — a flat cap reads short across a big gap — yet grows ever more slowly,
/// so it never sprawls out to the sides the way a linear reach does. Tuned so a node-width
/// backlink (~180px) reaches ~135px.
const BACKREACH_GAIN: f32 = 10.0;

/// One wire's full cubic: the endpoints `p0` → `p3` plus the interior
/// control points its handle rule placed. Built through [`Wire::data`] or
/// [`Wire::event`], so the choice of rule happens once — at the only place
/// that knows which family the curve belongs to — and every consumer
/// downstream (cull, breaker, paint) takes the finished curve.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Wire {
    pub(crate) p0: Vec2,
    pub(crate) p1: Vec2,
    pub(crate) p2: Vec2,
    pub(crate) p3: Vec2,
}

impl Wire {
    /// A left-to-right cubic between `p0` (the output port) and `p3` (the
    /// input port): both handles run horizontally so the curve leaves `p0`
    /// rightward and arrives at `p3` leftward. Shared by the permanent and
    /// in-flight draws so a preview always matches its eventual committed
    /// curve exactly.
    ///
    /// The handle length is the larger of two terms:
    /// - **Forward** — half the *vertical* gap (clamped to `[MIN_HANDLE,
    ///   MAX_HANDLE]`): near-level anchors stay taut, stacked ones bow into a
    ///   gentle S without over-looping on a tall span.
    /// - **Backward** — when `p3` sits *left* of `p0` the curve must double back
    ///   on itself. A short handle whips it straight across whatever sits between
    ///   (the "hidden under the node" look); reaching out by `BACKREACH_GAIN *
    ///   sqrt(distance)` instead bows both ends into one wide, smooth loop that
    ///   leaves `p0` rightward, arcs around, and re-enters `p3` leftward. The
    ///   `sqrt` keeps the loop scaling with the backward distance (so a far-away
    ///   `p3` still gets a proper loop, not a stub) while growing slowly enough
    ///   that it never sprawls out to the sides.
    pub(crate) fn data(p0: Vec2, p3: Vec2) -> Self {
        let vertical = ((p3.y - p0.y).abs() * 0.5).clamp(MIN_HANDLE, MAX_HANDLE);
        let backreach = BACKREACH_GAIN * (p0.x - p3.x).max(0.0).sqrt();
        let len = vertical.max(backreach);
        Self {
            p0,
            p1: p0 + Vec2::new(len, 0.0),
            p2: p3 - Vec2::new(len, 0.0),
            p3,
        }
    }

    /// An event wire from emitter `p0` (a triangle on the right of its node)
    /// to subscriber pin `p3` (the top-left pin). The emitter handle leaves
    /// rightward like a data output; the subscriber handle points
    /// **up-left**, matching the pin's outward-pointing triangle so the wire
    /// meets it head-on.
    pub(crate) fn event(p0: Vec2, p3: Vec2) -> Self {
        let d = (p0.distance(p3) * 0.4).max(MIN_HANDLE);
        // (-1, -1) is up-left in screen space (y grows downward).
        let up_left = Vec2::new(-1.0, -1.0).normalize();
        Self {
            p0,
            p1: p0 + Vec2::new(d, 0.0),
            p2: p3 + up_left * d,
            p3,
        }
    }

    /// The curve's control-point bounding box. A cubic stays inside its control hull, so this is a
    /// conservative bound: what the cull keeps and the probes reject a wire by.
    pub(crate) fn hull(&self) -> Rect {
        let min = self.p0.min(self.p1).min(self.p2).min(self.p3);
        let max = self.p0.max(self.p1).max(self.p2).max(self.p3);
        Rect::from_min_max(min, max)
    }

    /// The straight distance between the curve's two ends.
    pub(crate) fn chord(&self) -> f32 {
        self.p0.distance(self.p3)
    }

    /// The curve as [`BEZIER_SAMPLES`] chords, from `p0` to `p3`, which a probe tests a line
    /// against. Inside the hull, as every point of the curve is.
    pub(crate) fn chords(&self) -> impl ExactSizeIterator<Item = [Vec2; 2]> + '_ {
        (0..BEZIER_SAMPLES).map(|idx| {
            let at = |idx: usize| self.at(idx as f32 / BEZIER_SAMPLES as f32);
            [at(idx), at(idx + 1)]
        })
    }

    /// The curve's derivative at parameter `t`: the way it runs there, scaled by its speed.
    pub(crate) fn velocity(&self, t: f32) -> Vec2 {
        let u = 1.0 - t;
        (self.p1 - self.p0) * (3.0 * u * u)
            + (self.p2 - self.p1) * (6.0 * u * t)
            + (self.p3 - self.p2) * (3.0 * t * t)
    }

    /// The point at parameter `t` along the curve, in Bernstein form.
    pub(crate) fn at(&self, t: f32) -> Vec2 {
        let u = 1.0 - t;
        let uu = u * u;
        let tt = t * t;
        self.p0 * (uu * u)
            + self.p1 * (3.0 * uu * t)
            + self.p2 * (3.0 * u * tt)
            + self.p3 * (tt * t)
    }
}

#[cfg(test)]
mod tests;
