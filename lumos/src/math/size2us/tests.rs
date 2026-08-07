use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

#[test]
fn size_measures_extent_and_indexes_row_major() {
    let size = Size2us::new(4, 3);
    assert_eq!((size.width, size.height), (4, 3));
    assert_eq!(size.pixel_count(), 12);

    // Row-major: the index advances by 1 across a row and by `width` down a column.
    assert_eq!(size.index_of(Vec2us::ZERO), 0);
    assert_eq!(size.index_of(Vec2us::new(3, 0)), 3);
    assert_eq!(size.index_of(Vec2us::new(0, 1)), 4);
    assert_eq!(size.index_of(Vec2us::new(3, 2)), 11);

    // The last in-bounds pixel is the last index; one past either axis is outside.
    assert!(size.contains(Vec2us::new(3, 2)));
    assert!(!size.contains(Vec2us::new(4, 2)));
    assert!(!size.contains(Vec2us::new(3, 3)));

    // `point_of` inverts `index_of` over the whole grid.
    assert_eq!(size.point_of(0), Vec2us::ZERO);
    assert_eq!(size.point_of(3), Vec2us::new(3, 0));
    assert_eq!(size.point_of(4), Vec2us::new(0, 1));
    assert_eq!(size.point_of(11), Vec2us::new(3, 2));
    for index in 0..size.pixel_count() {
        assert_eq!(size.index_of(size.point_of(index)), index);
    }

    // Width and height are not interchangeable — a transposed size indexes differently.
    let transposed = Size2us::new(3, 4);
    assert_ne!(size, transposed);
    assert_eq!(size.pixel_count(), transposed.pixel_count());
    assert_ne!(
        size.index_of(Vec2us::new(1, 1)),
        transposed.index_of(Vec2us::new(1, 1))
    );
    // Same index, different decomposition: 5 is (1, 1) at width 4 but (2, 1) at width 3.
    assert_eq!(size.point_of(5), Vec2us::new(1, 1));
    assert_eq!(transposed.point_of(5), Vec2us::new(2, 1));
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "past the last pixel")]
fn point_of_rejects_an_index_past_the_grid() {
    Size2us::new(4, 3).point_of(12);
}

#[test]
fn size_round_trips_through_a_tuple_in_declaration_order() {
    let size = Size2us::from((7, 2));
    assert_eq!((size.width, size.height), (7, 2));
    assert_eq!(<(usize, usize)>::from(size), (7, 2));
    assert_eq!(Size2us::default(), Size2us::new(0, 0));
}

#[test]
#[should_panic(expected = "must fit in usize")]
fn pixel_count_rejects_an_overflowing_extent() {
    Size2us::new(usize::MAX, 2).pixel_count();
}
