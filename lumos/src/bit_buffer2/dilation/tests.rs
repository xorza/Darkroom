//! Tests for morphological dilation.

use crate::bit_buffer2::BitBuffer2;
use crate::bit_buffer2::dilation::dilate_mask;
use crate::internals::prelude::*;

/// `mask` dilated by `radius` into `dilated`, through the in-place kernel.
fn dilate_into(mask: &BitBuffer2, radius: usize, dilated: &mut BitBuffer2) {
    dilated.copy_from(mask);
    dilate_mask(dilated, radius, &mut BitBuffer2::new_default(mask.size));
}

/// Verify dilation result against a naive O(n²×r²) disk dilation.
fn assert_naive_dilation(mask: &BitBuffer2, dilated: &BitBuffer2, radius: usize, ctx: &str) {
    let size = Size2us::new(mask.size.width, mask.size.height);
    for y in 0..size.height {
        for x in 0..size.width {
            let mut expected = false;
            for sy in y.saturating_sub(radius)..=(y + radius).min(size.height - 1) {
                for sx in x.saturating_sub(radius)..=(x + radius).min(size.width - 1) {
                    let distance_squared = sx.abs_diff(x).pow(2) + sy.abs_diff(y).pow(2);
                    if distance_squared <= radius * radius && mask.get_at(Vec2us::new(sx, sy)) {
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
    dilate_into(&mask, 1, &mut dilated);
    assert!(dilated.iter().all(|x| !x));
}

#[test]
fn dilate_mask_single_pixel_radius_0() {
    // Radius 0 should not expand
    let mut mask_data = vec![false; 9];
    mask_data[4] = true; // center
    let mask = BitBuffer2::from_slice(Size2us::new(3, 3), &mask_data);
    let mut dilated = BitBuffer2::new_filled(Size2us::new(3, 3), false);
    dilate_into(&mask, 0, &mut dilated);

    assert_eq!(dilated.iter().filter(|x| *x).count(), 1);
    assert!(dilated.get(4));
}

/// One set pixel grows into a disk, clipped at the frame's edge: the pixels within Euclidean
/// distance `radius`. Radius 1 is the 5-pixel cross, not the 3 × 3 square; radius 2 is the 5 × 5
/// square less the 12 pixels at distance √5 and √8, 13 pixels; at the corner, radius 1 keeps the
/// pixel and its two neighbours, and not the diagonal one at √2.
#[test]
fn a_pixel_dilates_into_a_disk() {
    for (size, seed, radius, picture) in [
        (
            5,
            (2, 2),
            1,
            ".....\n\
             ..#..\n\
             .###.\n\
             ..#..\n\
             .....",
        ),
        (
            7,
            (3, 3),
            2,
            ".......\n\
             ...#...\n\
             ..###..\n\
             .#####.\n\
             ..###..\n\
             ...#...\n\
             .......",
        ),
        (
            4,
            (0, 0),
            1,
            "##..\n\
             #...\n\
             ....\n\
             ....",
        ),
    ] {
        let mut mask = BitBuffer2::new_filled(Size2us::new(size, size), false);
        mask.set_at(Vec2us::new(seed.0, seed.1), true);
        let mut dilated = BitBuffer2::new_filled(Size2us::new(size, size), false);
        dilate_into(&mask, radius, &mut dilated);
        for (y, row) in picture.lines().enumerate() {
            for (x, cell) in row.trim().chars().enumerate() {
                assert_eq!(
                    dilated.get_at(Vec2us::new(x, y)),
                    cell == '#',
                    "radius {radius} from {seed:?} at ({x}, {y})"
                );
            }
        }
    }
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
    dilate_into(&mask, 1, &mut dilated);

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
    dilate_into(&mask, 64, &mut dilated);
}

/// Every dilation shape against the brute-force reference, at every pixel.
///
/// The reference rescans a `(2r+1)^2` window per pixel, so it shares no structure with the
/// word-parallel bit implementation under test — an independent oracle, not a second copy of the
/// same logic — and it checks the whole buffer. The explicit-footprint tests above are the
/// arithmetic anchor that validates the reference itself.
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
            dilate_into(&mask, radius, &mut dilated);
            assert_naive_dilation(
                &mask,
                &dilated,
                radius,
                &format!("{} r={radius}", case.name),
            );
        }
    }
}
