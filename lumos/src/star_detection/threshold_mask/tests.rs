//! Tests for threshold mask creation (packed `BitBuffer2` version).

use common::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::star_detection::threshold_mask::threshold_params::ThresholdParams;
use common::USize2;

/// The mask of `filtered` at `sigma` over `noise`, at the shared test floor.
fn threshold_mask(filtered: &Buffer2<f32>, noise: &Buffer2<f32>, sigma: f32) -> BitBuffer2 {
    let mut mask = BitBuffer2::new_filled(filtered.size(), false);
    ThresholdParams::test(sigma).fill_mask_plane(filtered, noise, &mut mask);
    mask
}

#[derive(Debug)]
struct ThresholdMaskCase {
    name: &'static str,
    pixels: &'static [f32],
    noise: &'static [f32],
    sigma: f32,
    size: USize2,
    expected: &'static [bool],
}

impl ThresholdMaskCase {
    /// The case's pixels and noise, as planes of its size, and the mask of the one over the other.
    fn mask(&self) -> BitBuffer2 {
        let plane = |values: &[f32]| Buffer2::new(self.size, values.to_vec());
        threshold_mask(&plane(self.pixels), &plane(self.noise), self.sigma)
    }
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
            size: USize2::new(4, 1),
            expected: &[true; 4],
        },
        ThresholdMaskCase {
            name: "mixed",
            pixels: &[0.2, 0.4, 0.6, 0.8],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: USize2::new(2, 2),
            expected: &[false, true, true, true],
        },
        ThresholdMaskCase {
            name: "variable_noise",
            pixels: &[0.5; 4],
            noise: &[0.1, 0.2, 0.3, 0.05],
            sigma: 3.0,
            size: USize2::new(2, 2),
            expected: &[true, false, false, true],
        },
        ThresholdMaskCase {
            name: "negative_pixels",
            pixels: &[-0.5, 0.5, -0.1, 0.4],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: USize2::new(2, 2),
            expected: &[false, true, false, true],
        },
        ThresholdMaskCase {
            name: "zero_noise_uses_epsilon",
            pixels: &[0.1, -0.1],
            noise: &[0.0; 2],
            sigma: 3.0,
            size: USize2::new(2, 1),
            expected: &[true, false],
        },
        ThresholdMaskCase {
            name: "negative_noise_uses_epsilon",
            pixels: &[0.1, -0.1],
            noise: &[-0.1; 2],
            sigma: 3.0,
            size: USize2::new(2, 1),
            expected: &[true, false],
        },
        ThresholdMaskCase {
            name: "standard_below",
            pixels: &[10.0; 4],
            noise: &[10.0; 4],
            sigma: 3.0,
            size: USize2::new(2, 2),
            expected: &[false; 4],
        },
        ThresholdMaskCase {
            name: "high_noise_region",
            pixels: &[1.0; 2],
            noise: &[0.1, 0.5],
            sigma: 3.0,
            size: USize2::new(2, 1),
            expected: &[true, false],
        },
        ThresholdMaskCase {
            name: "tiny_1x1_above",
            pixels: &[1.0],
            noise: &[0.1],
            sigma: 3.0,
            size: USize2::new(1, 1),
            expected: &[true],
        },
        ThresholdMaskCase {
            name: "tiny_1x1_below",
            pixels: &[0.0],
            noise: &[0.1],
            sigma: 3.0,
            size: USize2::new(1, 1),
            expected: &[false],
        },
        ThresholdMaskCase {
            name: "exact_threshold_is_false",
            pixels: &[0.3, 0.30001],
            noise: &[0.1; 2],
            sigma: 3.0,
            size: USize2::new(2, 1),
            expected: &[false, true],
        },
        ThresholdMaskCase {
            name: "sigma_3",
            pixels: &[0.5; 4],
            noise: &[0.1; 4],
            sigma: 3.0,
            size: USize2::new(2, 2),
            expected: &[true; 4],
        },
        ThresholdMaskCase {
            name: "sigma_4",
            pixels: &[0.5; 4],
            noise: &[0.1; 4],
            sigma: 4.0,
            size: USize2::new(2, 2),
            expected: &[true; 4],
        },
        ThresholdMaskCase {
            name: "sigma_5",
            pixels: &[0.5; 4],
            noise: &[0.1; 4],
            sigma: 5.0,
            size: USize2::new(2, 2),
            expected: &[false; 4],
        },
    ];

    for case in cases {
        let mask = case.mask();
        let actual: Vec<bool> = mask.iter().collect();

        assert_eq!(actual.as_slice(), case.expected, "{}: {case:?}", case.name);
    }
}

/// The whole-image entry point maps rows onto the mask's row-aligned words: every pattern, at
/// widths short of, on and past a 64-bit word, and in a single row or column, comes back pixel for
/// pixel. Above-threshold pixels read 1.0 against `3·0.1`, the rest −0.5.
#[test]
fn row_layout_over_sizes_and_patterns() {
    type Pattern = (&'static str, fn(usize, usize, USize2) -> bool);
    let patterns: [Pattern; 4] = [
        ("checkerboard", |x, y, _| (x + y) % 2 == 0),
        ("horizontal stripes", |_, y, _| y % 2 == 0),
        ("vertical stripes", |x, _, _| x % 2 == 0),
        ("row ends", |x, _, size| x == 0 || x == size.width - 1),
    ];
    let sizes = [
        USize2::new(10, 10),
        USize2::new(64, 4),
        USize2::new(70, 4),
        USize2::new(128, 3),
        USize2::new(191, 5),
        USize2::new(100, 73),
        USize2::new(1, 10),
        USize2::new(10, 1),
    ];
    for size in sizes {
        for (name, pattern) in patterns {
            let expected: Vec<bool> = (0..size.height)
                .flat_map(|y| (0..size.width).map(move |x| pattern(x, y, size)))
                .collect();
            let residual = Buffer2::new(
                size,
                expected
                    .iter()
                    .map(|&on| if on { 1.0 } else { -0.5 })
                    .collect(),
            );
            let noise = Buffer2::new_filled(size, 0.1f32);
            let mask = threshold_mask(&residual, &noise, 3.0);
            let actual: Vec<bool> = mask.iter().collect();
            assert_eq!(actual, expected, "{name} at {size:?}");
        }
    }
}
