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

    // Width and height are not interchangeable — a transposed size indexes differently.
    let transposed = Size2us::new(3, 4);
    assert_ne!(size, transposed);
    assert_eq!(size.pixel_count(), transposed.pixel_count());
    assert_ne!(
        size.index_of(Vec2us::new(1, 1)),
        transposed.index_of(Vec2us::new(1, 1))
    );
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
