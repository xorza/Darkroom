use std::collections::BTreeMap;

use glam::Vec2;
use scenarium::{InputPort, NodeId, OutputPort};

use crate::core::document::graph_view::GraphView;
use crate::core::document::route::Route;
use crate::core::document::tie::{Tie, TieAngle, TieId};
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::gui::pane::graph_ui::frame::tie_layout::{Span, TieLayout};

/// A data wire of its own, from a fresh node to another.
fn wire() -> WireKey {
    WireKey::Data(DataWire {
        output: OutputPort::new(NodeId::unique(), 0),
        input: InputPort::new(NodeId::unique(), 0),
    })
}

/// A view with one tie at `center`, turned by `angle`, through which each of `wires` runs, and
/// the layout of it with each wire's ends at its span.
fn tied(center: Vec2, angle: TieAngle, wires: &[(WireKey, Span)]) -> TieLayout {
    let tie = TieId::unique();
    let mut view = GraphView::default();
    view.ties.insert(tie, Tie { pos: center, angle });
    for &(wire, _) in wires {
        view.routes.insert(wire, Route::new([tie]));
    }
    let spans: BTreeMap<WireKey, Span> = wires.iter().copied().collect();
    let mut layout = TieLayout::default();
    layout.rebuild(&view, |wire| spans.get(&wire).copied());
    layout
}

fn span(start: (f32, f32), end: (f32, f32)) -> Span {
    Span {
        start: Vec2::from(start),
        end: Vec2::from(end),
    }
}

/// The slot of `wire` in its one tie: where its first piece ends.
fn slot(layout: &TieLayout, wire: WireKey) -> Vec2 {
    layout.pieces(wire).unwrap()[0].p3
}

/// The playground's mini scene: outputs at x 200 and y 83, 105, 213, 303, inputs at x 460 and
/// y 243, 83, 265, 105, and a tie at (330, 175). The chords `(260, 160)`, `(260, -22)`,
/// `(260, 52)` and `(260, -198)` normalise and sum to about `(3.624, 0.030)`, so the strands
/// run along `x` and the axis along `+y`. Every chord's midpoint lies on `x = 330`, so along the
/// axis it is its `y` less 175: m1 `163 - 175 = -12`, m2 `94 - 175 = -81`, m3 `239 - 175 = 64`,
/// m4 `204 - 175 = 29`. That orders the band m2, m1, m4, m3, at `-13.5`, `-4.5`, `4.5` and
/// `13.5` along the axis: `(rank - 1.5) · 9`.
#[test]
fn the_barycenters_order_a_band() {
    let [m1, m2, m3, m4] = [wire(), wire(), wire(), wire()];
    let layout = tied(
        Vec2::new(330.0, 175.0),
        TieAngle::Auto,
        &[
            (m1, span((200.0, 83.0), (460.0, 243.0))),
            (m2, span((200.0, 105.0), (460.0, 83.0))),
            (m3, span((200.0, 213.0), (460.0, 265.0))),
            (m4, span((200.0, 303.0), (460.0, 105.0))),
        ],
    );
    let band = &layout.bands()[0];
    assert_eq!(band.strand_count(), 4);
    assert!(
        band.dir.x > 0.9999 && band.axis.y > 0.9999,
        "the strands run along x, the axis along +y: {band:?}"
    );
    for (wire, along) in [(m2, -13.5), (m1, -4.5), (m4, 4.5), (m3, 13.5)] {
        assert_eq!(slot(&layout, wire), band.center + band.axis * along);
    }
}

/// Two strands mirrored about the band's line, chords `(200, 100)` and `(200, -100)`, normalise
/// to `(x, y)` and `(x, -y)`, whose sum `(2x, 0)` normalises to exactly `(1, 0)`.
#[test]
fn mirrored_strands_run_the_band_square_to_them() {
    let layout = tied(
        Vec2::new(100.0, 0.0),
        TieAngle::Auto,
        &[
            (wire(), span((0.0, -50.0), (200.0, 50.0))),
            (wire(), span((0.0, 50.0), (200.0, -50.0))),
        ],
    );
    let band = &layout.bands()[0];
    assert_eq!((band.dir, band.axis), (Vec2::X, Vec2::Y));
}

/// A band pinned to 90° stands along `+y`, so its strands run along `+x`: `(sin 90°, -cos 90°)`.
/// 90° in radians is no `f32`, so its cosine misses zero by `4.4e-8`, under the `1e-7` this
/// allows.
#[test]
fn a_band_pinned_to_ninety_degrees_runs_its_strands_along_x() {
    let layout = tied(
        Vec2::ZERO,
        TieAngle::Pinned(90.0),
        &[(wire(), span((-100.0, -80.0), (100.0, 40.0)))],
    );
    let band = &layout.bands()[0];
    assert!(band.dir.abs_diff_eq(Vec2::X, 1e-7), "{band:?}");
    assert!(band.axis.abs_diff_eq(Vec2::Y, 1e-7), "{band:?}");
}

/// Three level strands at y -10, 0 and 10 through a tie at the origin run along `x`, so the axis
/// is `+y` and the slots sit `(rank - 1) · 9` along it: -9, 0 and +9.
#[test]
fn three_strands_sit_nine_apart_about_the_center() {
    let [low, mid, high] = [wire(), wire(), wire()];
    let layout = tied(
        Vec2::ZERO,
        TieAngle::Auto,
        &[
            (high, span((-100.0, 10.0), (100.0, 10.0))),
            (low, span((-100.0, -10.0), (100.0, -10.0))),
            (mid, span((-100.0, 0.0), (100.0, 0.0))),
        ],
    );
    for (wire, y) in [(low, -9.0), (mid, 0.0), (high, 9.0)] {
        assert_eq!(slot(&layout, wire), Vec2::new(0.0, y));
    }
}

/// A tied wire is G1 at its slot: both pieces run there along the strand direction, `(1, 0)` for
/// a level wire. Each handle is `0.4` of its piece's chord within `[24, 140]`: the first piece's
/// chord from `(-100, 0)` to the slot `(0, 0)` is 100, so its handle is 40, and the second's,
/// 400 long, is held to 140.
#[test]
fn a_tied_wire_keeps_its_tangent_through_the_slot() {
    let level = wire();
    let layout = tied(
        Vec2::ZERO,
        TieAngle::Auto,
        &[(level, span((-100.0, 0.0), (400.0, 0.0)))],
    );
    let [into, out_of] = layout.pieces(level).unwrap() else {
        panic!("one tie makes two pieces");
    };
    assert_eq!(into.p3, out_of.p0);
    assert_eq!(into.p2, into.p3 - Vec2::new(40.0, 0.0));
    assert_eq!(out_of.p1, out_of.p0 + Vec2::new(140.0, 0.0));
    assert_eq!(
        into.p1,
        into.p0 + Vec2::new(40.0, 0.0),
        "the port end leaves along +x"
    );
    assert!(
        layout.pieces(wire()).is_none(),
        "an untied wire has no pieces"
    );
}

/// A tied event wire meets its subscriber pin as an untied one does, from the up-left: the last
/// piece's arriving handle runs along `(1, 1) / √2`, the direction `Wire::event` arrives in.
#[test]
fn a_tied_event_wire_meets_its_pin_from_the_up_left() {
    use scenarium::{EventPort, Subscription};

    use crate::gui::pane::graph_ui::frame::wire::Wire;

    let event = WireKey::Event(Subscription::new(
        EventPort::new(NodeId::unique(), 0),
        NodeId::unique(),
    ));
    let (start, end) = ((-100.0, 0.0), (100.0, 80.0));
    let layout = tied(Vec2::ZERO, TieAngle::Auto, &[(event, span(start, end))]);
    let [_, into_pin] = layout.pieces(event).unwrap() else {
        panic!("one tie makes two pieces");
    };
    let untied = Wire::event(Vec2::from(start), Vec2::from(end));
    let arriving = |wire: &Wire| (wire.p3 - wire.p2).normalize();
    assert!(
        arriving(into_pin).abs_diff_eq(arriving(&untied), 1e-6),
        "{:?} against {:?}: the two unit vectors agree to f32 rounding",
        arriving(into_pin),
        arriving(&untied)
    );
    assert_eq!(into_pin.p3, Vec2::from(end));
}

/// Two ties in a chain keep one strand order. Both strands leave `t1` from different heights and
/// meet `t2`'s far side at one point, so on the first pass `t2` sees equal barycenters and breaks
/// them by wire key. The later passes read `t1`'s slots instead of its center, so `t2` takes
/// `t1`'s order, low over high, even with the keys the other way round.
#[test]
fn chained_ties_keep_one_strand_order() {
    let mut keys = [wire(), wire()];
    keys.sort_unstable();
    // The key that sorts last takes the low start, so a tie broken by key alone would flip it.
    let [high_start, low_start] = keys;
    let [t1, t2] = [TieId::unique(), TieId::unique()];
    let mut view = GraphView::default();
    for (tie, x) in [(t1, 0.0), (t2, 100.0)] {
        view.ties.insert(
            tie,
            Tie {
                pos: Vec2::new(x, 0.0),
                angle: TieAngle::Pinned(90.0),
            },
        );
    }
    let spans = BTreeMap::from([
        (low_start, span((-200.0, -50.0), (300.0, 0.0))),
        (high_start, span((-200.0, 50.0), (300.0, 0.0))),
    ]);
    for &wire in spans.keys() {
        view.routes.insert(wire, Route::new([t1, t2]));
    }
    let mut layout = TieLayout::default();
    layout.rebuild(&view, |wire| spans.get(&wire).copied());
    let slot_at = |wire, tie_step: usize| layout.pieces(wire).unwrap()[tie_step].p3;
    for step in [0, 1] {
        assert!(
            slot_at(low_start, step).y < slot_at(high_start, step).y,
            "tie {step}: the low strand stays above the high one"
        );
    }
}
