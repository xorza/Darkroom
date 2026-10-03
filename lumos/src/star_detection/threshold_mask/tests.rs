//! Tests for threshold mask creation (packed `BitBuffer2` version).

use crate::bit_buffer2::BitBuffer2;
use crate::internals::prelude::*;
use crate::star_detection::threshold_mask::create_residual_threshold_mask;
use crate::star_detection::threshold_mask::internals::test_params;

/// Helper to create filtered threshold mask for tests
fn create_residual_threshold_mask_test(
    filtered: &[f32],
    noise: &[f32],
    sigma: f32,
    size: Size2us,
) -> BitBuffer2 {
    let filtered = Buffer2::new(size.width, size.height, filtered.to_vec());
    let noise = Buffer2::new(size.width, size.height, noise.to_vec());
    let mut mask = BitBuffer2::new_filled(size, false);
    create_residual_threshold_mask(&filtered, &noise, test_params(sigma), &mut mask);
    mask
}

#[derive(Debug)]
struct ThresholdMaskCase {
    name: &'static str,
    pixels: &'static [f32],
    noise: &'static [f32],
    sigma: f32,
    size: Size2us,
    expected: &'static [bool],
}

/// A pixel is set where its residual exceeds σ·max(noise, floor), strictly: 0.3 against 3 × 0.1,
/// which rounds to the same f32, is not above it, and 0.30001 is. σ 3 and 4 pass a residual of 0.5
/// at noise 0.1, and σ 5 does not. A zero or negative noise is held to the floor.
#[test]
fn threshold_mask_truth_table() {
    let cases = [
        ThresholdMaskCase {
            name: "constant_above",
            pixels: &[50.0; 4],
            noise: &[10.0; 4],
            sigma: 3.0,
            size: Size2us::new(4, 1),
            expected: &[true; 4],
        },
        ThresholdMaskCase {
            name: "mixed",
            pixels: &[0.2, 0.4, 0.6, 0.8],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[false, true, true, true],
        },
        ThresholdMaskCase {
            name: "variable_noise",
            pixels: &[0.5; 4],
            noise: &[0.1, 0.2, 0.3, 0.05],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[true, false, false, true],
        },
        ThresholdMaskCase {
            name: "negative_pixels",
            pixels: &[-0.5, 0.5, -0.1, 0.4],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[false, true, false, true],
        },
        ThresholdMaskCase {
            name: "zero_noise_uses_epsilon",
            pixels: &[0.1, -0.1],
            noise: &[0.0; 2],
            sigma: 3.0,
            size: Size2us::new(2, 1),
            expected: &[true, false],
        },
        ThresholdMaskCase {
            name: "negative_noise_uses_epsilon",
            pixels: &[0.1, -0.1],
            noise: &[-0.1; 2],
            sigma: 3.0,
            size: Size2us::new(2, 1),
            expected: &[true, false],
        },
        ThresholdMaskCase {
            name: "standard_below",
            pixels: &[10.0; 4],
            noise: &[10.0; 4],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[false; 4],
        },
        ThresholdMaskCase {
            name: "high_noise_region",
            pixels: &[1.0; 2],
            noise: &[0.1, 0.5],
            sigma: 3.0,
            size: Size2us::new(2, 1),
            expected: &[true, false],
        },
        ThresholdMaskCase {
            name: "tiny_1x1_above",
            pixels: &[1.0],
            noise: &[0.1],
            sigma: 3.0,
            size: Size2us::new(1, 1),
            expected: &[true],
        },
        ThresholdMaskCase {
            name: "tiny_1x1_below",
            pixels: &[0.0],
            noise: &[0.1],
            sigma: 3.0,
            size: Size2us::new(1, 1),
            expected: &[false],
        },
        ThresholdMaskCase {
            name: "exact_threshold_is_false",
            pixels: &[0.3, 0.30001],
            noise: &[0.1; 2],
            sigma: 3.0,
            size: Size2us::new(2, 1),
            expected: &[false, true],
        },
        ThresholdMaskCase {
            name: "sigma_3",
            pixels: &[0.5; 4],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[true; 4],
        },
        ThresholdMaskCase {
            name: "sigma_4",
            pixels: &[0.5; 4],
            noise: &[0.1; 4],
            sigma: 4.0,
            size: Size2us::new(2, 2),
            expected: &[true; 4],
        },
        ThresholdMaskCase {
            name: "sigma_5",
            pixels: &[0.5; 4],
            noise: &[0.1; 4],
            sigma: 5.0,
            size: Size2us::new(2, 2),
            expected: &[false; 4],
        },
    ];

    for case in cases {
        let mask =
            create_residual_threshold_mask_test(case.pixels, case.noise, case.sigma, case.size);
        let actual: Vec<bool> = mask.iter().collect();

        assert_eq!(actual.as_slice(), case.expected, "{}: {case:?}", case.name);
    }
}

/// The whole-image entry point maps rows onto the mask's row-aligned words: every pattern, at
/// widths short of, on and past a 64-bit word, and in a single row or column, comes back pixel for
/// pixel. Above-threshold pixels read 1.0 against `3·0.1`, the rest −0.5.
#[test]
fn row_layout_over_sizes_and_patterns() {
    type Pattern = (&'static str, fn(usize, usize, Size2us) -> bool);
    let patterns: [Pattern; 4] = [
        ("checkerboard", |x, y, _| (x + y) % 2 == 0),
        ("horizontal stripes", |_, y, _| y % 2 == 0),
        ("vertical stripes", |x, _, _| x % 2 == 0),
        ("row ends", |x, _, size| x == 0 || x == size.width - 1),
    ];
    let sizes = [
        Size2us::new(10, 10),
        Size2us::new(64, 4),
        Size2us::new(70, 4),
        Size2us::new(128, 3),
        Size2us::new(191, 5),
        Size2us::new(100, 73),
        Size2us::new(1, 10),
        Size2us::new(10, 1),
    ];
    for size in sizes {
        for (name, pattern) in patterns {
            let expected: Vec<bool> = (0..size.height)
                .flat_map(|y| (0..size.width).map(move |x| pattern(x, y, size)))
                .collect();
            let residual: Vec<f32> = expected
                .iter()
                .map(|&on| if on { 1.0 } else { -0.5 })
                .collect();
            let noise = vec![0.1f32; size.pixel_count()];
            let mask = create_residual_threshold_mask_test(&residual, &noise, 3.0, size);
            let actual: Vec<bool> = mask.iter().collect();
            assert_eq!(actual, expected, "{name} at {size:?}");
        }
    }
}
