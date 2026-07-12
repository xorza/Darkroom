//! Shared connection-curve primitives. The data-wire renderer
//! ([`super::connection_ui`]) and the pinned-output bezier
//! ([`super::pin_ui`]) both grow rightward from an output-ish
//! anchor, so they share [`cubic_handles`] itself; the event/subscription-wire
//! renderer ([`super::subscription_ui`]) has its own up-left arrival shape
//! ([`super::subscription_ui::event_handles`]) but all three draw through the
//! one paint primitive here ([`add_cubic_wire`]) — and resolve their emphasis
//! tiers (rest-dim / hover-lift / gesture-fade) through the one
//! [`WireEmphasis`] — so they stay visually identical apart from brush and
//! handle shape, and can't drift.

use aperture::{Brush, Color, LineCap, Shape, Ui};
use glam::Vec2;

/// Minimum length of a wire's bezier control handles, so a short or backward
/// link still bows out into a readable curve.
pub(crate) const MIN_HANDLE: f32 = 30.0;

/// Upper bound on the *vertical-gap* term of [`cubic_handles`]'s handle
/// length, so a tall forward span bows into a gentle S rather than a huge loop.
const MAX_HANDLE: f32 = 120.0;

/// Gain on [`cubic_handles`]'s *backward-reach* term: `reach = BACKREACH_GAIN * sqrt(distance)`.
/// A square-root law (not linear, not a fixed cap) so the loop keeps growing as the far end
/// moves further left — a flat cap reads short across a big gap — yet grows ever more slowly,
/// so it never sprawls out to the sides the way a linear reach does. Tuned so a node-width
/// backlink (~180px) reaches ~135px.
const BACKREACH_GAIN: f32 = 10.0;

/// The two interior control points of a connection cubic — the named result
/// of each renderer's handle-placement function.
#[derive(Debug)]
pub(crate) struct CubicHandles {
    pub(crate) p1: Vec2,
    pub(crate) p2: Vec2,
}

/// Control points for a left-to-right cubic between `p0` (an output-ish
/// anchor: a data output port, or a pin's own port) and `p3` (the far end: an
/// input port, or a pin's satellite): both handles run horizontally so the
/// curve leaves `p0` rightward and arrives at `p3` leftward. Shared by every
/// caller's permanent and in-flight draws so a preview always matches its
/// eventual committed curve exactly.
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
pub(crate) fn cubic_handles(p0: Vec2, p3: Vec2) -> CubicHandles {
    let vertical = ((p3.y - p0.y).abs() * 0.5).clamp(MIN_HANDLE, MAX_HANDLE);
    let backreach = BACKREACH_GAIN * (p0.x - p3.x).max(0.0).sqrt();
    let len = vertical.max(backreach);
    CubicHandles {
        p1: p0 + Vec2::new(len, 0.0),
        p2: p3 - Vec2::new(len, 0.0),
    }
}

/// Emit a stroked cubic-bezier wire (round caps) from `p0` to `p3` through
/// `handles`. The single place the wire `Shape` is built, so data, event, and
/// pin curves can't drift in width policy, cap, or primitive.
pub(crate) fn add_cubic_wire(
    ui: &mut Ui,
    p0: Vec2,
    p3: Vec2,
    handles: CubicHandles,
    width: f32,
    brush: Brush,
) {
    ui.add_shape(
        Shape::cubic_bezier(p0, handles.p1, handles.p2, p3, width)
            .brush(brush)
            .cap(LineCap::Round),
    );
}

/// How far rest-state wire endpoint colors pull toward the canvas, so the
/// port dots (identity) stay the brightest points on the data path and long
/// wires don't outshine them.
const WIRE_REST_DIM: f32 = 0.15;

/// Alpha of the standing wires while a wire gesture (new-connection drag,
/// subscription drag, breaker scribble) is active — dimming the plumbing so
/// the preview, candidate ports, and broken-alarm wires pop.
const WIRE_DRAG_FADE: f32 = 0.35;

/// Width multiplier for an emphasized (hovered or broken-alarm) wire, so
/// one connection stays traceable through a crossing.
const WIRE_HOVER_WIDTH: f32 = 1.25;

/// Linear-space pull of `c` toward `to` by `t`, alpha untouched. Storage
/// colors are already linear, so a straight component lerp is correct.
pub(crate) fn toward(c: Color, to: Color, t: f32) -> Color {
    Color::linear_rgba(
        c.r + (to.r - c.r) * t,
        c.g + (to.g - c.g) * t,
        c.b + (to.b - c.b) * t,
        c.a,
    )
}

/// Per-pass emphasis state shared by both wire renderers, resolved once in
/// the canvas frame. The tiers: while any wire gesture is in flight
/// (`fading`) the standing set drops to [`WIRE_DRAG_FADE`] alpha and hover
/// is off; at rest, endpoint colors pull toward the canvas; a hovered (or
/// broken-alarm) wire gets full strength and a width lift.
///
/// Emphasis is driven by endpoint *hover targets* only (port circles,
/// event glyphs, subscription pins — all with generously scaled hit
/// boxes), never by raw pointer proximity to the curve: hover-target
/// state repaints exactly when it changes, whereas pointer-derived
/// paint needs a `MOVE` subscription (a record per mouse move) to stay
/// fresh on screen.
#[derive(Debug)]
pub(crate) struct WireEmphasis {
    fading: bool,
    canvas_bg: Color,
}

impl WireEmphasis {
    /// Resolve this frame's emphasis inputs. `fading` is "any wire gesture
    /// is active" — the callers OR together the two drag controllers and
    /// the breaker.
    pub(crate) fn resolve(canvas_bg: Color, fading: bool) -> Self {
        Self { fading, canvas_bg }
    }

    /// Whether this wire is hover-emphasized: an endpoint glyph is
    /// hovered. Never while a gesture fades the set — the snap target's
    /// forced endpoint hover must not re-emphasize a faded wire.
    pub(crate) fn hovered(&self, endpoint_hovered: bool) -> bool {
        !self.fading && endpoint_hovered
    }

    /// The tiered color for a (non-broken) wire endpoint.
    pub(crate) fn tint(&self, c: Color, emphasized: bool) -> Color {
        if self.fading {
            c.with_alpha(WIRE_DRAG_FADE)
        } else if emphasized {
            c
        } else {
            toward(c, self.canvas_bg, WIRE_REST_DIM)
        }
    }

    /// The tiered stroke width. Broken-alarm wires pass `emphasized: true`
    /// too: full width against the faded rest of the set is the alarm.
    pub(crate) fn width(&self, base: f32, emphasized: bool) -> f32 {
        if emphasized {
            base * WIRE_HOVER_WIDTH
        } else {
            base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected ~{expected}, got {actual}"
        );
    }

    #[test]
    fn toward_lerps_linearly_preserving_alpha() {
        let a = Color::linear_rgba(1.0, 0.0, 0.5, 0.8);
        let b = Color::linear_rgba(0.0, 1.0, 0.5, 0.1);
        assert_eq!(toward(a, b, 0.0), a);
        // t = 1 lands on `b`'s rgb but keeps `a`'s alpha.
        let full = toward(a, b, 1.0);
        assert_eq!((full.r, full.g, full.b, full.a), (0.0, 1.0, 0.5, 0.8));
        // Hand-computed midpoint: rgb (0.5, 0.5, 0.5), alpha still 0.8.
        let mid = toward(a, b, 0.5);
        assert_eq!((mid.r, mid.g, mid.b, mid.a), (0.5, 0.5, 0.5, 0.8));
    }

    #[test]
    fn emphasis_tiers_fade_dim_and_lift() {
        let canvas = Color::linear_rgba(0.0, 0.0, 0.0, 1.0);
        let c = Color::linear_rgba(1.0, 0.5, 0.0, 1.0);
        let rest = WireEmphasis::resolve(canvas, false);
        // Rest pulls 15% toward the (black) canvas: r 1.0→0.85, g 0.5→0.425.
        let dimmed = rest.tint(c, false);
        assert_close(dimmed.r, 0.85);
        assert_close(dimmed.g, 0.425);
        assert_close(dimmed.b, 0.0);
        // Emphasis keeps the full color and lifts the width by 1.25×.
        assert_eq!(rest.tint(c, true), c);
        assert_eq!(rest.width(2.0, true), 2.5);
        assert_eq!(rest.width(2.0, false), 2.0);
        // Endpoint hover carries the emphasis at rest…
        assert!(rest.hovered(true));
        assert!(!rest.hovered(false));

        // …but a fading pass drops alpha to the fade constant and
        // suppresses hover even with a forced endpoint hover — the snap
        // target's forced hover must not re-emphasize a faded wire.
        let fading = WireEmphasis::resolve(canvas, true);
        assert_eq!(fading.tint(c, false).a, WIRE_DRAG_FADE);
        assert!(!fading.hovered(true));
    }

    #[test]
    fn cubic_handles_forward_span_uses_half_vertical_gap_clamped() {
        // Level ports (no vertical gap): clamps up to MIN_HANDLE.
        let h = cubic_handles(Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0));
        assert_eq!(h.p1, Vec2::new(MIN_HANDLE, 0.0));
        assert_eq!(h.p2, Vec2::new(200.0 - MIN_HANDLE, 0.0));

        // A tall span's half-gap (300) exceeds MAX_HANDLE, so it clamps down.
        // Only the x component moves — both handles stay level with their
        // own endpoint's y.
        let h = cubic_handles(Vec2::new(0.0, 0.0), Vec2::new(200.0, 600.0));
        assert_eq!(h.p1, Vec2::new(MAX_HANDLE, 0.0));
        assert_eq!(h.p2, Vec2::new(200.0 - MAX_HANDLE, 600.0));
    }

    #[test]
    fn cubic_handles_backward_span_loops_via_sqrt_reach() {
        // p3 sits left of p0 by 400 — forward term is 0 (level), backward
        // term is 10 * sqrt(400) = 200, which wins.
        let h = cubic_handles(Vec2::new(400.0, 0.0), Vec2::new(0.0, 0.0));
        assert_close(h.p1.x, 400.0 + 200.0);
        assert_close(h.p2.x, 0.0 - 200.0);
    }
}
