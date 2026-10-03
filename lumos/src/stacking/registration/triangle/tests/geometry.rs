use super::*;

/// A triangle's ratios are its shorter sides over its longest, its vertices are ordered by the side
/// they face — shortest first — and its orientation is the sign of that ordering's turn.
///
/// The 3-4-5 triangle's sides are exactly 3, 4 and 5: ratios (0.6, 0.8), correctly rounded. Its
/// vertex at the origin faces the 5, (3, 0) faces the 4 and (0, 4) the 3, so indices `[a, b, c]`
/// reorder to `[c, b, a]`; from (0, 4) to (3, 0) to the origin turns clockwise, and the mirror
/// image turns the other way. Scale and translation change none of it. The equilateral triangle's
/// sides are 10 to the rounding of its irrational height: ratios 1 to a few ulps.
#[test]
fn triangle_ratios_roles_and_orientation() {
    let mirrored = THREE_FOUR_FIVE.map(|p| DVec2::new(-p.x, p.y));
    let moved = THREE_FOUR_FIVE.map(|p| p * 10.0 + DVec2::new(7.0, -2.0));
    for (positions, ratios, orientation) in [
        (THREE_FOUR_FIVE, (0.6, 0.8), Orientation::Clockwise),
        (moved, (0.6, 0.8), Orientation::Clockwise),
        (mirrored, (0.6, 0.8), Orientation::CounterClockwise),
    ] {
        let tri = Triangle::from_positions([10, 20, 30], positions).unwrap();
        assert_eq!(tri.ratios, ratios, "{positions:?}");
        assert_eq!(tri.indices, [30, 20, 10], "{positions:?}");
        assert_eq!(tri.orientation, orientation, "{positions:?}");
    }

    let equilateral = Triangle::from_positions(
        [0, 1, 2],
        [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            // Height `10·√3/2 = √75`.
            DVec2::new(5.0, 75.0f64.sqrt()),
        ],
    )
    .unwrap();
    assert!((equilateral.ratios.0 - 1.0).abs() <= 4.0 * f64::EPSILON);
    assert!((equilateral.ratios.1 - 1.0).abs() <= 4.0 * f64::EPSILON);
}

/// Degenerate and unstable shapes are refused. The side-ratio limit is `longest/shortest > 10`:
/// sides 1, √85 and exactly 10 — the shortest side `(0, 1)`, the longest `(6, 8)` — sit on it and
/// are kept, while moving the far vertex a ten-millionth further out refuses them.
#[test]
fn degenerate_and_elongated_triangles_are_refused() {
    let p = DVec2::new;
    for (positions, kept, why) in [
        ([p(0.0, 0.0), p(1.0, 1.0), p(2.0, 2.0)], false, "collinear"),
        (
            [p(0.0, 0.0), p(0.0, 0.0), p(1.0, 1.0)],
            false,
            "a repeated point",
        ),
        (
            [p(0.0, 0.0), p(100.0, 0.0), p(50.0, 1e-10)],
            false,
            "area² 2.5e-17, under 1e-6",
        ),
        (
            [p(0.0, 0.0), p(100.0, 0.0), p(50.0, 1.0)],
            true,
            "thin but sound: area² 2500, side ratio 2",
        ),
        (
            [p(0.0, 0.0), p(100.0, 0.0), p(100.0, 1.0)],
            false,
            "side ratio 100",
        ),
        (
            [p(0.0, 0.0), p(0.0, 1.0), p(6.0, 8.0)],
            true,
            "side ratio exactly 10",
        ),
        (
            [p(0.0, 0.0), p(0.0, 1.0), p(6.000_000_6, 8.000_000_8)],
            false,
            "side ratio 10.000 001",
        ),
    ] {
        assert_eq!(
            Triangle::from_positions([0, 1, 2], positions).is_some(),
            kept,
            "{why}"
        );
    }
}

/// Similarity is `|Δratio| < tolerance` on both ratios, strictly: ratios a dyadic 1/32 apart are
/// not similar at a tolerance of 1/32 and are at the next float up. Both ratios have to pass.
#[test]
fn similarity_is_strict_in_both_ratios() {
    let tri = |ratios| Triangle {
        indices: [0, 1, 2],
        ratios,
        orientation: Orientation::Clockwise,
    };
    let a = tri((0.5, 0.75));
    let step = 1.0 / 32.0;
    let b = tri((0.5 + step, 0.75));
    assert!(!a.is_similar(&b, step));
    assert!(a.is_similar(&b, step.next_up()));
    let c = tri((0.5, 0.75 + step));
    assert!(!a.is_similar(&c, step));
    assert!(a.is_similar(&c, step.next_up()));
    let d = tri((0.5 + step / 2.0, 0.75 + step));
    assert!(
        !a.is_similar(&d, step),
        "the first ratio passes, the second does not"
    );
}

/// Every order of the same three indexed points gives the same triangle: the side sort breaks ties
/// by original index.
#[test]
fn vertex_ordering_is_the_same_for_every_input_order() {
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(5.0, 0.0),
        DVec2::new(2.0, 7.0),
    ];
    let indices = [100, 200, 300];
    let reference = Triangle::from_positions(indices, points).unwrap();
    for (a, b, c) in [(0, 2, 1), (1, 0, 2), (1, 2, 0), (2, 0, 1), (2, 1, 0)] {
        let tri = Triangle::from_positions(
            [indices[a], indices[b], indices[c]],
            [points[a], points[b], points[c]],
        )
        .unwrap();
        assert_eq!(tri.indices, reference.indices, "({a}, {b}, {c})");
        assert_eq!(tri.orientation, reference.orientation, "({a}, {b}, {c})");
        assert_eq!(tri.ratios, reference.ratios, "({a}, {b}, {c})");
    }
}

/// The invariant tree holds each triangle's ratios as a point at the triangle's own index, and an
/// empty set has no tree.
#[test]
fn the_invariant_tree_indexes_triangles_by_their_ratios() {
    assert!(build_invariant_tree(&[]).is_none());
    let triangles = triangles_of(&IRREGULAR, 4);
    let tree = build_invariant_tree(&triangles).unwrap();
    assert_eq!(tree.len(), triangles.len());
    for (i, tri) in triangles.iter().enumerate() {
        assert_eq!(tree.get_point(i), DVec2::new(tri.ratios.0, tri.ratios.1));
    }
}

/// The fixture the matching tests rely on: all ten of `IRREGULAR`'s triangles are valid, and no two
/// are within 0.05 of each other on both ratios — the closest pair is 0.056 apart.
#[test]
fn irregular_triangles_are_pairwise_dissimilar() {
    let triangles = triangles_of(&IRREGULAR, 4);
    assert_eq!(triangles.len(), 10);
    for (i, a) in triangles.iter().enumerate() {
        for b in &triangles[i + 1..] {
            assert!(
                !a.is_similar(b, 0.05),
                "{:?} and {:?}",
                a.indices,
                b.indices
            );
        }
    }
}
