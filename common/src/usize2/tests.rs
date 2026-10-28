use crate::usize2::USize2;
use glam::USizeVec2;
#[test]
fn size_measures_extent_and_indexes_row_major() {
    let size = USize2::new(4, 3);
    assert_eq!((size.width, size.height), (4, 3));
    assert_eq!(size.pixel_count(), 12);

    // Row-major: the index advances by 1 across a row and by `width` down a column.
    assert_eq!(size.idx_of(USizeVec2::ZERO), 0);
    assert_eq!(size.idx_of(USizeVec2::new(3, 0)), 3);
    assert_eq!(size.idx_of(USizeVec2::new(0, 1)), 4);
    assert_eq!(size.idx_of(USizeVec2::new(3, 2)), 11);

    // The last in-bounds pixel is the last index; one past either axis is outside.
    assert!(size.contains(USizeVec2::new(3, 2)));
    assert!(!size.contains(USizeVec2::new(4, 2)));
    assert!(!size.contains(USizeVec2::new(3, 3)));

    // `point_of` inverts `idx_of` over the whole grid.
    assert_eq!(size.point_of(0), USizeVec2::ZERO);
    assert_eq!(size.point_of(3), USizeVec2::new(3, 0));
    assert_eq!(size.point_of(4), USizeVec2::new(0, 1));
    assert_eq!(size.point_of(11), USizeVec2::new(3, 2));
    for idx in 0..size.pixel_count() {
        assert_eq!(size.idx_of(size.point_of(idx)), idx);
    }

    // Width and height are not interchangeable — a transposed size indexes differently.
    let transposed = USize2::new(3, 4);
    assert_ne!(size, transposed);
    assert_eq!(size.pixel_count(), transposed.pixel_count());
    assert_ne!(
        size.idx_of(USizeVec2::new(1, 1)),
        transposed.idx_of(USizeVec2::new(1, 1))
    );
    // Same index, different decomposition: 5 is (1, 1) at width 4 but (2, 1) at width 3.
    assert_eq!(size.point_of(5), USizeVec2::new(1, 1));
    assert_eq!(transposed.point_of(5), USizeVec2::new(2, 1));
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "past the last pixel")]
fn point_of_rejects_an_index_past_the_grid() {
    USize2::new(4, 3).point_of(12);
}

#[test]
fn size_round_trips_through_a_tuple_in_declaration_order() {
    let size = USize2::from((7, 2));
    assert_eq!((size.width, size.height), (7, 2));
    assert_eq!(<(usize, usize)>::from(size), (7, 2));
    assert_eq!(USize2::default(), USize2::new(0, 0));
}

#[test]
#[should_panic(expected = "must fit in usize")]
fn pixel_count_rejects_an_overflowing_extent() {
    USize2::new(usize::MAX, 2).pixel_count();
}
