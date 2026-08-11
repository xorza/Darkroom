use crate::math::urect::URect;
use crate::testing::prelude::*;

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
    assert_eq!((LEFT.width(), LEFT.height(), LEFT.area()), (4, 6, 24));
    // Inverted bounds saturate to zero instead of wrapping.
    assert_eq!((URect::empty().width(), URect::empty().area()), (0, 0));
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
    assert_eq!(bounds.area(), covered.len());
}
