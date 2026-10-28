use crate::rect2us::Rect2us;
use glam::USizeVec2;

#[test]
fn accumulation_uses_exclusive_max() {
    const LEFT: Rect2us = Rect2us::new(USizeVec2::new(2, 3), USizeVec2::new(6, 9));

    assert_eq!(Rect2us::default(), Rect2us::empty());
    assert_eq!((LEFT.width(), LEFT.height(), LEFT.area()), (4, 6, 24));
    // Inverted bounds saturate to zero instead of wrapping.
    assert_eq!((Rect2us::empty().width(), Rect2us::empty().area()), (0, 0));
    assert!(LEFT.contains(USizeVec2::new(2, 3)));
    assert!(LEFT.contains(USizeVec2::new(5, 8)));
    assert!(!LEFT.contains(USizeVec2::new(6, 8)));
    assert!(!LEFT.contains(USizeVec2::new(5, 9)));

    let mut bounds = Rect2us::empty();
    bounds.include(USizeVec2::new(5, 3));
    assert_eq!(
        bounds,
        Rect2us::new(USizeVec2::new(5, 3), USizeVec2::new(6, 4))
    );
    bounds.include(USizeVec2::new(2, 7));
    assert_eq!(
        bounds,
        Rect2us::new(USizeVec2::new(2, 3), USizeVec2::new(6, 8))
    );
    bounds.include(USizeVec2::new(8, 1));
    assert_eq!(
        bounds,
        Rect2us::new(USizeVec2::new(2, 1), USizeVec2::new(9, 8))
    );

    let covered: Vec<USizeVec2> = (bounds.min.y..bounds.max.y)
        .flat_map(|y| (bounds.min.x..bounds.max.x).map(move |x| USizeVec2::new(x, y)))
        .collect();
    assert_eq!(covered.first(), Some(&USizeVec2::new(2, 1)));
    assert_eq!(covered.last(), Some(&USizeVec2::new(8, 7)));
    assert_eq!(covered.len(), 7 * 7);
    assert_eq!(bounds.area(), covered.len());
}

/// Inverted bounds are a caller's logic error, checked in debug builds.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "invalid rectangle bounds")]
fn inverted_bounds_panic_in_debug_builds() {
    Rect2us::new(USizeVec2::new(1, 1), USizeVec2::ZERO);
}
