use crate::math::floor_i32::FloorI32;

/// The library `floor` on both sides of 0, on integers and halves, and an ulp either side of an
/// integer: −3.5 → −4, −3 → −3, −0.5 → −1, −0.0 → 0, 0.5 → 0, 2 → 2, 2 − ulp → 1.
#[test]
fn floor_i32_is_the_library_floor() {
    for x in [
        -3.5f64,
        -3.0,
        -0.5,
        -0.0,
        0.5,
        2.0,
        2.0 - f64::EPSILON,
        -1e-300,
        123_456.75,
    ] {
        assert_eq!(x.floor_i32(), x.floor() as i32, "{x}");
        let narrow = x as f32;
        assert_eq!(narrow.floor_i32(), narrow.floor() as i32, "{narrow}");
    }
    assert_eq!((-0.5f32).floor_i32(), -1);
    assert_eq!((2.0f64 - f64::EPSILON).floor_i32(), 1);
}
