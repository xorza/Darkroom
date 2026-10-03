use crate::internals::prelude::*;
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
