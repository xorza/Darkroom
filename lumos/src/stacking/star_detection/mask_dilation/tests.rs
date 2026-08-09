//! Tests for morphological dilation.

// Allow identity operations like `y * width + x` for clarity in 2D indexing
#![allow(clippy::identity_op, clippy::erasing_op)]

use crate::bit_buffer2::BitBuffer2;
use crate::stacking::star_detection::mask_dilation::dilate_mask;
use crate::testing::prelude::*;

/// Verify dilation result against naive O(n²×r²) box dilation.
fn assert_naive_dilation(mask: &BitBuffer2, dilated: &BitBuffer2, radius: usize, ctx: &str) {
    let size = Size2us::new(mask.size.width, mask.size.height);
    for y in 0..size.height {
        for x in 0..size.width {
            let mut expected = false;
            for sy in y.saturating_sub(radius)..=(y + radius).min(size.height - 1) {
                for sx in x.saturating_sub(radius)..=(x + radius).min(size.width - 1) {
                    if mask.get_at(Vec2us::new(sx, sy)) {
                        expected = true;
                        break;
                    }
                }
                if expected {
                    break;
                }
            }
            assert_eq!(
                dilated.get_at(Vec2us::new(x, y)),
                expected,
                "{ctx} mismatch at ({x}, {y})"
            );
        }
    }
}

#[test]
fn dilate_mask_empty() {
    let mask = BitBuffer2::from_slice(Size2us::new(3, 3), &[false; 9]);
    let mut dilated = BitBuffer2::new_filled(Size2us::new(3, 3), false);
    dilate_mask(&mask, 1, &mut dilated);
    assert!(dilated.iter().all(|x| !x));
}

#[test]
fn dilate_mask_single_pixel_radius_0() {
    // Radius 0 should not expand
    let mut mask_data = vec![false; 9];
    mask_data[4] = true; // center
    let mask = BitBuffer2::from_slice(Size2us::new(3, 3), &mask_data);
    let mut dilated = BitBuffer2::new_filled(Size2us::new(3, 3), false);
    dilate_mask(&mask, 0, &mut dilated);

    assert_eq!(dilated.iter().filter(|x| *x).count(), 1);
    assert!(dilated.get(4));
}

#[test]
fn dilate_mask_single_pixel_radius_1() {
    // 3x3 mask with center pixel, radius 1 should create 3x3 square
    let mut mask_data = vec![false; 25]; // 5x5
    mask_data[2 * 5 + 2] = true; // center at (2, 2)
    let mask = BitBuffer2::from_slice(Size2us::new(5, 5), &mask_data);
    let mut dilated = BitBuffer2::new_filled(Size2us::new(5, 5), false);
    dilate_mask(&mask, 1, &mut dilated);

    // Should dilate to 3x3 square centered at (2,2)
    for y in 1..=3 {
        for x in 1..=3 {
            assert!(
                dilated.get(y * 5 + x),
                "Pixel ({}, {}) should be true",
                x,
                y
            );
        }
    }
    // Corners should be false
    assert!(!dilated.get(0 * 5 + 0));
    assert!(!dilated.get(0 * 5 + 4));
    assert!(!dilated.get(4 * 5 + 0));
    assert!(!dilated.get(4 * 5 + 4));
}

#[test]
fn dilate_mask_single_pixel_radius_2() {
    // 7x7 mask with center pixel, radius 2 should create 5x5 square
    let mut mask_data = vec![false; 49];
    mask_data[3 * 7 + 3] = true; // center at (3, 3)
    let mask = BitBuffer2::from_slice(Size2us::new(7, 7), &mask_data);
    let mut dilated = BitBuffer2::new_filled(Size2us::new(7, 7), false);
    dilate_mask(&mask, 2, &mut dilated);

    // Should dilate to 5x5 square centered at (3,3)
    let mut count = 0;
    for y in 1..=5 {
        for x in 1..=5 {
            assert!(
                dilated.get(y * 7 + x),
                "Pixel ({}, {}) should be true",
                x,
                y
            );
            count += 1;
        }
    }
    assert_eq!(count, 25);
}

#[test]
fn dilate_mask_corner_pixel() {
    // Pixel at corner (0,0), dilation should be clipped to image bounds
    let mut mask_data = vec![false; 16];
    mask_data[0] = true;
    let mask = BitBuffer2::from_slice(Size2us::new(4, 4), &mask_data);
    let mut dilated = BitBuffer2::new_filled(Size2us::new(4, 4), false);
    dilate_mask(&mask, 1, &mut dilated);

    // Only 2x2 corner should be dilated
    assert!(dilated.get(0 * 4 + 0));
    assert!(dilated.get(0 * 4 + 1));
    assert!(dilated.get(1 * 4 + 0));
    assert!(dilated.get(1 * 4 + 1));
    // Rest should be false
    assert!(!dilated.get(0 * 4 + 2));
    assert!(!dilated.get(2 * 4 + 0));
}

#[test]
fn dilate_mask_preserves_original_pixels() {
    // Original pixels should always be in dilated result
    let mut mask_data = vec![false; 25];
    mask_data[0] = true;
    mask_data[12] = true; // center
    mask_data[24] = true;
    let mask = BitBuffer2::from_slice(Size2us::new(5, 5), &mask_data);
    let mut dilated = BitBuffer2::new_filled(Size2us::new(5, 5), false);
    dilate_mask(&mask, 1, &mut dilated);

    // All original pixels must be present
    assert!(dilated.get(0));
    assert!(dilated.get(12));
    assert!(dilated.get(24));
}

#[test]
#[should_panic(expected = "radius must be <= 63")]
fn dilate_mask_radius_above_63_panics() {
    // Radius > 63 is out of contract (production caps dilation at 50).
    let mask = BitBuffer2::from_slice(Size2us::new(200, 1), &[false; 200]);
    let mut dilated = BitBuffer2::new_filled(Size2us::new(200, 1), false);
    dilate_mask(&mask, 64, &mut dilated);
}

/// Every dilation shape against the brute-force reference, at every pixel.
///
/// The reference rescans a `(2r+1)^2` window per pixel, so it shares no structure with the
/// word-parallel bit implementation under test — an independent oracle, not a second copy of the
/// same logic. It also checks the whole buffer, where the hand-written cases this replaces
/// spot-checked a few pixels each. The explicit-footprint tests above stay as the arithmetic
/// anchor that validates the reference itself.
#[test]
fn dilation_matches_the_brute_force_reference() {
    /// How a case seeds its mask before dilating.
    enum Seed {
        /// Individual `(x, y)` pixels.
        Pixels(&'static [(usize, usize)]),
        /// Every pixel where `(x + y) % n == 0`; `n = 1` sets all of them.
        Every(usize),
    }

    struct Case {
        name: &'static str,
        size: Size2us,
        seed: Seed,
        radii: &'static [usize],
    }

    // Widths straddle the 64-bit word boundary, because the implementation dilates word-wise.
    let cases = [
        Case {
            name: "single centre",
            size: Size2us::new(9, 9),
            seed: Seed::Pixels(&[(4, 4)]),
            radii: &[0, 1, 2, 3, 8, 20],
        },
        Case {
            name: "corner",
            size: Size2us::new(8, 8),
            seed: Seed::Pixels(&[(0, 0)]),
            radii: &[1, 2, 7],
        },
        Case {
            name: "all corners",
            size: Size2us::new(8, 8),
            seed: Seed::Pixels(&[(0, 0), (7, 0), (0, 7), (7, 7)]),
            radii: &[1, 2, 4],
        },
        Case {
            name: "edge midpoints",
            size: Size2us::new(9, 9),
            seed: Seed::Pixels(&[(4, 0), (0, 4), (8, 4), (4, 8)]),
            radii: &[1, 3],
        },
        Case {
            name: "nearby pair merges",
            size: Size2us::new(10, 4),
            seed: Seed::Pixels(&[(2, 2), (5, 2)]),
            radii: &[1, 2, 3],
        },
        Case {
            name: "width 64 exact word",
            size: Size2us::new(64, 4),
            seed: Seed::Pixels(&[(0, 1), (63, 1), (31, 2)]),
            radii: &[0, 1, 2],
        },
        Case {
            name: "width 65 crosses word",
            size: Size2us::new(65, 4),
            seed: Seed::Pixels(&[(63, 1), (64, 2)]),
            radii: &[0, 1, 2],
        },
        Case {
            name: "width 128 two words",
            size: Size2us::new(128, 3),
            seed: Seed::Pixels(&[(63, 1), (64, 1), (127, 0)]),
            radii: &[1, 2],
        },
        Case {
            name: "wide sparse",
            size: Size2us::new(200, 5),
            seed: Seed::Pixels(&[(0, 0), (99, 2), (199, 4)]),
            radii: &[1, 3, 5],
        },
        Case {
            name: "max radius 63",
            size: Size2us::new(70, 3),
            seed: Seed::Pixels(&[(35, 1)]),
            radii: &[63],
        },
        Case {
            name: "single column",
            size: Size2us::new(1, 10),
            seed: Seed::Pixels(&[(0, 5)]),
            radii: &[0, 1, 3],
        },
        Case {
            name: "single row",
            size: Size2us::new(10, 1),
            seed: Seed::Pixels(&[(5, 0)]),
            radii: &[0, 1, 3],
        },
        Case {
            name: "first and last row",
            size: Size2us::new(12, 6),
            seed: Seed::Pixels(&[(3, 0), (8, 5)]),
            radii: &[1, 2],
        },
        Case {
            name: "vertical word boundary",
            size: Size2us::new(70, 8),
            seed: Seed::Pixels(&[(64, 0), (64, 7)]),
            radii: &[1, 2],
        },
        Case {
            name: "empty",
            size: Size2us::new(16, 16),
            seed: Seed::Pixels(&[]),
            radii: &[0, 1, 5],
        },
        Case {
            name: "all set",
            size: Size2us::new(20, 6),
            seed: Seed::Every(1),
            radii: &[0, 1, 4],
        },
        Case {
            name: "checkerboard",
            size: Size2us::new(16, 16),
            seed: Seed::Every(2),
            radii: &[0, 1, 2],
        },
        Case {
            name: "sparse across words",
            size: Size2us::new(130, 4),
            seed: Seed::Every(7),
            radii: &[1, 3],
        },
    ];

    for case in &cases {
        let mut data = vec![false; case.size.pixel_count()];
        match case.seed {
            Seed::Pixels(pixels) => {
                for &(x, y) in pixels {
                    data[y * case.size.width + x] = true;
                }
            }
            Seed::Every(n) => {
                for y in 0..case.size.height {
                    for x in 0..case.size.width {
                        data[y * case.size.width + x] = (x + y) % n == 0;
                    }
                }
            }
        }
        let mask = BitBuffer2::from_slice(case.size, &data);
        for &radius in case.radii {
            let mut dilated = BitBuffer2::new_filled(case.size, false);
            dilate_mask(&mask, radius, &mut dilated);
            assert_naive_dilation(
                &mask,
                &dilated,
                radius,
                &format!("{} r={radius}", case.name),
            );
        }
    }
}
