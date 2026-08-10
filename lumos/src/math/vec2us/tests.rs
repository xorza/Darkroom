use crate::testing::prelude::*;
#[test]
fn construction_constants_arithmetic_and_tuple_conversions_are_exact() {
    let left = Vec2us::new(5, 7);
    let right = Vec2us::new(2, 3);

    assert_eq!(Vec2us::ZERO, Vec2us { x: 0, y: 0 });
    assert_eq!(left + right, Vec2us::new(7, 10));
    assert_eq!(left - right, Vec2us::new(3, 4));
    assert_eq!(Vec2us::from((11, 13)), Vec2us::new(11, 13));
    assert_eq!(<(usize, usize)>::from(left), (5, 7));
}

/// Row-major offsets, hand-computed rather than restated.
///
/// Asserting `to_index(width) == y * width + x` writes the implementation's own formula on the
/// expected side, so it can only catch a transposition. These are the arithmetic done separately:
/// at width 5 the point (2, 3) is three whole rows plus two, so index 17.
///
/// The inverse lives on `Size2us::point_of`, which knows the whole extent and can bounds-check;
/// `Vec2us` alone has no height, so a `y` past any grid is still a valid offset here.
#[test]
fn row_major_index_matches_the_hand_computed_offset() {
    for (width, point, expected) in [
        (5, Vec2us::ZERO, 0),
        (5, Vec2us::new(4, 0), 4),
        (5, Vec2us::new(0, 1), 5),
        (5, Vec2us::new(2, 3), 17),
        (5, Vec2us::new(4, 7), 39),
        // A single-column grid: the index is the row.
        (1, Vec2us::ZERO, 0),
        (1, Vec2us::new(0, 7), 7),
        (128, Vec2us::new(127, 0), 127),
        (128, Vec2us::new(0, 1), 128),
        (128, Vec2us::new(127, 7), 1023),
    ] {
        assert_eq!(
            point.to_index(width),
            expected,
            "{point:?} at width {width}"
        );
    }

    // Not interchangeable: swapping the axes lands somewhere else, which is the bug the formula
    // restated on both sides could hide.
    assert_ne!(Vec2us::new(2, 3).to_index(5), Vec2us::new(3, 2).to_index(5));

    // `Size2us::point_of` inverts it wherever the point is actually inside a grid.
    let size = Size2us::new(5, 8);
    for index in 0..size.pixel_count() {
        assert_eq!(size.point_of(index).to_index(5), index);
    }
}
