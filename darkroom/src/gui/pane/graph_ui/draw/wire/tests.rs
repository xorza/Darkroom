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
        ramp.stops[0].color().to_srgba8(),
        ramp.stops[ramp.stops.len() - 1].color().to_srgba8(),
    ))
}

/// Each pass set and wire mark gives one tier and width: at rest an endpoint hover emphasizes
/// and lifts 2 to 2.5; a fading pass suppresses hover, as the snap target's forced hover must
/// not re-emphasize a faded wire; a broken wire stays at the rest tier, so the caller takes the
/// flat alarm color, but keeps the lift in either set, the alarm read against the dimmed rest.
/// The tints: rest pulls 15% toward the black canvas (r 1.0 → 0.85, g 0.5 → 0.425), emphasis
/// keeps the color, and a fade drops alpha to the fade constant in either tier.
#[test]
fn emphasis_tiers_fade_dim_and_lift() {
    let canvas = RgbaF32::new(0.0, 0.0, 0.0, 1.0);
    for (set, mark, tier, width) in [
        (WireSet::Standing, WireMark::Plain, Tier::Rest, 2.0),
        (
            WireSet::Standing,
            WireMark::EndpointHovered,
            Tier::Emphasized,
            2.5,
        ),
        (WireSet::Standing, WireMark::Broken, Tier::Rest, 2.5),
        (WireSet::Faded, WireMark::Plain, Tier::Rest, 2.0),
        (WireSet::Faded, WireMark::EndpointHovered, Tier::Rest, 2.0),
        (WireSet::Faded, WireMark::Broken, Tier::Rest, 2.5),
    ] {
        let stroke = WireEmphasis::resolve(canvas, set).stroke(2.0, mark);
        assert_eq!(
            (stroke.tier, stroke.width),
            (tier, width),
            "{set:?} {mark:?}"
        );
    }

    let c = RgbaF32::new(1.0, 0.5, 0.0, 1.0);
    let rest = WireEmphasis::resolve(canvas, WireSet::Standing);
    let dimmed = rest.tint(c, Tier::Rest);
    assert_close(dimmed.r, 0.85);
    assert_close(dimmed.g, 0.425);
    assert_close(dimmed.b, 0.0);
    assert_eq!(rest.tint(c, Tier::Emphasized), c);
    let fading = WireEmphasis::resolve(canvas, WireSet::Faded);
    assert_eq!(fading.tint(c, Tier::Rest).a, WIRE_DRAG_FADE);
    assert_eq!(fading.tint(c, Tier::Emphasized).a, WIRE_DRAG_FADE);
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
    assert_eq!(start, rest.tint(a, Tier::Rest).to_srgba8());
    assert_eq!(end, rest.tint(b, Tier::Rest).to_srgba8());
    assert_ne!(start, end, "and the two ends stay distinguishable");
    // Emphasized keeps both stops at full strength.
    let (start, end) = gradient(&rest.paint(WireTint::new(a, b), Tier::Emphasized))
        .expect("still a gradient when emphasized");
    assert_eq!((start, end), (a.to_srgba8(), b.to_srgba8()));

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
    assert_eq!(start, a.with_alpha(WIRE_DRAG_FADE).to_srgba8());
    assert_eq!(end, b.with_alpha(WIRE_DRAG_FADE).to_srgba8());
    assert_eq!(
        solid(&fading.paint(WireTint::flat(a), Tier::Rest)).map(|c| c.a),
        Some(WIRE_DRAG_FADE),
        "and a flat one keeps painting solid"
    );
}

/// A piece of a ramped wire takes the whole ramp's colors at its two ends: the first half of red
/// to green runs from red to the ramp's middle, the second half from that same middle to green,
/// so the joint matches. A flat paint stays flat in every piece.
#[test]
fn a_piece_paints_its_share_of_the_wire_ramp() {
    let canvas = RgbaF32::new(0.0, 0.0, 0.0, 1.0);
    let rest = WireEmphasis::resolve(canvas, WireSet::Standing);
    let (a, b) = (
        RgbaF32::new(1.0, 0.0, 0.0, 1.0),
        RgbaF32::new(0.0, 1.0, 0.0, 1.0),
    );
    let whole = rest.paint(WireTint::new(a, b), Tier::Emphasized);
    let ramp = whole.ramp.unwrap();
    let middle = ramp.color_at(0.5).to_srgba8();
    assert_eq!(
        gradient(&whole.between(0.0, 0.5)),
        Some((a.to_srgba8(), middle))
    );
    assert_eq!(
        gradient(&whole.between(0.5, 1.0)),
        Some((middle, b.to_srgba8()))
    );
    assert_ne!(middle, a.to_srgba8(), "the middle is no end");
    let flat = rest.paint(WireTint::flat(a), Tier::Emphasized);
    assert_eq!(solid(&flat.between(0.25, 0.5)), Some(a));
}

/// Pieces of chords 30, 10 and 0 share a 40-long wire's ramp as `0..0.75`, `0.75..1` and
/// `1..1`; one piece takes it whole, and a wire with no length gives each piece the end.
#[test]
fn the_pieces_share_the_ramp_by_chord_length() {
    use glam::Vec2;

    use crate::gui::pane::graph_ui::frame::wire::Wire;

    let straight = |x0: f32, x1: f32| Wire {
        p0: Vec2::new(x0, 0.0),
        p1: Vec2::new(x0, 0.0),
        p2: Vec2::new(x1, 0.0),
        p3: Vec2::new(x1, 0.0),
    };
    let pieces = [
        straight(0.0, 30.0),
        straight(30.0, 40.0),
        straight(40.0, 40.0),
    ];
    assert_eq!(
        shares(&pieces).collect::<Vec<_>>(),
        [0.0..0.75, 0.75..1.0, 1.0..1.0]
    );
    assert_eq!(shares(&pieces[..1]).collect::<Vec<_>>(), [0.0..1.0]);
    assert_eq!(shares(&pieces[2..]).collect::<Vec<_>>(), [0.0..1.0]);
    assert_eq!(shares(&pieces).len(), 3);
}

/// A cut wire's bar sits halfway along its length, square to it: a level wire from (0, 0) to
/// (200, 0) is straight, so its middle is (100, 0), running along x; split at (120, 0) into two
/// level pieces, 120 and 80 long, its middle is still 100 along. Within a chord the parameter
/// runs linearly while the curve's speed does not, so the split middle lands within 0.02 px, a
/// fiftieth of a pixel, under what any screen shows.
#[test]
fn a_wires_middle_is_halfway_along_its_length() {
    let middle = Middle::of(&[Wire::data(Vec2::ZERO, Vec2::new(200.0, 0.0))]).expect("a length");
    assert!(
        middle.at.abs_diff_eq(Vec2::new(100.0, 0.0), 1e-3),
        "{:?}",
        middle.at
    );
    assert_eq!(middle.dir, Vec2::X);
    let split = [
        Wire::data(Vec2::ZERO, Vec2::new(120.0, 0.0)),
        Wire::data(Vec2::new(120.0, 0.0), Vec2::new(200.0, 0.0)),
    ];
    let middle = Middle::of(&split).expect("a length");
    assert!(
        middle.at.abs_diff_eq(Vec2::new(100.0, 0.0), 0.02),
        "{:?}",
        middle.at
    );
    assert_eq!(middle.dir, Vec2::X);
    let point = Wire {
        p0: Vec2::ONE,
        p1: Vec2::ONE,
        p2: Vec2::ONE,
        p3: Vec2::ONE,
    };
    assert!(Middle::of(&[point]).is_none(), "no length, no middle");
}
