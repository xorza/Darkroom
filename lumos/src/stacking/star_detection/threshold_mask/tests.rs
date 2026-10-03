//! Tests for threshold mask creation (packed `BitBuffer2` version).
//!
//! Test organization:
//! - Basic threshold tests: Core functionality for standard thresholding
//! - Edge cases: Boundary conditions, special values, tiny images
//! - SIMD validation: Remainder handling, alignment, SIMD vs scalar consistency
//! - Filtered threshold tests: Background-subtracted image thresholding
//! - Multi-row tests: 2D image patterns and row boundary handling

use crate::bit_buffer2::BitBuffer2;
use crate::stacking::star_detection::threshold_mask::internals::test_params;
use crate::stacking::star_detection::threshold_mask::{
    create_residual_threshold_mask, create_threshold_mask,
};
use crate::testing::prelude::*;

/// Helper to create threshold mask for tests using packed version
fn create_threshold_mask_test(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma: f32,
    size: Size2us,
) -> BitBuffer2 {
    let pixels = Buffer2::new(size.width, size.height, pixels.to_vec());
    let bg = Buffer2::new(size.width, size.height, bg.to_vec());
    let noise = Buffer2::new(size.width, size.height, noise.to_vec());
    let mut mask = BitBuffer2::new_filled(size, false);
    create_threshold_mask(&pixels, &bg, &noise, test_params(sigma), &mut mask);
    mask
}

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
    background: &'static [f32],
    noise: &'static [f32],
    sigma: f32,
    size: Size2us,
    expected: &'static [bool],
}

#[test]
fn threshold_mask_truth_table() {
    let cases = [
        ThresholdMaskCase {
            name: "standard_above",
            pixels: &[100.0; 4],
            background: &[50.0; 4],
            noise: &[10.0; 4],
            sigma: 3.0,
            size: Size2us::new(4, 1),
            expected: &[true; 4],
        },
        ThresholdMaskCase {
            name: "standard_below",
            pixels: &[60.0; 4],
            background: &[50.0; 4],
            noise: &[10.0; 4],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[false; 4],
        },
        ThresholdMaskCase {
            name: "mixed",
            pixels: &[1.0, 2.0, 0.5, 1.5],
            background: &[1.0; 4],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[false, true, false, true],
        },
        ThresholdMaskCase {
            name: "variable_background",
            pixels: &[1.5; 4],
            background: &[1.0, 1.2, 1.4, 0.8],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[true, false, false, true],
        },
        ThresholdMaskCase {
            name: "high_noise_region",
            pixels: &[2.0; 2],
            background: &[1.0; 2],
            noise: &[0.1, 0.5],
            sigma: 3.0,
            size: Size2us::new(2, 1),
            expected: &[true, false],
        },
        ThresholdMaskCase {
            name: "negative_pixels",
            pixels: &[-0.5, 0.5, -1.0, 1.0],
            background: &[0.0; 4],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[false, true, false, true],
        },
        ThresholdMaskCase {
            name: "negative_background",
            pixels: &[0.0; 4],
            background: &[-1.0, -0.5, 0.0, 0.5],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[true, true, false, false],
        },
        ThresholdMaskCase {
            name: "zero_noise_uses_epsilon",
            pixels: &[1.1, 0.9],
            background: &[1.0; 2],
            noise: &[0.0; 2],
            sigma: 3.0,
            size: Size2us::new(2, 1),
            expected: &[true, false],
        },
        ThresholdMaskCase {
            name: "negative_noise_uses_epsilon",
            pixels: &[1.1, 0.9],
            background: &[1.0; 2],
            noise: &[-0.1; 2],
            sigma: 3.0,
            size: Size2us::new(2, 1),
            expected: &[true, false],
        },
        ThresholdMaskCase {
            name: "tiny_1x1_above",
            pixels: &[2.0],
            background: &[1.0],
            noise: &[0.1],
            sigma: 3.0,
            size: Size2us::new(1, 1),
            expected: &[true],
        },
        ThresholdMaskCase {
            name: "tiny_1x1_below",
            pixels: &[1.0],
            background: &[1.0],
            noise: &[0.1],
            sigma: 3.0,
            size: Size2us::new(1, 1),
            expected: &[false],
        },
        ThresholdMaskCase {
            name: "exact_threshold_is_false",
            pixels: &[1.3, 1.30001],
            background: &[1.0; 2],
            noise: &[0.1; 2],
            sigma: 3.0,
            size: Size2us::new(2, 1),
            expected: &[false, true],
        },
        ThresholdMaskCase {
            name: "sigma_3",
            pixels: &[1.5; 4],
            background: &[1.0; 4],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[true; 4],
        },
        ThresholdMaskCase {
            name: "sigma_5",
            pixels: &[1.5; 4],
            background: &[1.0; 4],
            noise: &[0.1; 4],
            sigma: 5.0,
            size: Size2us::new(2, 2),
            expected: &[false; 4],
        },
        ThresholdMaskCase {
            name: "sigma_4",
            pixels: &[1.5; 4],
            background: &[1.0; 4],
            noise: &[0.1; 4],
            sigma: 4.0,
            size: Size2us::new(2, 2),
            expected: &[true; 4],
        },
    ];
    let mut sigma_three = None;
    let mut sigma_five = None;

    for case in cases {
        let mask = create_threshold_mask_test(
            case.pixels,
            case.background,
            case.noise,
            case.sigma,
            case.size,
        );
        let actual: Vec<bool> = mask.iter().collect();

        assert_eq!(actual.as_slice(), case.expected, "{case:?}");

        match case.name {
            "sigma_3" => sigma_three = Some(actual),
            "sigma_5" => sigma_five = Some(actual),
            _ => {}
        }
    }

    assert_ne!(sigma_three.unwrap(), sigma_five.unwrap());
}

#[derive(Debug)]
struct FilteredThresholdMaskCase {
    name: &'static str,
    pixels: &'static [f32],
    noise: &'static [f32],
    sigma: f32,
    size: Size2us,
    expected: &'static [bool],
}

#[test]
fn filtered_threshold_mask_truth_table() {
    let cases = [
        FilteredThresholdMaskCase {
            name: "constant_above",
            pixels: &[50.0; 4],
            noise: &[10.0; 4],
            sigma: 3.0,
            size: Size2us::new(4, 1),
            expected: &[true; 4],
        },
        FilteredThresholdMaskCase {
            name: "mixed",
            pixels: &[0.2, 0.4, 0.6, 0.8],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[false, true, true, true],
        },
        FilteredThresholdMaskCase {
            name: "variable_noise",
            pixels: &[0.5; 4],
            noise: &[0.1, 0.2, 0.3, 0.05],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[true, false, false, true],
        },
        FilteredThresholdMaskCase {
            name: "negative_pixels",
            pixels: &[-0.5, 0.5, -0.1, 0.4],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: Size2us::new(2, 2),
            expected: &[false, true, false, true],
        },
        FilteredThresholdMaskCase {
            name: "zero_noise_uses_epsilon",
            pixels: &[0.1, -0.1],
            noise: &[0.0; 2],
            sigma: 3.0,
            size: Size2us::new(2, 1),
            expected: &[true, false],
        },
    ];

    for case in cases {
        let mask =
            create_residual_threshold_mask_test(case.pixels, case.noise, case.sigma, case.size);
        let actual: Vec<bool> = mask.iter().collect();

        assert_eq!(actual.as_slice(), case.expected, "{}: {case:?}", case.name);
    }
}

/// The whole-image entry points map rows onto the mask's row-aligned words: every pattern, at
/// widths short of, on and past a 64-bit word, and in a single row or column, comes back pixel for
/// pixel. Above-threshold pixels read 2.0 against `1.0 + 3·0.1`, the rest 0.5; the residual mode
/// sees them less the 1.0.
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
            let pixels: Vec<f32> = expected
                .iter()
                .map(|&on| if on { 2.0 } else { 0.5 })
                .collect();
            let bg = vec![1.0f32; size.pixel_count()];
            let noise = vec![0.1f32; size.pixel_count()];
            let residual: Vec<f32> = pixels.iter().map(|&p| p - 1.0).collect();

            let with_bg = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, size);
            let without_bg = create_residual_threshold_mask_test(&residual, &noise, 3.0, size);
            for (mode, mask) in [("with bg", with_bg), ("residual", without_bg)] {
                let actual: Vec<bool> = mask.iter().collect();
                assert_eq!(actual, expected, "{name} at {size:?}, {mode}");
            }
        }
    }
}
