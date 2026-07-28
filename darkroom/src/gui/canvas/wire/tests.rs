use palantir::ColorU8;

use super::*;

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "expected ~{expected}, got {actual}"
    );
}

/// The single color a solid brush carries, or `None` for a gradient.
fn solid(brush: &CurveBrush) -> Option<Color> {
    match brush {
        CurveBrush::Solid(c) => Some(*c),
        _ => None,
    }
}

/// A linear brush's `(t = 0, t = 1)` stop colors, or `None` for a solid.
/// Stops are stored quantized, so the comparisons below go through
/// [`ColorU8`] rather than the float color.
fn gradient(brush: &CurveBrush) -> Option<(ColorU8, ColorU8)> {
    match brush {
        CurveBrush::Linear(g) => Some((g.stops[0].color, g.stops[g.stops.len() - 1].color)),
        _ => None,
    }
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
    assert!(!idle.hovered);
    assert_eq!(idle.width, 2.0);

    // Endpoint hover alone lifts both the tier and the width.
    let hover = rest.stroke(2.0, false, true);
    assert!(hover.hovered);
    assert_eq!(hover.width, 2.5);

    // Broken *and* hovered: `hovered` is suppressed so the caller takes the
    // flat alarm color, but the width lift is kept.
    let broken = rest.stroke(2.0, true, true);
    assert!(!broken.hovered);
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
fn brush_gradients_differing_ends_and_flattens_equal_ones() {
    let canvas = Color::linear_rgba(0.0, 0.0, 0.0, 1.0);
    let rest = WireEmphasis::resolve(canvas, false);
    let a = Color::linear_rgba(1.0, 0.0, 0.0, 1.0);
    let b = Color::linear_rgba(0.0, 1.0, 0.0, 1.0);

    // Distinct ends run p0 → p3 as a gradient, each stop taken through this
    // frame's tier — the rest-dim pull, not the raw color.
    let (start, end) = gradient(&rest.brush(WireTint::new(a, b), false))
        .expect("two different endpoint colors lower to a gradient");
    assert_eq!(start, rest.tint(a, false).into());
    assert_eq!(end, rest.tint(b, false).into());
    assert_ne!(start, end, "and the two ends stay distinguishable");
    // Emphasized keeps both stops at full strength.
    let (start, end) =
        gradient(&rest.brush(WireTint::new(a, b), true)).expect("still a gradient when emphasized");
    assert_eq!((start, end), (a.into(), b.into()));

    // Equal ends — an event wire, or a type-mismatched data wire — collapse to
    // one flat brush rather than a gradient between two identical stops.
    assert_eq!(
        solid(&rest.brush(WireTint::flat(a), true)),
        Some(a),
        "a flat tint paints solid"
    );
    // `WireTint::new` with the same color both sides is the same thing: the
    // collapse is decided on the resolved colors, not on which constructor ran.
    assert_eq!(
        solid(&rest.brush(WireTint::new(a, a), true)),
        Some(a),
        "equal ends collapse however they were built"
    );
    // Two colors that differ *only* before the tier still differ after it, so
    // the collapse can't swallow a real gradient.
    assert!(
        gradient(&rest.brush(WireTint::new(a, b), false)).is_some(),
        "the tier is applied per end, not to a pre-collapsed pair"
    );

    // A fading pass drops both stops to the fade alpha and keeps the gradient;
    // a flat tint keeps painting solid through it.
    let fading = WireEmphasis::resolve(canvas, true);
    let (start, end) = gradient(&fading.brush(WireTint::new(a, b), false))
        .expect("a faded wire keeps its gradient");
    assert_eq!(start, a.with_alpha(WIRE_DRAG_FADE).into());
    assert_eq!(end, b.with_alpha(WIRE_DRAG_FADE).into());
    assert_eq!(
        solid(&fading.brush(WireTint::flat(a), false)).map(|c| c.a),
        Some(WIRE_DRAG_FADE),
        "and a flat one keeps painting solid"
    );
}

#[test]
fn a_glyph_key_names_the_node_its_glyph_hangs_off() {
    use crate::core::document::{PortKind, PortRef};
    use crate::gui::EventRef;

    let node = NodeId::unique();
    // A port and an emitter event belong to their node…
    assert_eq!(
        PortRef {
            node_id: node,
            kind: PortKind::Input,
            port_idx: 3,
        }
        .node(),
        node
    );
    assert_eq!(
        EventRef {
            node_id: node,
            event_idx: 1,
        }
        .node(),
        node
    );
    // …and a subscription pin *is* its node, since a subscription is
    // whole-node. That identity is what lets one `GlyphDrag` span all three.
    assert_eq!(node.node(), node);

    // So a drag off any of them resolves the same owning node, whichever end
    // of an event wire the press started on.
    let emitter = EventRef {
        node_id: node,
        event_idx: 0,
    };
    assert_eq!(GlyphDrag::<EventRef, NodeId>::new(emitter).node(), node);
    assert_eq!(GlyphDrag::<NodeId, EventRef>::new(node).node(), node);
    // A fresh drag has no target yet — the snap scan fills it in each frame.
    assert_eq!(GlyphDrag::<EventRef, NodeId>::new(emitter).snap, None);
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
