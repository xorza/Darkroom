use palantir::SrgbaU8;

use super::*;

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "expected ~{expected}, got {actual}"
    );
}

/// The single color a flat paint carries, or `None` for a ramp.
fn solid(paint: &WirePaint) -> Option<RgbaF32> {
    paint.ramp.is_none().then_some(paint.color)
}

/// A ramp's `(t = 0, t = 1)` stop colors, or `None` for a flat paint. A
/// ramp rides a white stroke, so its stops are the colors that show.
/// Stops are stored quantized, so the comparisons below go through
/// [`SrgbaU8`] rather than the float color.
fn gradient(paint: &WirePaint) -> Option<(SrgbaU8, SrgbaU8)> {
    let ramp = paint.ramp?;
    assert_eq!(paint.color, RgbaF32::WHITE, "a ramp multiplies white");
    Some((
        ramp.stops[0].color().into(),
        ramp.stops[ramp.stops.len() - 1].color().into(),
    ))
}

#[test]
fn emphasis_tiers_fade_dim_and_lift() {
    let canvas = RgbaF32::new(0.0, 0.0, 0.0, 1.0);
    let c = RgbaF32::new(1.0, 0.5, 0.0, 1.0);
    let rest = WireEmphasis::resolve(canvas, WireSet::Standing);
    // Rest pulls 15% toward the (black) canvas: r 1.0→0.85, g 0.5→0.425.
    let dimmed = rest.tint(c, Tier::Rest);
    assert_close(dimmed.r, 0.85);
    assert_close(dimmed.g, 0.425);
    assert_close(dimmed.b, 0.0);
    // Emphasis keeps the full color and lifts the width by 1.25×.
    assert_eq!(rest.tint(c, Tier::Emphasized), c);
    assert_eq!(width(2.0, Tier::Emphasized), 2.5);
    assert_eq!(width(2.0, Tier::Rest), 2.0);
    // Endpoint hover carries the emphasis at rest…
    assert_eq!(
        rest.stroke(2.0, WireMark::EndpointHovered).tier,
        Tier::Emphasized
    );
    assert_eq!(rest.stroke(2.0, WireMark::Plain).tier, Tier::Rest);

    // …but a fading pass drops alpha to the fade constant and
    // suppresses hover even with a forced endpoint hover — the snap
    // target's forced hover must not re-emphasize a faded wire.
    let fading = WireEmphasis::resolve(canvas, WireSet::Faded);
    assert_eq!(fading.tint(c, Tier::Rest).a, WIRE_DRAG_FADE);
    assert_eq!(fading.tint(c, Tier::Emphasized).a, WIRE_DRAG_FADE);
    assert_eq!(
        fading.stroke(2.0, WireMark::EndpointHovered).tier,
        Tier::Rest
    );
}

#[test]
fn stroke_lets_the_broken_alarm_win_over_hover() {
    let rest = WireEmphasis::resolve(RgbaF32::new(0.0, 0.0, 0.0, 1.0), WireSet::Standing);

    // Plain wire: no lift at all.
    let idle = rest.stroke(2.0, WireMark::Plain);
    assert_eq!(idle.tier, Tier::Rest);
    assert_eq!(idle.width, 2.0);

    // Endpoint hover alone lifts both the tier and the width.
    let hover = rest.stroke(2.0, WireMark::EndpointHovered);
    assert_eq!(hover.tier, Tier::Emphasized);
    assert_eq!(hover.width, 2.5);

    // Broken (hovered or not): the tier stays at rest so the caller takes the
    // flat alarm color, but the width lift is kept.
    let broken = rest.stroke(2.0, WireMark::Broken);
    assert_eq!(broken.tier, Tier::Rest);
    assert_eq!(broken.width, 2.5);

    // A fading pass still gives a broken wire full width — that's the
    // alarm read against the dimmed rest of the set.
    let fading = WireEmphasis::resolve(RgbaF32::new(0.0, 0.0, 0.0, 1.0), WireSet::Faded);
    let faded_hover = fading.stroke(2.0, WireMark::EndpointHovered);
    assert_eq!(faded_hover.tier, Tier::Rest);
    assert_eq!(faded_hover.width, 2.0);
    assert_eq!(fading.stroke(2.0, WireMark::Broken).width, 2.5);
}

#[test]
fn paint_ramps_differing_ends_and_flattens_equal_ones() {
    let canvas = RgbaF32::new(0.0, 0.0, 0.0, 1.0);
    let rest = WireEmphasis::resolve(canvas, WireSet::Standing);
    let a = RgbaF32::new(1.0, 0.0, 0.0, 1.0);
    let b = RgbaF32::new(0.0, 1.0, 0.0, 1.0);

    // Distinct ends run p0 → p3 as a gradient, each stop taken through this
    // frame's tier — the rest-dim pull, not the raw color.
    let (start, end) = gradient(&rest.paint(WireTint::new(a, b), Tier::Rest))
        .expect("two different endpoint colors lower to a gradient");
    assert_eq!(start, rest.tint(a, Tier::Rest).into());
    assert_eq!(end, rest.tint(b, Tier::Rest).into());
    assert_ne!(start, end, "and the two ends stay distinguishable");
    // Emphasized keeps both stops at full strength.
    let (start, end) = gradient(&rest.paint(WireTint::new(a, b), Tier::Emphasized))
        .expect("still a gradient when emphasized");
    assert_eq!((start, end), (a.into(), b.into()));

    // Equal ends — an event wire, or a type-mismatched data wire — collapse to
    // one flat paint rather than a ramp between two identical stops.
    assert_eq!(
        solid(&rest.paint(WireTint::flat(a), Tier::Emphasized)),
        Some(a),
        "a flat tint paints solid"
    );
    // `WireTint::new` with the same color both sides is the same thing: the
    // collapse is decided on the resolved colors, not on which constructor ran.
    assert_eq!(
        solid(&rest.paint(WireTint::new(a, a), Tier::Emphasized)),
        Some(a),
        "equal ends collapse however they were built"
    );
    // Two colors that differ *only* before the tier still differ after it, so
    // the collapse can't swallow a real gradient.
    assert!(
        gradient(&rest.paint(WireTint::new(a, b), Tier::Rest)).is_some(),
        "the tier is applied per end, not to a pre-collapsed pair"
    );

    // A fading pass drops both stops to the fade alpha and keeps the gradient;
    // a flat tint keeps painting solid through it.
    let fading = WireEmphasis::resolve(canvas, WireSet::Faded);
    let (start, end) = gradient(&fading.paint(WireTint::new(a, b), Tier::Rest))
        .expect("a faded wire keeps its gradient");
    assert_eq!(start, a.with_alpha(WIRE_DRAG_FADE).into());
    assert_eq!(end, b.with_alpha(WIRE_DRAG_FADE).into());
    assert_eq!(
        solid(&fading.paint(WireTint::flat(a), Tier::Rest)).map(|c| c.a),
        Some(WIRE_DRAG_FADE),
        "and a flat one keeps painting solid"
    );
}

#[test]
fn a_node_key_names_the_node_its_glyph_hangs_off() {
    use scenarium::NodeId;

    use crate::core::document::node_key::NodeKey;
    use crate::core::document::port_ref::{PortKind, PortRef};
    use crate::gui::event_ref::EventRef;
    use crate::gui::pane::graph_ui::paint::wire::glyph_drag::GlyphDrag;

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

/// The curve runs from `p0` at `t = 0` to `p3` at `t = 1`, and at `t = ½`
/// weighs the four points `⅛, ⅜, ⅜, ⅛`: over `x = 0, 30, 60, 90` and
/// `y = 0, 40, 40, 0` that is `(0 + 11.25 + 22.5 + 11.25, 0 + 15 + 15 + 0)` =
/// `(45, 30)`, every term exact in f32.
#[test]
fn at_runs_the_curve_from_p0_to_p3() {
    let w = Wire {
        p0: Vec2::new(0.0, 0.0),
        p1: Vec2::new(30.0, 40.0),
        p2: Vec2::new(60.0, 40.0),
        p3: Vec2::new(90.0, 0.0),
    };
    assert_eq!(w.at(0.0), w.p0);
    assert_eq!(w.at(1.0), w.p3);
    assert_eq!(w.at(0.5), Vec2::new(45.0, 30.0));
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
