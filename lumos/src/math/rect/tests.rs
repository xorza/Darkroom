use common::Vec2us;
use glam::Vec2;

use super::{Rect, URect};

#[test]
fn rect_construction_and_overlap_are_half_open_and_const() {
    const UNIT: Rect = Rect::new(Vec2::ZERO, Vec2::ONE);
    const CENTERED: Rect = Rect::from_center_half_extent(Vec2::splat(0.5), 0.5);
    const CASES: &[(Rect, f32)] = &[
        (Rect::new(Vec2::ZERO, Vec2::ONE), 1.0),
        (Rect::new(Vec2::new(0.5, 0.0), Vec2::new(1.5, 1.0)), 0.5),
        (Rect::new(Vec2::new(0.5, 0.5), Vec2::new(1.5, 1.5)), 0.25),
        (Rect::new(Vec2::new(1.0, 0.0), Vec2::new(2.0, 1.0)), 0.0),
        (Rect::new(Vec2::splat(2.0), Vec2::splat(3.0)), 0.0),
    ];

    assert_eq!(CENTERED, UNIT);
    for &(other, expected) in CASES {
        assert_eq!(UNIT.overlap_area(other), expected);
        assert_eq!(other.overlap_area(UNIT), expected);
    }

    assert!(std::panic::catch_unwind(|| Rect::new(Vec2::ONE, Vec2::ZERO)).is_err());
    assert!(std::panic::catch_unwind(|| Rect::from_center_half_extent(Vec2::ZERO, -1.0)).is_err());
}

#[test]
fn urect_accumulation_uses_exclusive_max_and_const_union() {
    const LEFT: URect = URect::new(Vec2us::new(2, 3), Vec2us::new(6, 9));
    const RIGHT: URect = URect::new(Vec2us::new(4, 6), Vec2us::new(10, 11));
    const UNION: URect = LEFT.union(RIGHT);
    const DISJOINT: URect = URect::new(Vec2us::new(20, 30), Vec2us::new(22, 33));
    const CONTAINED: URect = URect::new(Vec2us::new(3, 4), Vec2us::new(5, 8));

    assert_eq!(UNION, URect::new(Vec2us::new(2, 3), Vec2us::new(10, 11)));
    assert_eq!(LEFT.union(URect::empty()), LEFT);
    assert_eq!(URect::empty().union(LEFT), LEFT);
    assert_eq!(LEFT.union(CONTAINED), LEFT);
    assert_eq!(CONTAINED.union(LEFT), LEFT);
    assert_eq!(
        LEFT.union(DISJOINT),
        URect::new(Vec2us::new(2, 3), Vec2us::new(22, 33))
    );
    assert_eq!(LEFT.union(RIGHT), RIGHT.union(LEFT));
    assert_eq!(URect::default(), URect::empty());
    assert!(URect::empty().is_empty());
    assert!(!LEFT.is_empty());
    assert!(LEFT.contains(Vec2us::new(2, 3)));
    assert!(LEFT.contains(Vec2us::new(5, 8)));
    assert!(!LEFT.contains(Vec2us::new(6, 8)));
    assert!(!LEFT.contains(Vec2us::new(5, 9)));
    assert!(std::panic::catch_unwind(|| URect::new(Vec2us::new(1, 1), Vec2us::ZERO)).is_err());

    let mut bounds = URect::empty();
    bounds.include(Vec2us::new(5, 3));
    assert_eq!(bounds, URect::new(Vec2us::new(5, 3), Vec2us::new(6, 4)));
    bounds.include(Vec2us::new(2, 7));
    assert_eq!(bounds, URect::new(Vec2us::new(2, 3), Vec2us::new(6, 8)));
    bounds.include(Vec2us::new(8, 1));
    assert_eq!(bounds, URect::new(Vec2us::new(2, 1), Vec2us::new(9, 8)));

    let covered: Vec<Vec2us> = (bounds.min.y..bounds.max.y)
        .flat_map(|y| (bounds.min.x..bounds.max.x).map(move |x| Vec2us::new(x, y)))
        .collect();
    assert_eq!(covered.first(), Some(&Vec2us::new(2, 1)));
    assert_eq!(covered.last(), Some(&Vec2us::new(8, 7)));
    assert_eq!(covered.len(), 7 * 7);
}
