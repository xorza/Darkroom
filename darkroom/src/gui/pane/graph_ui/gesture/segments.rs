//! Whether two segments cross: the one rule the breaker and the tie line cut wires by.
//!
//! Half-open: a point on a segment's line counts on its positive side, so the point two chords
//! of a wire share belongs to exactly one of them, and a stroke through it crosses one, as the
//! crossing-number point-in-polygon test treats a vertex
//! ([Wikipedia](https://en.wikipedia.org/wiki/Point_in_polygon)). Both chords read that point's
//! side from one expression, so rounding cannot drop it from both. Refused: a strict test, under
//! which a stroke through the shared point crosses neither chord; a closed one, under which it
//! crosses both and a stroke that only grazes a wire cuts it.

use glam::Vec2;

/// Whether `a` and `b` cross: each one's ends lie on the two sides of the other's line. A stroke
/// along a chord, with every end on the line, crosses nothing.
pub(crate) fn cross(a: [Vec2; 2], b: [Vec2; 2]) -> bool {
    straddles(a, b) && straddles(b, a)
}

/// Where along `a`, from 0 at its start to 1 at its end, `a` crosses `b`, or `None` when it does
/// not cross it.
pub(crate) fn crossing(a: [Vec2; 2], b: [Vec2; 2]) -> Option<f32> {
    if !cross(a, b) {
        return None;
    }
    // Two segments that cross are not parallel, so the denominator is not zero; the clamp holds
    // a quotient rounded past an end to the segment.
    let (line, segment) = (a[1] - a[0], b[1] - b[0]);
    let along = (b[0] - a[0]).perp_dot(segment) / line.perp_dot(segment);
    Some(along.clamp(0.0, 1.0))
}

/// Whether `segment`'s ends lie on the two sides of `line`'s line.
fn straddles(line: [Vec2; 2], segment: [Vec2; 2]) -> bool {
    side(line, segment[0]) != side(line, segment[1])
}

/// Whether `point` lies on `line`'s positive side or on the line itself.
fn side([start, end]: [Vec2; 2], point: Vec2) -> bool {
    (end - start).perp_dot(point - start) >= 0.0
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use crate::gui::pane::graph_ui::gesture::segments::{cross, crossing};

    /// A level wire sampled at x = 0, 50 and 100: a vertical stroke at x = 50 passes through the
    /// point its two chords share and crosses exactly one of them, from either end; one at x = 25
    /// crosses the first only. A level stroke touching the apex of a V crosses neither arm; a
    /// stroke along a chord crosses nothing; one that stops short misses.
    #[test]
    fn a_stroke_through_a_shared_point_crosses_one_chord() {
        let p = |x: f32, y: f32| Vec2::new(x, y);
        let chords = [[p(0.0, 0.0), p(50.0, 0.0)], [p(50.0, 0.0), p(100.0, 0.0)]];
        for stroke in [
            [p(50.0, -10.0), p(50.0, 10.0)],
            [p(50.0, 10.0), p(50.0, -10.0)],
        ] {
            let crossed = chords.map(|chord| cross(stroke, chord));
            assert_eq!(
                crossed.iter().filter(|&&c| c).count(),
                1,
                "{stroke:?}: {crossed:?}"
            );
        }
        assert_eq!(
            chords.map(|chord| cross([p(25.0, -10.0), p(25.0, 10.0)], chord)),
            [true, false]
        );

        let arms = [[p(-10.0, 10.0), p(0.0, 0.0)], [p(0.0, 0.0), p(10.0, 10.0)]];
        let touch = [p(-5.0, 0.0), p(5.0, 0.0)];
        assert_eq!(arms.map(|arm| cross(touch, arm)), [false, false]);

        assert!(
            !cross([p(10.0, 0.0), p(40.0, 0.0)], chords[0]),
            "along a chord"
        );
        assert!(
            !cross([p(25.0, 5.0), p(25.0, 10.0)], chords[0]),
            "short of it"
        );
    }

    /// A stroke from (25, -10) to (25, 10) crosses the chord from (0, 0) to (100, 0) halfway along
    /// itself, 10 of its 20, and the chord a quarter of the way along it, 25 of its 100; one that
    /// misses has no crossing.
    #[test]
    fn a_crossing_reports_where_along_the_first_segment() {
        let p = |x: f32, y: f32| Vec2::new(x, y);
        let (stroke, chord) = (
            [p(25.0, -10.0), p(25.0, 10.0)],
            [p(0.0, 0.0), p(100.0, 0.0)],
        );
        assert_eq!(crossing(stroke, chord), Some(0.5));
        assert_eq!(crossing(chord, stroke), Some(0.25));
        assert_eq!(crossing([p(120.0, -10.0), p(120.0, 10.0)], chord), None);
    }
}
