use crate::common::luma::{WEIGHTS_Q16, round_q16};

/// `0.2126 · 65536 = 13933.4`, `0.7152 · 65536 = 46871.3`, `0.0722 · 65536 = 4731.8`.
#[test]
fn fixed_point_weights_are_the_nearest_and_sum_to_one() {
    assert_eq!(WEIGHTS_Q16, [13933, 46871, 4732]);
}

#[test]
fn rounding_takes_the_nearest_and_ties_to_even() {
    for (sum, rounded) in [
        (0, 0),
        (0x7FFF, 0),
        (0x8000, 0),
        (0x8001, 1),
        (0x1_7FFF, 1),
        (0x1_8000, 2),
        (0x2_8000, 2),
        (0x2_8001, 3),
        (255 << 16, 255),
    ] {
        assert_eq!(round_q16(sum), rounded, "{sum:#x}");
    }
}
