use crate::math::vec2us::Vec2us;

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

#[test]
fn row_major_index_matches_the_hand_computed_offset() {
    // The inverse lives on `Size2us::point_of`, which knows the whole extent and can bounds-check.
    for width in [1, 5, 128] {
        for point in [
            Vec2us::new(0, 0),
            Vec2us::new(width - 1, 0),
            Vec2us::new(0, 7),
            Vec2us::new(width - 1, 7),
        ] {
            assert_eq!(point.to_index(width), point.y * width + point.x);
        }
    }
}
