//! Shared connection-curve primitives. Both the data-wire renderer
//! ([`super::connection_ui`]) and the event/subscription-wire renderer
//! ([`super::subscription_ui`]) compute their own control handles but
//! draw the curve through the one primitive here — and resolve their
//! emphasis tiers (rest-dim / hover-lift / gesture-fade) through the one
//! [`WireEmphasis`] — so the two stay visually identical apart from their
//! brush and can't drift.

use aperture::{Brush, Color, LineCap, Shape, Ui};
use glam::Vec2;

use crate::gui::canvas::breaker::cubic_point;
use crate::gui::canvas::{outer_canvas_widget_id, pointer_world};
use crate::gui::scene::Scene;

/// Minimum length of a wire's bezier control handles, so a short or backward
/// link still bows out into a readable curve.
pub(crate) const MIN_HANDLE: f32 = 30.0;

/// How far rest-state wire endpoint colors pull toward the canvas, so the
/// port dots (identity) stay the brightest points on the data path and long
/// wires don't outshine them.
const WIRE_REST_DIM: f32 = 0.15;

/// Alpha of the standing wires while a wire gesture (new-connection drag,
/// subscription drag, breaker scribble) is active — dimming the plumbing so
/// the preview, candidate ports, and broken-alarm wires pop.
const WIRE_DRAG_FADE: f32 = 0.35;

/// Screen-px radius around the pointer that counts as "on the wire" for the
/// hover highlight (divided by zoom into world units).
const WIRE_HOVER_RADIUS: f32 = 6.0;

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
#[derive(Debug)]
pub(crate) struct WireEmphasis {
    fading: bool,
    /// Pointer in canvas world coords — only set when the bare canvas is
    /// the actual hover target (chrome, nodes, and panels stacked above it
    /// suppress wire hover) and nothing is fading.
    pointer: Option<Vec2>,
    /// [`WIRE_HOVER_RADIUS`] converted to world units.
    hover_radius: f32,
    canvas_bg: Color,
}

impl WireEmphasis {
    /// Resolve this frame's emphasis inputs. `fading` is "any wire gesture
    /// is active" — the callers OR together the two drag controllers and
    /// the breaker. The pointer is taken only when the outer canvas itself
    /// is hovered, so wires under nodes, panels, or floating chrome don't
    /// react to a pointer that can't reach them.
    pub(crate) fn resolve(
        ui: &Ui,
        scene: &Scene,
        canvas_origin: Vec2,
        canvas_bg: Color,
        fading: bool,
    ) -> Self {
        let pointer = (!fading && ui.response_for(outer_canvas_widget_id()).hovered)
            .then(|| pointer_world(ui, scene, canvas_origin))
            .flatten();
        // Same degenerate-scale fallback as `to_world`, so the radius and
        // the pointer it gates agree on the mapping.
        let zoom = if scene.viewport.zoom > 0.0 {
            scene.viewport.zoom
        } else {
            1.0
        };
        Self {
            fading,
            pointer,
            hover_radius: WIRE_HOVER_RADIUS / zoom,
            canvas_bg,
        }
    }

    /// Whether this wire is hover-emphasized: an endpoint glyph is hovered
    /// or the curve passes within the hover radius of the pointer. Never
    /// while a gesture fades the set — the snap target's forced endpoint
    /// hover must not re-emphasize a faded wire.
    pub(crate) fn hovered(
        &self,
        endpoint_hovered: bool,
        p0: Vec2,
        handles: &CubicHandles,
        p3: Vec2,
    ) -> bool {
        if self.fading {
            return false;
        }
        endpoint_hovered
            || self
                .pointer
                .is_some_and(|p| near_cubic(p0, handles, p3, p, self.hover_radius))
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

/// Whether `point` lies within `radius` of the cubic, tested against the
/// 16 chord *segments* of the sampled curve — point-distance to the samples
/// alone leaves dead zones between them on any wire longer than a couple
/// hundred px (sample spacing exceeds the radius).
fn near_cubic(p0: Vec2, handles: &CubicHandles, p3: Vec2, point: Vec2, radius: f32) -> bool {
    const SAMPLES: u32 = 16;
    let r2 = radius * radius;
    let mut prev = p0;
    for i in 1..=SAMPLES {
        let t = i as f32 / SAMPLES as f32;
        let next = cubic_point(p0, handles.p1, handles.p2, p3, t);
        if dist_sq_to_segment(point, prev, next) <= r2 {
            return true;
        }
        prev = next;
    }
    false
}

/// Squared distance from `p` to segment `a..b`.
fn dist_sq_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    };
    p.distance_squared(a + ab * t)
}

/// The two interior control points of a connection cubic — the named result
/// of each renderer's handle-placement function.
#[derive(Debug)]
pub(crate) struct CubicHandles {
    pub(crate) p1: Vec2,
    pub(crate) p2: Vec2,
}

/// Emit a stroked cubic-bezier wire (round caps) from `p0` to `p3` through
/// `handles`. The single place the wire `Shape` is built, so data and event
/// wires can't drift in width policy, cap, or primitive.
pub(crate) fn add_cubic_wire(
    ui: &mut Ui,
    p0: Vec2,
    p3: Vec2,
    handles: CubicHandles,
    width: f32,
    brush: Brush,
) {
    ui.add_shape(Shape::CubicBezier {
        p0,
        p1: handles.p1,
        p2: handles.p2,
        p3,
        width,
        brush,
        cap: LineCap::Round,
    });
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

    /// A degenerate "straight" cubic along the x axis: handles collinear
    /// with the endpoints, so the curve is exactly the segment (0,0)→(400,0)
    /// and expected distances can be computed by hand.
    fn straight() -> (Vec2, CubicHandles, Vec2) {
        (
            Vec2::ZERO,
            CubicHandles {
                p1: Vec2::new(100.0, 0.0),
                p2: Vec2::new(300.0, 0.0),
            },
            Vec2::new(400.0, 0.0),
        )
    }

    #[test]
    fn near_cubic_covers_mid_segment_not_just_samples() {
        let (p0, h, p3) = straight();
        // 400px curve → samples 25px apart; x = 12.5 sits exactly between
        // two samples, 12.5px from the nearest — a point-distance test with
        // a 6px radius misses it, the segment test must hit.
        assert!(near_cubic(p0, &h, p3, Vec2::new(12.5, 5.0), 6.0));
        // On the curve anywhere along its length.
        assert!(near_cubic(p0, &h, p3, Vec2::new(203.0, 0.0), 6.0));
        // Just past the radius → miss.
        assert!(!near_cubic(p0, &h, p3, Vec2::new(200.0, 6.5), 6.0));
        // Past the endpoint by more than the radius → miss; within → hit.
        assert!(!near_cubic(p0, &h, p3, Vec2::new(410.0, 0.0), 6.0));
        assert!(near_cubic(p0, &h, p3, Vec2::new(-4.0, 0.0), 6.0));
    }

    #[test]
    fn emphasis_tiers_fade_dim_and_lift() {
        let canvas = Color::linear_rgba(0.0, 0.0, 0.0, 1.0);
        let c = Color::linear_rgba(1.0, 0.5, 0.0, 1.0);
        let rest = WireEmphasis {
            fading: false,
            pointer: None,
            hover_radius: 6.0,
            canvas_bg: canvas,
        };
        // Rest pulls 15% toward the (black) canvas: r 1.0→0.85, g 0.5→0.425.
        let dimmed = rest.tint(c, false);
        assert_close(dimmed.r, 0.85);
        assert_close(dimmed.g, 0.425);
        assert_close(dimmed.b, 0.0);
        // Emphasis keeps the full color and lifts the width by 1.25×.
        assert_eq!(rest.tint(c, true), c);
        assert_eq!(rest.width(2.0, true), 2.5);
        assert_eq!(rest.width(2.0, false), 2.0);

        let (p0, h, p3) = straight();
        // A fading pass drops alpha to the fade constant and suppresses
        // hover even with a forced endpoint hover and an on-curve pointer —
        // the snap target's forced hover must not re-emphasize a faded wire.
        let fading = WireEmphasis {
            fading: true,
            pointer: Some(Vec2::new(200.0, 0.0)),
            hover_radius: 6.0,
            canvas_bg: canvas,
        };
        assert_eq!(fading.tint(c, false).a, WIRE_DRAG_FADE);
        assert!(!fading.hovered(true, p0, &h, p3));
        // Live pass with the pointer on the curve hovers without endpoint help.
        let live = WireEmphasis {
            fading: false,
            pointer: Some(Vec2::new(200.0, 0.0)),
            hover_radius: 6.0,
            canvas_bg: canvas,
        };
        assert!(live.hovered(false, p0, &h, p3));
    }
}
