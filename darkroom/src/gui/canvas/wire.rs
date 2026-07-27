//! Shared connection-curve primitives. Every curve on the canvas — a data
//! connection ([`crate::gui::canvas::connection_ui`]), an event wire
//! ([`crate::gui::canvas::subscription_ui`]), a pinned output's satellite
//! bezier ([`crate::gui::canvas::pin_ui`]) — is one [`Wire`]: two endpoints
//! plus the interior control points its family's handle rule placed. That
//! single value is what the cull test, the breaker probe, and the paint call
//! all take, and [`WirePass`] carries the per-frame inputs the three
//! renderers share, so they stay visually identical apart from brush and
//! handle shape and can't drift.

use glam::Vec2;
use palantir::{Color, CurveBrush, LineCap, Rect, Shape, Size, Ui};

use crate::gui::canvas::breaker::BreakerProbe;
use crate::gui::canvas::cull::CullRegion;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::scene::GraphScene;
use crate::gui::theme::Theme;

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
pub(super) struct Wire {
    pub(super) p0: Vec2,
    pub(super) p1: Vec2,
    pub(super) p2: Vec2,
    pub(super) p3: Vec2,
}

impl Wire {
    /// A left-to-right cubic between `p0` (an output-ish anchor: a data
    /// output port, or a pin's own port) and `p3` (the far end: an input
    /// port, or a pin's satellite): both handles run horizontally so the
    /// curve leaves `p0` rightward and arrives at `p3` leftward. Shared by
    /// every caller's permanent and in-flight draws so a preview always
    /// matches its eventual committed curve exactly.
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
    pub(super) fn data(p0: Vec2, p3: Vec2) -> Self {
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
    pub(super) fn event(p0: Vec2, p3: Vec2) -> Self {
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

    /// The curve's control-point bounding box. A cubic stays inside its
    /// control hull, so this is a conservative bound — what
    /// [`CullRegion::keeps_wire`] tests against.
    pub(super) fn hull(&self) -> Rect {
        let min = self.p0.min(self.p1).min(self.p2).min(self.p3);
        let max = self.p0.max(self.p1).max(self.p2).max(self.p3);
        Rect {
            min,
            size: Size::new(max.x - min.x, max.y - min.y),
        }
    }

    /// Emit the stroked curve (round caps). The single place the wire
    /// `Shape` is built, so data, event, and pin curves can't drift in
    /// width policy, cap, or primitive.
    pub(super) fn add(&self, ui: &mut Ui, width: f32, brush: CurveBrush) {
        ui.add_shape(
            Shape::cubic_bezier(self.p0, self.p1, self.p2, self.p3, width)
                .brush(brush)
                .cap(LineCap::Round),
        );
    }
}

/// The per-frame inputs all three wire renderers need, bundled so each
/// `draw` takes one argument instead of six ([`crate::gui::node::RecordCtx`]
/// is the same pattern). Built once in [`crate::gui::canvas::GraphUI::draw`]
/// and passed by `&mut`, so the breaker probe reborrows into each renderer
/// in turn.
#[derive(Debug)]
pub(super) struct WirePass<'a, 'p> {
    pub(super) theme: &'a Theme,
    pub(super) graph: GraphScene<'a>,
    pub(super) geometry: &'a CanvasGeometry,
    pub(super) cull: CullRegion,
    pub(super) probe: &'a mut BreakerProbe<'p>,
    pub(super) emphasis: &'a WireEmphasis,
}

impl WirePass<'_, '_> {
    /// Cull, breaker-probe, and resolve the paint tier for one committed
    /// wire — the prologue the data and event renderers both run. `None`
    /// drops the wire entirely, probe included: the scribble is always
    /// on-screen, so it can't have crossed an off-screen curve.
    ///
    /// Recording the hit stays with the caller — once [`WireStroke::broken`]
    /// comes back set it calls the matching `probe.mark_broken_*`, since only
    /// it knows the domain key. ([`crate::gui::canvas::pin_ui`] resolves its
    /// own: a pin is kept on-screen, and cut, by its card as much as by its
    /// curve.)
    pub(super) fn resolve(&mut self, wire: &Wire, endpoint_hover: bool) -> Option<WireStroke> {
        if !self.cull.keeps_wire(wire) {
            return None;
        }
        let broken = self.probe.crosses_wire(wire);
        Some(
            self.emphasis
                .stroke(self.theme.connection_width, broken, endpoint_hover),
        )
    }
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

/// Per-pass emphasis state shared by every wire renderer, resolved once in
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
pub(super) struct WireEmphasis {
    fading: bool,
    canvas_bg: Color,
}

impl WireEmphasis {
    /// Resolve this frame's emphasis inputs. `fading` is "any wire gesture
    /// is active" — the callers OR together the two drag controllers and
    /// the breaker.
    pub(super) fn resolve(canvas_bg: Color, fading: bool) -> Self {
        Self { fading, canvas_bg }
    }

    /// This frame's paint tier for one wire, folding the two rules each
    /// renderer used to spell out by hand: a broken wire never *also* reads
    /// as hovered (the alarm hue wins outright), and either state takes the
    /// width lift.
    pub(super) fn stroke(&self, base_width: f32, broken: bool, endpoint_hover: bool) -> WireStroke {
        let hovered = !broken && self.hovered(endpoint_hover);
        WireStroke {
            broken,
            hovered,
            width: self.width(base_width, hovered || broken),
        }
    }

    /// Whether this wire is hover-emphasized: an endpoint glyph is
    /// hovered. Never while a gesture fades the set — the snap target's
    /// forced endpoint hover must not re-emphasize a faded wire.
    fn hovered(&self, endpoint_hovered: bool) -> bool {
        !self.fading && endpoint_hovered
    }

    /// The tiered color for a (non-broken) wire endpoint.
    pub(super) fn tint(&self, c: Color, emphasized: bool) -> Color {
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
    fn width(&self, base: f32, emphasized: bool) -> f32 {
        if emphasized {
            base * WIRE_HOVER_WIDTH
        } else {
            base
        }
    }
}

/// How one wire paints this frame, as resolved by [`WireEmphasis::stroke`].
#[derive(Clone, Copy, Debug)]
pub(super) struct WireStroke {
    /// The active breaker crosses this wire: the renderer paints it in the
    /// alarm color and records the hit against its own domain key.
    pub(super) broken: bool,
    /// Paints at full strength rather than the rest-dim tint. Never set
    /// together with `broken` — the alarm read wins outright.
    pub(super) hovered: bool,
    pub(super) width: f32,
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
    fn stroke_lets_the_broken_alarm_win_over_hover() {
        let rest = WireEmphasis::resolve(Color::linear_rgba(0.0, 0.0, 0.0, 1.0), false);

        // Plain wire: no lift at all.
        let idle = rest.stroke(2.0, false, false);
        assert!(!idle.broken && !idle.hovered);
        assert_eq!(idle.width, 2.0);

        // Endpoint hover alone lifts both the tier and the width.
        let hover = rest.stroke(2.0, false, true);
        assert!(hover.hovered);
        assert_eq!(hover.width, 2.5);

        // Broken *and* hovered: `hovered` is suppressed so the brush takes
        // the flat alarm color, but the width lift is kept.
        let broken = rest.stroke(2.0, true, true);
        assert!(broken.broken && !broken.hovered);
        assert_eq!(broken.width, 2.5);

        // A fading pass still gives a broken wire full width — that's the
        // alarm read against the dimmed rest of the set.
        let fading = WireEmphasis::resolve(Color::linear_rgba(0.0, 0.0, 0.0, 1.0), true);
        let faded_hover = fading.stroke(2.0, false, true);
        assert!(!faded_hover.hovered);
        assert_eq!(faded_hover.width, 2.0);
        assert_eq!(fading.stroke(2.0, true, false).width, 2.5);
    }

    #[test]
    fn data_wire_forward_span_uses_half_vertical_gap_clamped() {
        // Level ports (no vertical gap): clamps up to MIN_HANDLE.
        let w = Wire::data(Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0));
        assert_eq!(w.p1, Vec2::new(MIN_HANDLE, 0.0));
        assert_eq!(w.p2, Vec2::new(200.0 - MIN_HANDLE, 0.0));

        // A tall span's half-gap (300) exceeds MAX_HANDLE, so it clamps down.
        // Only the x component moves — both handles stay level with their
        // own endpoint's y.
        let w = Wire::data(Vec2::new(0.0, 0.0), Vec2::new(200.0, 600.0));
        assert_eq!(w.p1, Vec2::new(MAX_HANDLE, 0.0));
        assert_eq!(w.p2, Vec2::new(200.0 - MAX_HANDLE, 600.0));
    }

    #[test]
    fn data_wire_backward_span_loops_via_sqrt_reach() {
        // p3 sits left of p0 by 400 — forward term is 0 (level), backward
        // term is 10 * sqrt(400) = 200, which wins.
        let w = Wire::data(Vec2::new(400.0, 0.0), Vec2::new(0.0, 0.0));
        assert_close(w.p1.x, 400.0 + 200.0);
        assert_close(w.p2.x, 0.0 - 200.0);
    }

    #[test]
    fn event_wire_arrives_up_left_at_the_subscriber_pin() {
        // Level span of 100: d = max(100 * 0.4, MIN_HANDLE) = 40. The
        // emitter handle leaves straight right; the subscriber handle backs
        // off along the normalized (-1, -1) diagonal, i.e. 40/√2 ≈ 28.284
        // in each axis, so the curve meets the outward-pointing pin
        // head-on rather than flat.
        let w = Wire::event(Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0));
        assert_eq!(w.p1, Vec2::new(40.0, 0.0));
        let diag = 40.0 / 2.0_f32.sqrt();
        assert_close(w.p2.x, 100.0 - diag);
        assert_close(w.p2.y, -diag);

        // A short span floors at MIN_HANDLE (30) instead of collapsing:
        // 10 * 0.4 = 4 < 30.
        let short = Wire::event(Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        assert_eq!(short.p1, Vec2::new(MIN_HANDLE, 0.0));

        // The two families place different handles for the same endpoints —
        // a data wire arrives level from the left, an event wire diagonally.
        assert_ne!(Wire::data(Vec2::ZERO, Vec2::new(100.0, 0.0)).p2, w.p2);
    }

    #[test]
    fn hull_bounds_every_control_point() {
        // Hand-computed: the backward-loop wire above spans x from -200
        // (p2) to 600 (p1) and y stays 0, so the hull is 800 wide and flat.
        let w = Wire::data(Vec2::new(400.0, 0.0), Vec2::new(0.0, 0.0));
        let hull = w.hull();
        assert_close(hull.min.x, -200.0);
        assert_close(hull.size.w, 800.0);
        assert_close(hull.size.h, 0.0);

        // A stacked span's hull covers both endpoints' y.
        let w = Wire::data(Vec2::new(0.0, 0.0), Vec2::new(200.0, 600.0));
        let hull = w.hull();
        assert_eq!(hull.min, Vec2::ZERO);
        assert_eq!(hull.size, Size::new(200.0, 600.0));
    }
}
