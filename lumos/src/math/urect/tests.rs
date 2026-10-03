use crate::math::urect::URect;
use crate::testing::prelude::*;
use std::panic;

#[test]
fn urect_accumulation_uses_exclusive_max() {
    const LEFT: URect = URect::new(Vec2us::new(2, 3), Vec2us::new(6, 9));

    assert_eq!(URect::default(), URect::empty());
    assert_eq!((LEFT.width(), LEFT.height(), LEFT.area()), (4, 6, 24));
    // Inverted bounds saturate to zero instead of wrapping.
    assert_eq!((URect::empty().width(), URect::empty().area()), (0, 0));
    assert!(LEFT.contains(Vec2us::new(2, 3)));
    assert!(LEFT.contains(Vec2us::new(5, 8)));
    assert!(!LEFT.contains(Vec2us::new(6, 8)));
    assert!(!LEFT.contains(Vec2us::new(5, 9)));
    assert!(panic::catch_unwind(|| URect::new(Vec2us::new(1, 1), Vec2us::ZERO)).is_err());

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
