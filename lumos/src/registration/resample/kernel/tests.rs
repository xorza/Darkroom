use crate::internals::prelude::*;
use crate::registration::resample::kernel;
use crate::registration::resample::source_position::SourcePosition;

/// Catmull-Rom (`A = −½`) by hand, every value dyadic and so exact in f32:
/// inner branch `(1.5|x| − 2.5)·x² + 1`, outer `((−0.5|x| + 2.5)·|x| − 4)·|x| + 2`.
/// `K(0.25)` = (0.375 − 2.5)·0.0625 + 1 = 0.8671875; `K(0.5)` = (0.75 − 2.5)·0.25 + 1 = 0.5625;
/// `K(0.75)` = (1.125 − 2.5)·0.5625 + 1 = 0.2265625; `K(1)` = 0 from both branches;
/// `K(1.25)` = (1.875·1.25 − 4)·1.25 + 2 = −0.0703125; `K(1.5)` = (1.75·1.5 − 4)·1.5 + 2 = −0.0625;
/// `K(1.75)` = (1.625·1.75 − 4)·1.75 + 2 = −0.0234375; `K(2)` = 0, and nothing beyond.
///
/// Both branches have slope `−½` at 1 (`4.5x² − 5x` and `−1.5x² + 5x − 4`), so across `1 ± 1e-4`
/// the kernel falls by 1e-4; a jump between the branches would show on top of that. The f32
/// evaluation of terms near 2.5 that cancel to 5e-5 rounds to a few ulps of 2.5, under 1e-6.
#[test]
fn bicubic_kernel_hand_values() {
    for (x, expected) in [
        (0.0, 1.0),
        (0.25, 0.867_187_5),
        (0.5, 0.5625),
        (0.75, 0.226_562_5),
        (1.0, 0.0),
        (1.25, -0.070_312_5),
        (1.5, -0.0625),
        (1.75, -0.023_437_5),
        (2.0, 0.0),
        (2.5, 0.0),
    ] {
        assert_eq!(kernel::internals::bicubic_kernel(x), expected, "K({x})");
        assert_eq!(kernel::internals::bicubic_kernel(-x), expected, "K(-{x})");
    }
    let fall = kernel::internals::bicubic_kernel(1.0 - 1e-4)
        - kernel::internals::bicubic_kernel(1.0 + 1e-4);
    assert!((fall - 1e-4).abs() < 1e-6, "{fall}");
}

/// The nearest pixel, a half rounding up: on `[[10, 20], [30, 40]]`, (0.4, 0.4) is pixel (0, 0),
/// (1.4, 0.4) is (1, 0), (0.4, 1.4) is (0, 1), and (0.5, 0) rounds up to (1, 0). In the rim past
/// the last centre it holds to the edge pixel: (1.45, −0.45) is (1, 0).
#[test]
fn nearest_rounds_a_half_up() {
    let input = Buffer2::new(2, 2, vec![10.0, 20.0, 30.0, 40.0]);
    let size = Size2us::new(2, 2);
    for (x, y, expected) in [
        (0.4, 0.4, 10.0),
        (1.4, 0.4, 20.0),
        (0.4, 1.4, 30.0),
        (1.4, 1.4, 40.0),
        (0.5, 0.0, 20.0),
        (1.45, -0.45, 20.0),
    ] {
        let position = SourcePosition::within(DVec2::new(x, y), size).unwrap();
        assert_eq!(
            input.pixels()[kernel::nearest_index(size, position)],
            expected,
            "({x}, {y})"
        );
    }
}
