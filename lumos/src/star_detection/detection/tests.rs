//! Tests for star detection.

// Allow identity operations like `y * width + x` for clarity in 2D indexing
#![allow(clippy::identity_op, clippy::erasing_op)]

use super::*;
use crate::common::{BitBuffer2, Buffer2};
use crate::star_detection::background::{BackgroundConfig, BackgroundMap};
use crate::testing::synthetic::background_map;

/// Default deblend config for tests
const TEST_DEBLEND_CONFIG: DeblendConfig = DeblendConfig {
    min_separation: 3,
    min_prominence: 0.3,
    n_thresholds: 0,
    min_contrast: 0.005,
    max_area: 10000, // large enough to not filter anything in small test images
};

fn make_test_image_with_star(
    width: usize,
    height: usize,
    star_x: usize,
    star_y: usize,
) -> Buffer2<f32> {
    let mut pixels = vec![0.1f32; width * height];

    // Add a Gaussian-like star
    for dy in -3i32..=3 {
        for dx in -3i32..=3 {
            let x = star_x as i32 + dx;
            let y = star_y as i32 + dy;
            if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                let dist_sq = (dx * dx + dy * dy) as f32;
                let value = 0.1 + 0.8 * (-dist_sq / 4.0).exp();
                pixels[y as usize * width + x as usize] = value;
            }
        }
    }

    Buffer2::new(width, height, pixels)
}

// =============================================================================
// High-Level Detection Tests (detect_stars function)
// =============================================================================

mod detect_stars_tests {
    use super::*;

    #[test]
    fn single_star() {
        let width = 64;
        let height = 64;
        let pixels = make_test_image_with_star(width, height, 32, 32);

        let bg = BackgroundMap::new(
            &pixels,
            &BackgroundConfig {
                tile_size: 32,
                ..Default::default()
            },
        );
        let config = StarDetectionConfig::default();
        let candidates = detect_stars(&pixels, None, &bg, &config);

        assert_eq!(candidates.len(), 1, "Should detect exactly one star");
        let star = &candidates[0];
        assert!(
            star.peak_x >= 30 && star.peak_x <= 34,
            "Peak X should be near 32"
        );
        assert!(
            star.peak_y >= 30 && star.peak_y <= 34,
            "Peak Y should be near 32"
        );
    }

    #[test]
    fn multiple_stars() {
        let width = 100;
        let height = 100;
        let mut pixels = vec![0.1f32; width * height];

        // Add three stars
        for (sx, sy) in [(25i32, 25i32), (50, 50), (75, 75)] {
            for dy in -3i32..=3 {
                for dx in -3i32..=3 {
                    let x = sx + dx;
                    let y = sy + dy;
                    if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                        let dist_sq = (dx * dx + dy * dy) as f32;
                        let value = 0.1 + 0.8 * (-dist_sq / 4.0).exp();
                        pixels[y as usize * width + x as usize] = value;
                    }
                }
            }
        }

        let pixels_buf = Buffer2::new(width, height, pixels);
        let bg = BackgroundMap::new(
            &pixels_buf,
            &BackgroundConfig {
                tile_size: 32,
                ..Default::default()
            },
        );
        let config = StarDetectionConfig {
            edge_margin: 5,
            ..Default::default()
        };
        let candidates = detect_stars(&pixels_buf, None, &bg, &config);

        assert_eq!(candidates.len(), 3, "Should detect three stars");
    }

    #[test]
    fn reject_edge_stars() {
        let width = 64;
        let height = 64;
        // Star at edge (x=5, y=32) should be rejected with edge_margin=10
        let pixels = make_test_image_with_star(width, height, 5, 32);

        let bg = BackgroundMap::new(
            &pixels,
            &BackgroundConfig {
                tile_size: 32,
                ..Default::default()
            },
        );
        let config = StarDetectionConfig {
            edge_margin: 10,
            ..Default::default()
        };
        let candidates = detect_stars(&pixels, None, &bg, &config);

        assert!(candidates.is_empty(), "Edge star should be rejected");
    }

    #[test]
    fn reject_small_objects() {
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.1f32; width * height];

        // Single bright pixel - after dilation (radius 2), becomes 25 pixels (5x5).
        // Use min_area > 25 to reject single-pixel noise.
        pixels[32 * width + 32] = 0.9;

        let pixels_buf = Buffer2::new(width, height, pixels);
        let bg = BackgroundMap::new(
            &pixels_buf,
            &BackgroundConfig {
                tile_size: 32,
                ..Default::default()
            },
        );
        let config = StarDetectionConfig {
            min_area: 26, // Must be > 25 to reject dilated single pixel (radius 2 = 5x5 = 25)
            ..Default::default()
        };
        let candidates = detect_stars(&pixels_buf, None, &bg, &config);

        assert!(candidates.is_empty(), "Single pixel should be rejected");
    }

    #[test]
    fn empty_image() {
        let width = 64;
        let height = 64;
        let pixels = vec![0.1f32; width * height];

        let pixels_buf = Buffer2::new(width, height, pixels);
        let bg = BackgroundMap::new(
            &pixels_buf,
            &BackgroundConfig {
                tile_size: 32,
                ..Default::default()
            },
        );
        let config = StarDetectionConfig::default();
        let candidates = detect_stars(&pixels_buf, None, &bg, &config);

        assert!(candidates.is_empty(), "Uniform image should have no stars");
    }
}

// =============================================================================
// Connected Components Tests (LabelMap::from_mask)
// =============================================================================

mod label_map_tests {
    use super::*;

    #[test]
    fn empty_mask() {
        let mask = BitBuffer2::from_slice(4, 4, &[false; 16]);
        let label_map = LabelMap::from_mask(&mask);

        assert_eq!(label_map.num_labels(), 0);
        assert!(label_map.labels().iter().all(|&l| l == 0));
    }

    #[test]
    fn single_pixel() {
        // 4x4 mask with single pixel at (1, 1)
        let mut mask_data = vec![false; 16];
        mask_data[1 * 4 + 1] = true;
        let mask = BitBuffer2::from_slice(4, 4, &mask_data);

        let label_map = LabelMap::from_mask(&mask);

        assert_eq!(label_map.num_labels(), 1);
        assert_eq!(label_map[1 * 4 + 1], 1);
        assert_eq!(label_map.labels().iter().filter(|&&l| l == 1).count(), 1);
    }

    #[test]
    fn horizontal_line() {
        // 5x3 mask with horizontal line in middle row
        // .....
        // ###..
        // .....
        let mut mask_data = vec![false; 15];
        mask_data[1 * 5 + 0] = true;
        mask_data[1 * 5 + 1] = true;
        mask_data[1 * 5 + 2] = true;
        let mask = BitBuffer2::from_slice(5, 3, &mask_data);

        let label_map = LabelMap::from_mask(&mask);

        assert_eq!(label_map.num_labels(), 1);
        // All three pixels should have the same label
        let label = label_map[1 * 5 + 0];
        assert!(label > 0);
        assert_eq!(label_map[1 * 5 + 1], label);
        assert_eq!(label_map[1 * 5 + 2], label);
    }

    #[test]
    fn vertical_line() {
        // 3x5 mask with vertical line in middle column
        let mut mask_data = vec![false; 15];
        mask_data[0 * 3 + 1] = true;
        mask_data[1 * 3 + 1] = true;
        mask_data[2 * 3 + 1] = true;
        mask_data[3 * 3 + 1] = true;
        let mask = BitBuffer2::from_slice(3, 5, &mask_data);

        let label_map = LabelMap::from_mask(&mask);

        assert_eq!(label_map.num_labels(), 1);
        let label = label_map[0 * 3 + 1];
        assert!(label > 0);
        assert_eq!(label_map[1 * 3 + 1], label);
        assert_eq!(label_map[2 * 3 + 1], label);
        assert_eq!(label_map[3 * 3 + 1], label);
    }

    #[test]
    fn two_separate_regions() {
        // 6x3 mask with two separate single pixels
        // #....#
        // ......
        // ......
        let mut mask_data = vec![false; 18];
        mask_data[0] = true; // (0, 0)
        mask_data[5] = true; // (5, 0)
        let mask = BitBuffer2::from_slice(6, 3, &mask_data);

        let label_map = LabelMap::from_mask(&mask);

        assert_eq!(label_map.num_labels(), 2);
        assert!(label_map[0] > 0);
        assert!(label_map[5] > 0);
        assert_ne!(label_map[0], label_map[5]);
    }

    #[test]
    fn l_shape() {
        // 4x4 mask with L-shape
        // #...
        // #...
        // ##..
        // ....
        let mut mask_data = vec![false; 16];
        mask_data[0 * 4 + 0] = true;
        mask_data[1 * 4 + 0] = true;
        mask_data[2 * 4 + 0] = true;
        mask_data[2 * 4 + 1] = true;
        let mask = BitBuffer2::from_slice(4, 4, &mask_data);

        let label_map = LabelMap::from_mask(&mask);

        assert_eq!(label_map.num_labels(), 1);
        let label = label_map[0];
        assert!(label > 0);
        assert_eq!(label_map[1 * 4 + 0], label);
        assert_eq!(label_map[2 * 4 + 0], label);
        assert_eq!(label_map[2 * 4 + 1], label);
    }

    #[test]
    fn diagonal_not_connected() {
        // 3x3 mask with diagonal pixels (4-connectivity means they're separate)
        // #..
        // .#.
        // ..#
        let mut mask_data = vec![false; 9];
        mask_data[0 * 3 + 0] = true;
        mask_data[1 * 3 + 1] = true;
        mask_data[2 * 3 + 2] = true;
        let mask = BitBuffer2::from_slice(3, 3, &mask_data);

        let label_map = LabelMap::from_mask(&mask);

        // With 4-connectivity, diagonal pixels are NOT connected
        assert_eq!(label_map.num_labels(), 3);
    }

    #[test]
    fn u_shape_union_find() {
        // This tests the union-find when labels need to be merged
        // 5x3 mask forming a U shape:
        // #...#
        // #...#
        // #####
        let mut mask_data = vec![false; 15];
        // Left column
        mask_data[0 * 5 + 0] = true;
        mask_data[1 * 5 + 0] = true;
        mask_data[2 * 5 + 0] = true;
        // Right column
        mask_data[0 * 5 + 4] = true;
        mask_data[1 * 5 + 4] = true;
        mask_data[2 * 5 + 4] = true;
        // Bottom row connecting them
        mask_data[2 * 5 + 1] = true;
        mask_data[2 * 5 + 2] = true;
        mask_data[2 * 5 + 3] = true;
        let mask = BitBuffer2::from_slice(5, 3, &mask_data);

        let label_map = LabelMap::from_mask(&mask);

        // All pixels should be in one component due to union-find
        assert_eq!(label_map.num_labels(), 1);
        let label = label_map[0];
        assert!(label > 0);
        for i in 0..15 {
            if mask_data[i] {
                assert_eq!(
                    label_map[i], label,
                    "All U-shape pixels should have same label"
                );
            }
        }
    }

    #[test]
    fn checkerboard() {
        // 4x4 checkerboard pattern - no pixels should be connected
        // #.#.
        // .#.#
        // #.#.
        // .#.#
        let mut mask_data = vec![false; 16];
        for y in 0..4 {
            for x in 0..4 {
                if (x + y) % 2 == 0 {
                    mask_data[y * 4 + x] = true;
                }
            }
        }
        let mask = BitBuffer2::from_slice(4, 4, &mask_data);

        let label_map = LabelMap::from_mask(&mask);

        // Each pixel is isolated (4-connectivity)
        assert_eq!(label_map.num_labels(), 8);
    }

    #[test]
    fn filled_rectangle() {
        // 3x3 all true
        let mask = BitBuffer2::from_slice(3, 3, &[true; 9]);
        let label_map = LabelMap::from_mask(&mask);

        assert_eq!(label_map.num_labels(), 1);
        assert!(label_map.labels().iter().all(|&l| l == 1));
    }

    #[test]
    fn labels_are_sequential() {
        // 6x1 with three separate pixels
        // #.#.#.
        let mut mask_data = vec![false; 6];
        mask_data[0] = true;
        mask_data[2] = true;
        mask_data[4] = true;
        let mask = BitBuffer2::from_slice(6, 1, &mask_data);

        let label_map = LabelMap::from_mask(&mask);

        assert_eq!(label_map.num_labels(), 3);
        // Labels should be 1, 2, 3 (sequential)
        assert_eq!(label_map[0], 1);
        assert_eq!(label_map[2], 2);
        assert_eq!(label_map[4], 3);
    }

    #[test]
    fn zero_dimensions() {
        // Zero width
        let mask = BitBuffer2::from_slice(0, 10, &[]);
        let label_map = LabelMap::from_mask(&mask);
        assert_eq!(label_map.num_labels(), 0);

        // Zero height
        let mask = BitBuffer2::from_slice(10, 0, &[]);
        let label_map = LabelMap::from_mask(&mask);
        assert_eq!(label_map.num_labels(), 0);
    }

    #[test]
    fn edge_touching_components() {
        // Components touching all four edges
        let width = 10;
        let height = 10;
        let mut mask_data = vec![false; width * height];

        // Top edge component
        mask_data[0] = true;
        mask_data[1] = true;
        // Bottom edge component
        mask_data[9 * width + 8] = true;
        mask_data[9 * width + 9] = true;
        // Left edge component
        mask_data[5 * width] = true;
        // Right edge component
        mask_data[3 * width + 9] = true;

        let mask = BitBuffer2::from_slice(width, height, &mask_data);
        let label_map = LabelMap::from_mask(&mask);

        assert_eq!(label_map.num_labels(), 4);
    }

    mod word_boundary {
        use super::*;

        #[test]
        fn boundary_64() {
            // Test component crossing 64-pixel word boundary
            let width = 70;
            let height = 3;
            let mut mask_data = vec![false; width * height];

            // Horizontal line crossing word boundary at x=63-66
            for x in 62..67 {
                mask_data[1 * width + x] = true;
            }
            let mask = BitBuffer2::from_slice(width, height, &mask_data);

            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), 1);
            let label = label_map[1 * width + 62];
            for x in 62..67 {
                assert_eq!(
                    label_map[1 * width + x],
                    label,
                    "Pixel at x={} should have same label",
                    x
                );
            }
        }

        #[test]
        fn boundary_128() {
            // Test component crossing second word boundary at x=128
            let width = 140;
            let height = 3;
            let mut mask_data = vec![false; width * height];

            // Horizontal line crossing word boundary at x=126-130
            for x in 126..131 {
                mask_data[1 * width + x] = true;
            }
            let mask = BitBuffer2::from_slice(width, height, &mask_data);

            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), 1);
            let label = label_map[1 * width + 126];
            for x in 126..131 {
                assert_eq!(
                    label_map[1 * width + x],
                    label,
                    "Pixel at x={} should have same label",
                    x
                );
            }
        }
    }

    mod parallel {
        use super::*;

        #[test]
        fn large_image_parallel_path() {
            // Large image to trigger parallel code path (>100k pixels)
            let width = 400;
            let height = 300;
            let mut mask_data = vec![false; width * height];

            // Create several separate components
            // Component 1: top-left corner
            for y in 10..20 {
                for x in 10..20 {
                    mask_data[y * width + x] = true;
                }
            }
            // Component 2: bottom-right corner
            for y in 250..260 {
                for x in 350..360 {
                    mask_data[y * width + x] = true;
                }
            }
            // Component 3: center
            for y in 145..155 {
                for x in 195..205 {
                    mask_data[y * width + x] = true;
                }
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), 3);

            // Verify each component has consistent labels
            let label1 = label_map[10 * width + 10];
            let label2 = label_map[250 * width + 350];
            let label3 = label_map[145 * width + 195];

            assert!(label1 > 0);
            assert!(label2 > 0);
            assert!(label3 > 0);
            assert_ne!(label1, label2);
            assert_ne!(label1, label3);
            assert_ne!(label2, label3);

            // Check all pixels in component 1
            for y in 10..20 {
                for x in 10..20 {
                    assert_eq!(label_map[y * width + x], label1);
                }
            }
        }

        #[test]
        fn strip_boundary_vertical_line() {
            // Vertical line spanning multiple strips (tests boundary merging)
            let width = 400;
            let height = 300;
            let mut mask_data = vec![false; width * height];

            // Vertical line from y=0 to y=299 at x=200
            for y in 0..height {
                mask_data[y * width + 200] = true;
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            // Should be single component despite crossing strip boundaries
            assert_eq!(label_map.num_labels(), 1);

            let label = label_map[200]; // First pixel
            for y in 0..height {
                assert_eq!(
                    label_map[y * width + 200],
                    label,
                    "Pixel at y={} should have same label",
                    y
                );
            }
        }

        #[test]
        fn strip_boundary_diagonal() {
            // Diagonal line that crosses strip boundaries but should NOT connect
            // (4-connectivity means diagonal pixels are separate)
            let width = 400;
            let height = 300;
            let mut mask_data = vec![false; width * height];

            // Diagonal from (0,0) to (299,299) - one pixel per row
            for i in 0..height.min(width) {
                mask_data[i * width + i] = true;
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            // Each diagonal pixel should be separate (4-connectivity)
            assert_eq!(label_map.num_labels(), height.min(width));
        }

        #[test]
        fn strip_boundary_u_shape_large() {
            // Large U-shape that spans strip boundaries and requires merging
            let width = 400;
            let height = 300;
            let mut mask_data = vec![false; width * height];

            // Left vertical bar from y=50 to y=250
            for y in 50..250 {
                mask_data[y * width + 100] = true;
            }
            // Right vertical bar from y=50 to y=250
            for y in 50..250 {
                mask_data[y * width + 300] = true;
            }
            // Bottom horizontal bar connecting them at y=249
            for x in 100..=300 {
                mask_data[249 * width + x] = true;
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            // All should be one component
            assert_eq!(label_map.num_labels(), 1);

            let label = label_map[50 * width + 100];
            // Check some pixels from each part
            assert_eq!(label_map[100 * width + 100], label); // left bar
            assert_eq!(label_map[100 * width + 300], label); // right bar
            assert_eq!(label_map[249 * width + 200], label); // bottom bar
        }

        #[test]
        fn many_small_components() {
            // Many small isolated components (stress test for label allocation)
            let width = 400;
            let height = 300;
            let mut mask_data = vec![false; width * height];

            // Create grid of single pixels, spaced 10 apart
            let mut expected_count = 0;
            for y in (5..height).step_by(10) {
                for x in (5..width).step_by(10) {
                    mask_data[y * width + x] = true;
                    expected_count += 1;
                }
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), expected_count);
        }

        #[test]
        fn single_row_large() {
            // Single row, wide image (edge case for strip division)
            let width = 500;
            let height = 1;
            let mut mask_data = vec![false; width * height];

            // Three separate components
            mask_data[10..20].fill(true);
            mask_data[100..110].fill(true);
            mask_data[400..410].fill(true);

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), 3);
        }

        #[test]
        fn single_column_large() {
            // Single column, tall image
            let width = 1;
            let height = 500;
            let mut mask_data = vec![false; width * height];

            // Three separate components
            mask_data[10..20].fill(true);
            mask_data[100..110].fill(true);
            mask_data[400..410].fill(true);

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), 3);
        }

        #[test]
        fn component_at_strip_boundary_exact() {
            // Component exactly at strip boundary (64 rows per strip)
            let width = 400;
            let height = 300;
            let mut mask_data = vec![false; width * height];

            // Small square crossing y=64 boundary (rows 62-66)
            for y in 62..67 {
                for x in 100..105 {
                    mask_data[y * width + x] = true;
                }
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), 1);

            let label = label_map[62 * width + 100];
            for y in 62..67 {
                for x in 100..105 {
                    assert_eq!(label_map[y * width + x], label);
                }
            }
        }

        #[test]
        fn all_pixels_set_large() {
            // Large fully-filled image (worst case for union-find)
            let width = 200;
            let height = 200;
            let mask_data = vec![true; width * height];

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), 1);
            assert!(label_map.labels().iter().all(|&l| l == 1));
        }

        #[test]
        fn compare_sequential_parallel() {
            // Compare results between sequential and parallel paths
            // by testing same pattern at different sizes
            let pattern_test = |width: usize, height: usize| {
                let mut mask_data = vec![false; width * height];

                // Create a pattern: horizontal lines every 10 rows
                for y in (5..height).step_by(10) {
                    for x in 10..(width - 10) {
                        mask_data[y * width + x] = true;
                    }
                }

                let mask = BitBuffer2::from_slice(width, height, &mask_data);
                let label_map = LabelMap::from_mask(&mask);

                // Count expected: one component per line
                let expected_lines = (height - 5) / 10 + if (height - 5) % 10 >= 1 { 1 } else { 0 };
                (label_map.num_labels(), expected_lines)
            };

            // Small (sequential path)
            let (small_labels, small_expected) = pattern_test(100, 100);
            // Large (parallel path)
            let (large_labels, large_expected) = pattern_test(400, 300);

            assert_eq!(small_labels, small_expected);
            assert_eq!(large_labels, large_expected);
        }

        #[test]
        fn alternating_rows_large() {
            // Alternating rows pattern that stresses strip boundary merging
            let width = 400;
            let height = 300;
            let mut mask_data = vec![false; width * height];

            // Fill every other row completely
            for y in (0..height).step_by(2) {
                for x in 0..width {
                    mask_data[y * width + x] = true;
                }
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            // Each row is a separate component (no vertical connectivity)
            let expected = height.div_ceil(2);
            assert_eq!(label_map.num_labels(), expected);
        }

        #[test]
        fn sparse_large() {
            // Very sparse: few pixels spread across large image
            let width = 1000;
            let height = 1000;
            let mut mask_data = vec![false; width * height];

            // Only 4 isolated pixels in corners
            mask_data[10 * width + 10] = true;
            mask_data[10 * width + 990] = true;
            mask_data[990 * width + 10] = true;
            mask_data[990 * width + 990] = true;

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), 4);
        }
    }

    /// Tests specific to RLE-based implementation
    mod rle_specific {
        use super::*;

        #[test]
        fn long_horizontal_run() {
            // Long horizontal run that spans multiple 64-bit words
            let width = 200;
            let height = 5;
            let mut mask_data = vec![false; width * height];

            // Run from x=10 to x=190 (180 pixels, spans 3 words)
            for x in 10..190 {
                mask_data[2 * width + x] = true;
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), 1);
            let label = label_map[2 * width + 10];
            for x in 10..190 {
                assert_eq!(
                    label_map[2 * width + x],
                    label,
                    "All pixels in run should have same label"
                );
            }
        }

        #[test]
        fn multiple_runs_per_row() {
            // Multiple separate runs in a single row
            let width = 100;
            let height = 3;
            let mut mask_data = vec![false; width * height];

            // Row 1: three separate runs
            // Run 1: x=5-15
            for x in 5..15 {
                mask_data[1 * width + x] = true;
            }
            // Run 2: x=30-40
            for x in 30..40 {
                mask_data[1 * width + x] = true;
            }
            // Run 3: x=60-70
            for x in 60..70 {
                mask_data[1 * width + x] = true;
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), 3);

            // Each run should have distinct labels
            let label1 = label_map[1 * width + 5];
            let label2 = label_map[1 * width + 30];
            let label3 = label_map[1 * width + 60];

            assert!(label1 > 0);
            assert!(label2 > 0);
            assert!(label3 > 0);
            assert_ne!(label1, label2);
            assert_ne!(label1, label3);
            assert_ne!(label2, label3);
        }

        #[test]
        fn runs_merging_vertically() {
            // Two runs in adjacent rows that overlap
            let width = 50;
            let height = 4;
            let mut mask_data = vec![false; width * height];

            // Row 1: x=10-30
            for x in 10..30 {
                mask_data[1 * width + x] = true;
            }
            // Row 2: x=20-40 (overlaps with row 1 at x=20-30)
            for x in 20..40 {
                mask_data[2 * width + x] = true;
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            // Should be single component due to vertical overlap
            assert_eq!(label_map.num_labels(), 1);

            let label = label_map[1 * width + 10];
            for x in 10..30 {
                assert_eq!(label_map[1 * width + x], label);
            }
            for x in 20..40 {
                assert_eq!(label_map[2 * width + x], label);
            }
        }

        #[test]
        fn runs_not_merging_no_overlap() {
            // Two runs in adjacent rows that don't overlap
            let width = 50;
            let height = 4;
            let mut mask_data = vec![false; width * height];

            // Row 1: x=5-15
            for x in 5..15 {
                mask_data[1 * width + x] = true;
            }
            // Row 2: x=25-35 (no overlap with row 1)
            for x in 25..35 {
                mask_data[2 * width + x] = true;
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            // Should be two separate components
            assert_eq!(label_map.num_labels(), 2);
            assert_ne!(label_map[1 * width + 5], label_map[2 * width + 25]);
        }

        #[test]
        fn complex_run_pattern() {
            // Complex pattern with multiple runs merging via intermediate rows
            // Row 0: ###........###
            // Row 1: ..###..###....
            // Row 2: ....####......
            let width = 14;
            let height = 3;
            let mut mask_data = vec![false; width * height];

            // Row 0
            for x in 0..3 {
                mask_data[0 * width + x] = true;
            }
            for x in 11..14 {
                mask_data[0 * width + x] = true;
            }
            // Row 1
            for x in 2..5 {
                mask_data[1 * width + x] = true;
            }
            for x in 7..10 {
                mask_data[1 * width + x] = true;
            }
            // Row 2
            for x in 4..8 {
                mask_data[2 * width + x] = true;
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            // Row 0 left connects to row 1 left (overlap at x=2)
            // Row 1 left connects to row 2 (overlap at x=4)
            // Row 1 right connects to row 2 (overlap at x=7)
            // Row 0 right is isolated (no overlap with row 1)
            assert_eq!(label_map.num_labels(), 2);

            // Check the connected component
            let connected_label = label_map[0 * width + 0];
            assert_eq!(label_map[1 * width + 2], connected_label);
            assert_eq!(label_map[2 * width + 4], connected_label);
            assert_eq!(label_map[1 * width + 7], connected_label);

            // Check the isolated component
            let isolated_label = label_map[0 * width + 11];
            assert_ne!(isolated_label, connected_label);
        }

        #[test]
        fn all_ones_word() {
            // Test word that is all 1s (0xFFFFFFFFFFFFFFFF)
            // RLE should create one long run for this
            let width = 128; // 2 full words
            let height = 3;
            let mut mask_data = vec![false; width * height];

            // Fill entire middle row
            for x in 0..width {
                mask_data[1 * width + x] = true;
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), 1);
            let label = label_map[1 * width + 0];
            for x in 0..width {
                assert_eq!(label_map[1 * width + x], label);
            }
        }

        #[test]
        fn alternating_bits_in_word() {
            // Pattern: 101010... which creates many tiny runs
            let width = 64;
            let height = 3;
            let mut mask_data = vec![false; width * height];

            // Alternating pattern in middle row
            for x in (0..width).step_by(2) {
                mask_data[1 * width + x] = true;
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            // Each pixel is isolated (4-connectivity)
            assert_eq!(label_map.num_labels(), 32);
        }

        #[test]
        fn strip_boundary_run_overlap() {
            // Test that runs at strip boundaries merge correctly
            // This specifically tests the RLE boundary merging logic
            let width = 400;
            let height = 300;
            let mut mask_data = vec![false; width * height];

            // Create a wide horizontal band that spans strip boundaries
            // With 64-row strips, boundaries are at y=64, 128, 192, etc.
            for y in 60..70 {
                // Spans boundary at y=64
                for x in 100..200 {
                    mask_data[y * width + x] = true;
                }
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            assert_eq!(label_map.num_labels(), 1);

            let label = label_map[60 * width + 100];
            for y in 60..70 {
                for x in 100..200 {
                    assert_eq!(
                        label_map[y * width + x],
                        label,
                        "All pixels should have same label at ({}, {})",
                        x,
                        y
                    );
                }
            }
        }

        #[test]
        fn empty_rows_between_runs() {
            // Test runs separated by empty rows
            let width = 50;
            let height = 10;
            let mut mask_data = vec![false; width * height];

            // Run at row 2
            for x in 10..20 {
                mask_data[2 * width + x] = true;
            }
            // Run at row 7 (same x range but separated by empty rows)
            for x in 10..20 {
                mask_data[7 * width + x] = true;
            }

            let mask = BitBuffer2::from_slice(width, height, &mask_data);
            let label_map = LabelMap::from_mask(&mask);

            // Should be two separate components
            assert_eq!(label_map.num_labels(), 2);
        }
    }
}

// =============================================================================
// Extract Candidates Tests
// =============================================================================

mod extract_candidates_tests {
    use super::*;

    #[test]
    fn empty() {
        let pixels = Buffer2::new(3, 3, vec![0.5; 9]);
        let label_map = LabelMap::from_raw(Buffer2::new(3, 3, vec![0u32; 9]), 0);

        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert!(candidates.is_empty());
    }

    #[test]
    fn single_component() {
        // 3x3 with single component covering center 3 pixels horizontally
        let pixels = Buffer2::new(
            3,
            3,
            vec![
                0.1, 0.1, 0.1, //
                0.5, 0.9, 0.6, // <- component here
                0.1, 0.1, 0.1,
            ],
        );
        let label_map = LabelMap::from_raw(
            Buffer2::new(
                3,
                3,
                vec![
                    0, 0, 0, //
                    1, 1, 1, //
                    0, 0, 0,
                ],
            ),
            1,
        );

        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        assert_eq!(c.area, 3);
        assert_eq!(c.bbox.min.x, 0);
        assert_eq!(c.bbox.max.x, 2);
        assert_eq!(c.bbox.min.y, 1);
        assert_eq!(c.bbox.max.y, 1);
        assert_eq!(c.peak_x, 1);
        assert_eq!(c.peak_y, 1);
        assert!((c.peak_value - 0.9).abs() < 1e-6);
    }

    #[test]
    fn two_components() {
        // 5x3 with two separate components
        let pixels = Buffer2::new(
            5,
            3,
            vec![
                0.8, 0.1, 0.1, 0.1, 0.7, //
                0.1, 0.1, 0.1, 0.1, 0.1, //
                0.1, 0.1, 0.1, 0.1, 0.1,
            ],
        );
        let label_map = LabelMap::from_raw(
            Buffer2::new(
                5,
                3,
                vec![
                    1, 0, 0, 0, 2, //
                    0, 0, 0, 0, 0, //
                    0, 0, 0, 0, 0,
                ],
            ),
            2,
        );

        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(candidates.len(), 2);

        // First component
        assert_eq!(candidates[0].area, 1);
        assert_eq!(candidates[0].peak_x, 0);
        assert_eq!(candidates[0].peak_y, 0);
        assert!((candidates[0].peak_value - 0.8).abs() < 1e-6);

        // Second component
        assert_eq!(candidates[1].area, 1);
        assert_eq!(candidates[1].peak_x, 4);
        assert_eq!(candidates[1].peak_y, 0);
        assert!((candidates[1].peak_value - 0.7).abs() < 1e-6);
    }

    #[test]
    fn bounding_box() {
        // 5x5 with L-shaped component
        let mut pixels_data = vec![0.1; 25];
        let mut labels_data = vec![0u32; 25];

        // L-shape: (0,0), (0,1), (0,2), (1,2)
        labels_data[0 * 5 + 0] = 1;
        pixels_data[0 * 5 + 0] = 0.5;
        labels_data[1 * 5 + 0] = 1;
        pixels_data[1 * 5 + 0] = 0.6;
        labels_data[2 * 5 + 0] = 1;
        pixels_data[2 * 5 + 0] = 0.9; // peak
        labels_data[2 * 5 + 1] = 1;
        pixels_data[2 * 5 + 1] = 0.7;

        let pixels = Buffer2::new(5, 5, pixels_data);
        let label_map = LabelMap::from_raw(Buffer2::new(5, 5, labels_data), 1);
        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        assert_eq!(c.area, 4);
        assert_eq!(c.bbox.min.x, 0);
        assert_eq!(c.bbox.max.x, 1);
        assert_eq!(c.bbox.min.y, 0);
        assert_eq!(c.bbox.max.y, 2);
        assert_eq!(c.peak_x, 0);
        assert_eq!(c.peak_y, 2);
        assert!((c.peak_value - 0.9).abs() < 1e-6);
    }

    #[test]
    fn width_height() {
        let pixels = Buffer2::new(3, 2, vec![0.5; 6]);
        // 3x2 component covering full image
        let label_map = LabelMap::from_raw(Buffer2::new(3, 2, vec![1u32; 6]), 1);

        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        assert_eq!(c.width(), 3);
        assert_eq!(c.height(), 2);
    }

    #[test]
    fn max_area_filter() {
        // Test that components exceeding max_area are skipped early
        let pixels = Buffer2::new(10, 10, vec![0.5; 100]); // 10x10 image
        let mut labels_data = vec![0u32; 100];

        // Create a component with 50 pixels (labels 1)
        for label in labels_data.iter_mut().take(50) {
            *label = 1;
        }
        // Create a small component with 5 pixels (labels 2)
        for label in labels_data.iter_mut().take(55).skip(50) {
            *label = 2;
        }

        let label_map = LabelMap::from_raw(Buffer2::new(10, 10, labels_data), 2);

        // With max_area=10, the large component should be skipped
        let config_small = DeblendConfig {
            max_area: 10,
            ..TEST_DEBLEND_CONFIG
        };
        let candidates = extract_candidates(&pixels, &label_map, &config_small);
        assert_eq!(candidates.len(), 1, "Only small component should be found");
        assert_eq!(candidates[0].area, 5);

        // With max_area=100, both should be found
        let config_large = DeblendConfig {
            max_area: 100,
            ..TEST_DEBLEND_CONFIG
        };
        let candidates = extract_candidates(&pixels, &label_map, &config_large);
        assert_eq!(candidates.len(), 2, "Both components should be found");
    }

    #[test]
    fn multiple_peaks_same_value() {
        // When multiple pixels have the same peak value, one of them is selected as peak
        let pixels = Buffer2::new(
            3,
            3,
            vec![
                0.1, 0.1, 0.1, //
                0.9, 0.9, 0.9, // Three pixels with same peak value
                0.1, 0.1, 0.1,
            ],
        );
        let label_map = LabelMap::from_raw(
            Buffer2::new(
                3,
                3,
                vec![
                    0, 0, 0, //
                    1, 1, 1, //
                    0, 0, 0,
                ],
            ),
            1,
        );

        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        // Peak is at one of the positions with value 0.9
        assert_eq!(c.peak_y, 1);
        assert!(c.peak_x <= 2); // One of 0, 1, or 2
        assert!((c.peak_value - 0.9).abs() < 1e-6);
    }

    #[test]
    fn peak_at_corner() {
        // Component with peak at corner of bounding box
        let pixels = Buffer2::new(
            3,
            3,
            vec![
                0.9, 0.5, 0.1, //
                0.5, 0.3, 0.1, //
                0.1, 0.1, 0.1,
            ],
        );
        let label_map = LabelMap::from_raw(
            Buffer2::new(
                3,
                3,
                vec![
                    1, 1, 0, //
                    1, 1, 0, //
                    0, 0, 0,
                ],
            ),
            1,
        );

        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        assert_eq!(c.peak_x, 0);
        assert_eq!(c.peak_y, 0);
        assert_eq!(c.bbox.min.x, 0);
        assert_eq!(c.bbox.max.x, 1);
        assert_eq!(c.bbox.min.y, 0);
        assert_eq!(c.bbox.max.y, 1);
    }

    #[test]
    fn single_pixel_component() {
        let pixels = Buffer2::new(
            3,
            3,
            vec![
                0.1, 0.1, 0.1, //
                0.1, 0.8, 0.1, //
                0.1, 0.1, 0.1,
            ],
        );
        let label_map = LabelMap::from_raw(
            Buffer2::new(
                3,
                3,
                vec![
                    0, 0, 0, //
                    0, 1, 0, //
                    0, 0, 0,
                ],
            ),
            1,
        );

        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        assert_eq!(c.area, 1);
        assert_eq!(c.bbox.min.x, 1);
        assert_eq!(c.bbox.max.x, 1);
        assert_eq!(c.bbox.min.y, 1);
        assert_eq!(c.bbox.max.y, 1);
        assert_eq!(c.width(), 1);
        assert_eq!(c.height(), 1);
        assert_eq!(c.peak_x, 1);
        assert_eq!(c.peak_y, 1);
    }

    #[test]
    fn diagonal_component() {
        // Diagonal stripe (connected via 4-connectivity would be separate,
        // but here we test extraction from pre-labeled data)
        let pixels = Buffer2::new(
            3,
            3,
            vec![
                0.9, 0.1, 0.1, //
                0.1, 0.8, 0.1, //
                0.1, 0.1, 0.7,
            ],
        );
        let label_map = LabelMap::from_raw(
            Buffer2::new(
                3,
                3,
                vec![
                    1, 0, 0, //
                    0, 1, 0, //
                    0, 0, 1,
                ],
            ),
            1,
        );

        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        assert_eq!(c.area, 3);
        assert_eq!(c.bbox.min.x, 0);
        assert_eq!(c.bbox.max.x, 2);
        assert_eq!(c.bbox.min.y, 0);
        assert_eq!(c.bbox.max.y, 2);
        // Peak is at (0, 0) with value 0.9
        assert_eq!(c.peak_x, 0);
        assert_eq!(c.peak_y, 0);
    }

    #[test]
    fn sparse_labels() {
        // Labels are not contiguous (1 and 3, no 2)
        // Empty components (label 2) are skipped in the output
        let pixels = Buffer2::new(
            3,
            3,
            vec![
                0.8, 0.1, 0.7, //
                0.1, 0.1, 0.1, //
                0.1, 0.1, 0.1,
            ],
        );
        // num_labels should be 3 to account for label 3
        let label_map = LabelMap::from_raw(
            Buffer2::new(
                3,
                3,
                vec![
                    1, 0, 3, //
                    0, 0, 0, //
                    0, 0, 0,
                ],
            ),
            3,
        );
        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        // Only non-empty components are returned (labels 1 and 3)
        assert_eq!(candidates.len(), 2);
        // Label 1 at (0, 0)
        assert_eq!(candidates[0].area, 1);
        assert_eq!(candidates[0].peak_x, 0);
        // Label 3 at (2, 0)
        assert_eq!(candidates[1].area, 1);
        assert_eq!(candidates[1].peak_x, 2);
    }

    #[test]
    fn full_image_component() {
        // Component covering entire image
        let pixels = Buffer2::new(3, 3, (0..9).map(|i| 0.1 + i as f32 * 0.1).collect());
        let label_map = LabelMap::from_raw(Buffer2::new(3, 3, vec![1u32; 9]), 1);

        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        assert_eq!(c.area, 9);
        assert_eq!(c.bbox.min.x, 0);
        assert_eq!(c.bbox.max.x, 2);
        assert_eq!(c.bbox.min.y, 0);
        assert_eq!(c.bbox.max.y, 2);
        // Peak is last pixel (2, 2) with value 0.9
        assert_eq!(c.peak_x, 2);
        assert_eq!(c.peak_y, 2);
        assert!((c.peak_value - 0.9).abs() < 1e-6);
    }

    #[test]
    fn negative_pixel_values() {
        // Negative pixel values (can happen with background subtraction)
        let pixels = Buffer2::new(
            3,
            3,
            vec![
                -0.5, -0.1, 0.1, //
                -0.1, 0.3, 0.1, //
                0.1, 0.1, 0.1,
            ],
        );
        let label_map = LabelMap::from_raw(
            Buffer2::new(
                3,
                3,
                vec![
                    1, 1, 0, //
                    1, 1, 0, //
                    0, 0, 0,
                ],
            ),
            1,
        );

        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        // Peak is at (1, 1) with value 0.3
        assert_eq!(c.peak_x, 1);
        assert_eq!(c.peak_y, 1);
        assert!((c.peak_value - 0.3).abs() < 1e-6);
    }

    #[test]
    fn many_components() {
        // 10 separate single-pixel components
        let mut pixels_data = vec![0.1f32; 100];
        let mut labels_data = vec![0u32; 100];

        for i in 0..10 {
            let idx = i * 10 + i; // Diagonal positions
            pixels_data[idx] = 0.5 + i as f32 * 0.05;
            labels_data[idx] = (i + 1) as u32;
        }

        let pixels = Buffer2::new(10, 10, pixels_data);
        let label_map = LabelMap::from_raw(Buffer2::new(10, 10, labels_data), 10);
        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(candidates.len(), 10);
        for (i, c) in candidates.iter().enumerate() {
            assert_eq!(c.area, 1);
            assert_eq!(c.peak_x, i);
            assert_eq!(c.peak_y, i);
        }
    }

    #[test]
    fn non_square_image() {
        // Wide image (7x2)
        let pixels = Buffer2::new(
            7,
            2,
            vec![
                0.1, 0.2, 0.9, 0.2, 0.1, 0.8, 0.1, //
                0.1, 0.2, 0.3, 0.2, 0.1, 0.7, 0.1,
            ],
        );
        let label_map = LabelMap::from_raw(
            Buffer2::new(
                7,
                2,
                vec![
                    0, 1, 1, 1, 0, 2, 0, //
                    0, 1, 1, 1, 0, 2, 0,
                ],
            ),
            2,
        );

        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(candidates.len(), 2);

        // Component 1: columns 1-3, both rows
        assert_eq!(candidates[0].area, 6);
        assert_eq!(candidates[0].bbox.min.x, 1);
        assert_eq!(candidates[0].bbox.max.x, 3);
        assert_eq!(candidates[0].peak_x, 2);
        assert_eq!(candidates[0].peak_y, 0);

        // Component 2: column 5, both rows
        assert_eq!(candidates[1].area, 2);
        assert_eq!(candidates[1].bbox.min.x, 5);
        assert_eq!(candidates[1].bbox.max.x, 5);
        assert_eq!(candidates[1].peak_x, 5);
        assert_eq!(candidates[1].peak_y, 0);
    }
}

// =============================================================================
// Integration Tests for Connected Components + Extract Candidates
// =============================================================================

mod integration_tests {
    use super::*;

    #[test]
    fn connected_components_and_extract() {
        // Create a 10x10 image with two star-like regions
        let mut mask = vec![false; 100];
        let mut pixels = vec![0.1f32; 100];

        // Star 1 at (2, 2) - 3x3 region
        for dy in 0..3 {
            for dx in 0..3 {
                let idx = (2 + dy) * 10 + (2 + dx);
                mask[idx] = true;
                pixels[idx] = 0.5
                    + 0.4 * (1.0 - ((dx as f32 - 1.0).powi(2) + (dy as f32 - 1.0).powi(2)) / 2.0);
            }
        }
        // Peak at center (3, 3)
        pixels[3 * 10 + 3] = 0.9;

        // Star 2 at (7, 7) - 2x2 region
        for dy in 0..2 {
            for dx in 0..2 {
                let idx = (7 + dy) * 10 + (7 + dx);
                mask[idx] = true;
                pixels[idx] = 0.6;
            }
        }
        // Peak at (7, 7)
        pixels[7 * 10 + 7] = 0.8;

        let label_map = LabelMap::from_mask(&BitBuffer2::from_slice(10, 10, &mask));
        let pixels = Buffer2::new(10, 10, pixels);
        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        assert_eq!(label_map.num_labels(), 2);
        assert_eq!(candidates.len(), 2);

        // Verify star 1
        let star1 = candidates
            .iter()
            .find(|c| c.peak_x == 3 && c.peak_y == 3)
            .unwrap();
        assert_eq!(star1.area, 9);
        assert!((star1.peak_value - 0.9).abs() < 1e-6);

        // Verify star 2
        let star2 = candidates
            .iter()
            .find(|c| c.peak_x == 7 && c.peak_y == 7)
            .unwrap();
        assert_eq!(star2.area, 4);
        assert!((star2.peak_value - 0.8).abs() < 1e-6);
    }

    #[test]
    fn connected_components_complex_merge() {
        // Test a complex scenario where union-find needs to merge multiple times
        // Shape like:
        //   ###
        //   ..#
        //   ###
        // The bottom row connects with top through right column
        let mut mask = vec![false; 9];
        // Top row
        mask[0] = true;
        mask[1] = true;
        mask[2] = true;
        // Middle right
        mask[5] = true;
        // Bottom row
        mask[6] = true;
        mask[7] = true;
        mask[8] = true;

        let label_map = LabelMap::from_mask(&BitBuffer2::from_slice(3, 3, &mask));

        // All should be one component connected through the right column
        assert_eq!(label_map.num_labels(), 1);
        let label = label_map[0];
        for i in [0, 1, 2, 5, 6, 7, 8] {
            assert_eq!(
                label_map[i], label,
                "Pixel {} should be in same component",
                i
            );
        }
    }
}

// =============================================================================
// Deblending Tests
// =============================================================================

mod deblend_tests {
    use super::*;

    #[test]
    fn star_pair() {
        // Create a component with two distinct peaks (star pair)
        // Image: 15x9 with two Gaussian-like stars at (4,4) and (10,4)
        let width = 15;
        let height = 9;
        let mut pixels = vec![0.1f32; width * height];

        // Add first star at (4, 4)
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
                let x = 4 + dx;
                let y = 4 + dy;
                if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                    let dist_sq = (dx * dx + dy * dy) as f32;
                    let value = 0.8 * (-dist_sq / 3.0).exp();
                    pixels[y as usize * width + x as usize] += value;
                }
            }
        }

        // Add second star at (10, 4)
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
                let x = 10 + dx;
                let y = 4 + dy;
                if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                    let dist_sq = (dx * dx + dy * dy) as f32;
                    let value = 0.6 * (-dist_sq / 3.0).exp();
                    pixels[y as usize * width + x as usize] += value;
                }
            }
        }

        // Create a single connected component covering both stars
        let mut labels_data = vec![0u32; width * height];
        for y in 1..8 {
            for x in 1..14 {
                if pixels[y * width + x] > 0.15 {
                    labels_data[y * width + x] = 1;
                }
            }
        }

        let pixels = Buffer2::new(width, height, pixels);
        let label_map = LabelMap::from_raw(Buffer2::new(width, height, labels_data), 1);
        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        // Should deblend into 2 candidates
        assert_eq!(
            candidates.len(),
            2,
            "Should deblend into 2 candidates, got {}",
            candidates.len()
        );

        // Sort by peak_x to have consistent ordering
        let mut sorted: Vec<_> = candidates.iter().collect();
        sorted.sort_by_key(|c| c.peak_x);

        // First star at approximately (4, 4)
        assert!(
            (sorted[0].peak_x as i32 - 4).abs() <= 1,
            "First peak X should be near 4, got {}",
            sorted[0].peak_x
        );
        assert!(
            (sorted[0].peak_y as i32 - 4).abs() <= 1,
            "First peak Y should be near 4, got {}",
            sorted[0].peak_y
        );

        // Second star at approximately (10, 4)
        assert!(
            (sorted[1].peak_x as i32 - 10).abs() <= 1,
            "Second peak X should be near 10, got {}",
            sorted[1].peak_x
        );
        assert!(
            (sorted[1].peak_y as i32 - 4).abs() <= 1,
            "Second peak Y should be near 4, got {}",
            sorted[1].peak_y
        );
    }

    #[test]
    fn no_deblend_for_close_peaks() {
        // Two peaks that are too close together should not be deblended
        let width = 9;
        let height = 9;
        let mut pixels = vec![0.1f32; width * height];

        // Add two peaks very close together (separation < DEBLEND_MIN_SEPARATION)
        pixels[4 * width + 3] = 0.9; // Peak at (3, 4)
        pixels[4 * width + 4] = 0.85; // Peak at (4, 4) - only 1 pixel away

        // Surrounding pixels
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let x = 3 + dx;
                let y = 4 + dy;
                if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                    let idx = y as usize * width + x as usize;
                    if pixels[idx] < 0.5 {
                        pixels[idx] = 0.5;
                    }
                }
            }
        }

        // Create single component
        let mut labels_data = vec![0u32; width * height];
        for y in 2..7 {
            for x in 1..7 {
                if pixels[y * width + x] > 0.15 {
                    labels_data[y * width + x] = 1;
                }
            }
        }

        let pixels = Buffer2::new(width, height, pixels);
        let label_map = LabelMap::from_raw(Buffer2::new(width, height, labels_data), 1);
        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        // Should NOT deblend - only one candidate because peaks are too close
        assert_eq!(
            candidates.len(),
            1,
            "Close peaks should not be deblended, got {} candidates",
            candidates.len()
        );
    }

    #[test]
    fn respects_prominence() {
        // A small secondary peak that's not prominent enough should not cause deblending
        let width = 11;
        let height = 9;
        let mut pixels = vec![0.1f32; width * height];

        // Add main star at (3, 4)
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
                let x = 3 + dx;
                let y = 4 + dy;
                if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                    let dist_sq = (dx * dx + dy * dy) as f32;
                    let value = 0.9 * (-dist_sq / 3.0).exp();
                    pixels[y as usize * width + x as usize] += value;
                }
            }
        }

        // Add small bump at (7, 4) - only 20% of main peak (below 30% threshold)
        pixels[4 * width + 7] = 0.28; // 0.1 bg + 0.18 = 0.28 (0.18/0.9 = 20%)

        // Create single component
        let mut labels_data = vec![0u32; width * height];
        for y in 0..height {
            for x in 0..width {
                if pixels[y * width + x] > 0.15 {
                    labels_data[y * width + x] = 1;
                }
            }
        }

        let pixels = Buffer2::new(width, height, pixels);
        let label_map = LabelMap::from_raw(Buffer2::new(width, height, labels_data), 1);
        let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG);

        // Should NOT deblend - secondary peak is not prominent enough
        assert_eq!(
            candidates.len(),
            1,
            "Non-prominent secondary peak should not cause deblending, got {} candidates",
            candidates.len()
        );
    }

    mod multi_threshold {
        use super::*;

        #[test]
        fn star_pair() {
            // Test multi-threshold deblending with two separated stars
            let width = 30;
            let height = 15;
            let mut pixels = vec![0.1f32; width * height];

            // Add first star at (7, 7)
            for dy in -4i32..=4 {
                for dx in -4i32..=4 {
                    let x = 7 + dx;
                    let y = 7 + dy;
                    if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                        let dist_sq = (dx * dx + dy * dy) as f32;
                        let value = 0.8 * (-dist_sq / 4.0).exp();
                        pixels[y as usize * width + x as usize] += value;
                    }
                }
            }

            // Add second star at (22, 7)
            for dy in -4i32..=4 {
                for dx in -4i32..=4 {
                    let x = 22 + dx;
                    let y = 7 + dy;
                    if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                        let dist_sq = (dx * dx + dy * dy) as f32;
                        let value = 0.6 * (-dist_sq / 4.0).exp();
                        pixels[y as usize * width + x as usize] += value;
                    }
                }
            }

            // Create a single connected component covering both stars
            let mut labels_data = vec![0u32; width * height];
            for y in 0..height {
                for x in 0..width {
                    if pixels[y * width + x] > 0.15 {
                        labels_data[y * width + x] = 1;
                    }
                }
            }

            // Use multi-threshold deblending config
            let mt_config = DeblendConfig {
                min_separation: 3,
                min_prominence: 0.3,
                n_thresholds: 32,
                min_contrast: 0.005,
                ..Default::default()
            };

            let pixels = Buffer2::new(width, height, pixels);
            let label_map = LabelMap::from_raw(Buffer2::new(width, height, labels_data), 1);
            let candidates = extract_candidates(&pixels, &label_map, &mt_config);

            // Should deblend into 2 candidates
            assert_eq!(
                candidates.len(),
                2,
                "Multi-threshold deblend should find 2 candidates, got {}",
                candidates.len()
            );

            // Sort by peak_x to have consistent ordering
            let mut sorted: Vec<_> = candidates.iter().collect();
            sorted.sort_by_key(|c| c.peak_x);

            // Check peak positions
            assert!(
                (sorted[0].peak_x as i32 - 7).abs() <= 1,
                "First peak X should be near 7, got {}",
                sorted[0].peak_x
            );
            assert!(
                (sorted[1].peak_x as i32 - 22).abs() <= 1,
                "Second peak X should be near 22, got {}",
                sorted[1].peak_x
            );
        }

        #[test]
        fn vs_simple_deblend_consistency() {
            // Both deblending methods should produce similar results for clear star pairs
            let width = 30;
            let height = 15;
            let mut pixels = vec![0.1f32; width * height];

            // Add two well-separated stars
            for dy in -3i32..=3 {
                for dx in -3i32..=3 {
                    // Star 1 at (7, 7)
                    let x1 = 7 + dx;
                    let y1 = 7 + dy;
                    if x1 >= 0 && x1 < width as i32 && y1 >= 0 && y1 < height as i32 {
                        let dist_sq = (dx * dx + dy * dy) as f32;
                        pixels[y1 as usize * width + x1 as usize] += 0.8 * (-dist_sq / 3.0).exp();
                    }

                    // Star 2 at (22, 7)
                    let x2 = 22 + dx;
                    let y2 = 7 + dy;
                    if x2 >= 0 && x2 < width as i32 && y2 >= 0 && y2 < height as i32 {
                        let dist_sq = (dx * dx + dy * dy) as f32;
                        pixels[y2 as usize * width + x2 as usize] += 0.7 * (-dist_sq / 3.0).exp();
                    }
                }
            }

            // Create a single component
            let mut labels_data = vec![0u32; width * height];
            for y in 0..height {
                for x in 0..width {
                    if pixels[y * width + x] > 0.15 {
                        labels_data[y * width + x] = 1;
                    }
                }
            }

            // Simple deblending (n_thresholds = 0)
            let simple_config = DeblendConfig {
                min_separation: 3,
                min_prominence: 0.3,
                n_thresholds: 0,
                min_contrast: 0.005,
                ..Default::default()
            };
            let pixels = Buffer2::new(width, height, pixels);
            let label_map = LabelMap::from_raw(Buffer2::new(width, height, labels_data), 1);
            let simple_candidates = extract_candidates(&pixels, &label_map, &simple_config);

            // Multi-threshold deblending (n_thresholds > 0)
            let mt_config = DeblendConfig {
                min_separation: 3,
                min_prominence: 0.3,
                n_thresholds: 32,
                min_contrast: 0.005,
                ..Default::default()
            };
            let mt_candidates = extract_candidates(&pixels, &label_map, &mt_config);

            // Both should find 2 stars
            assert_eq!(
                simple_candidates.len(),
                2,
                "Simple deblend should find 2 stars"
            );
            assert_eq!(
                mt_candidates.len(),
                2,
                "Multi-threshold deblend should find 2 stars"
            );

            // Peak positions should be similar
            let mut simple_sorted: Vec<_> = simple_candidates.iter().collect();
            simple_sorted.sort_by_key(|c| c.peak_x);

            let mut mt_sorted: Vec<_> = mt_candidates.iter().collect();
            mt_sorted.sort_by_key(|c| c.peak_x);

            for i in 0..2 {
                assert!(
                    (simple_sorted[i].peak_x as i32 - mt_sorted[i].peak_x as i32).abs() <= 2,
                    "Peak {} X positions should be similar",
                    i
                );
                assert!(
                    (simple_sorted[i].peak_y as i32 - mt_sorted[i].peak_y as i32).abs() <= 2,
                    "Peak {} Y positions should be similar",
                    i
                );
            }
        }

        #[test]
        fn high_contrast_disables() {
            // Setting min_contrast to 1.0 should disable multi-threshold deblending
            let width = 30;
            let height = 15;
            let mut pixels = vec![0.1f32; width * height];

            // Add two separated stars
            for dy in -3i32..=3 {
                for dx in -3i32..=3 {
                    let x1 = 7 + dx;
                    let y1 = 7 + dy;
                    if x1 >= 0 && x1 < width as i32 && y1 >= 0 && y1 < height as i32 {
                        let dist_sq = (dx * dx + dy * dy) as f32;
                        pixels[y1 as usize * width + x1 as usize] += 0.8 * (-dist_sq / 3.0).exp();
                    }

                    let x2 = 22 + dx;
                    let y2 = 7 + dy;
                    if x2 >= 0 && x2 < width as i32 && y2 >= 0 && y2 < height as i32 {
                        let dist_sq = (dx * dx + dy * dy) as f32;
                        pixels[y2 as usize * width + x2 as usize] += 0.7 * (-dist_sq / 3.0).exp();
                    }
                }
            }

            // Create a single component
            let mut labels_data = vec![0u32; width * height];
            for y in 0..height {
                for x in 0..width {
                    if pixels[y * width + x] > 0.15 {
                        labels_data[y * width + x] = 1;
                    }
                }
            }

            // Multi-threshold with min_contrast = 1.0 (disabled)
            let config = DeblendConfig {
                min_separation: 3,
                min_prominence: 0.3,
                n_thresholds: 32,
                min_contrast: 1.0, // Disabled
                ..Default::default()
            };

            let pixels = Buffer2::new(width, height, pixels);
            let label_map = LabelMap::from_raw(Buffer2::new(width, height, labels_data), 1);
            let candidates = extract_candidates(&pixels, &label_map, &config);

            // Should return single candidate (deblending disabled)
            assert_eq!(
                candidates.len(),
                1,
                "High min_contrast should disable multi-threshold deblending, got {} candidates",
                candidates.len()
            );
        }
    }
}

// =============================================================================
// Filtered Image Detection Tests
// =============================================================================

mod filtered_image_tests {
    use super::*;

    #[test]
    fn with_filtered_image() {
        // Test the filtered image path where background-subtracted image is provided
        let width = 100;
        let height = 100;

        // Create raw image with star + background
        let mut pixels = vec![0.5f32; width * height]; // High background
        // Add star at (50, 50)
        for dy in -5i32..=5 {
            for dx in -5i32..=5 {
                let x = 50 + dx;
                let y = 50 + dy;
                if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                    let dist_sq = (dx * dx + dy * dy) as f32;
                    let value = 0.8 * (-dist_sq / 6.0).exp();
                    pixels[y as usize * width + x as usize] += value;
                }
            }
        }
        let pixels = Buffer2::new(width, height, pixels);

        // Create filtered (background-subtracted) image
        let mut filtered = vec![0.0f32; width * height];
        // Same star, but background is zero
        for dy in -5i32..=5 {
            for dx in -5i32..=5 {
                let x = 50 + dx;
                let y = 50 + dy;
                if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                    let dist_sq = (dx * dx + dy * dy) as f32;
                    let value = 0.8 * (-dist_sq / 6.0).exp();
                    filtered[y as usize * width + x as usize] = value;
                }
            }
        }
        let filtered = Buffer2::new(width, height, filtered);

        let bg = BackgroundMap::new(
            &pixels,
            &BackgroundConfig {
                tile_size: 32,
                ..Default::default()
            },
        );
        let config = StarDetectionConfig::default();

        // Detection with filtered image
        let candidates = detect_stars(&pixels, Some(&filtered), &bg, &config);

        assert_eq!(
            candidates.len(),
            1,
            "Should detect one star using filtered image"
        );
        let star = &candidates[0];
        assert!(
            (star.peak_x as i32 - 50).abs() <= 1,
            "Peak X should be near 50"
        );
        assert!(
            (star.peak_y as i32 - 50).abs() <= 1,
            "Peak Y should be near 50"
        );
    }

    #[test]
    fn filtered_vs_unfiltered_consistency() {
        // Both paths should detect the same stars for clean images
        let width = 100;
        let height = 100;

        let mut pixels = vec![0.1f32; width * height];
        // Add star at (50, 50)
        for dy in -4i32..=4 {
            for dx in -4i32..=4 {
                let x = 50 + dx;
                let y = 50 + dy;
                if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                    let dist_sq = (dx * dx + dy * dy) as f32;
                    let value = 0.8 * (-dist_sq / 4.0).exp();
                    pixels[y as usize * width + x as usize] += value;
                }
            }
        }
        let pixels = Buffer2::new(width, height, pixels);

        let bg = BackgroundMap::new(
            &pixels,
            &BackgroundConfig {
                tile_size: 32,
                ..Default::default()
            },
        );
        let config = StarDetectionConfig::default();

        // Without filtered image
        let candidates_unfiltered = detect_stars(&pixels, None, &bg, &config);

        // Create background-subtracted image
        let filtered: Vec<f32> = pixels
            .pixels()
            .iter()
            .zip(bg.background.pixels().iter())
            .map(|(p, b)| p - b)
            .collect();
        let filtered = Buffer2::new(width, height, filtered);

        // With filtered image
        let candidates_filtered = detect_stars(&pixels, Some(&filtered), &bg, &config);

        // Both should detect same number of stars
        assert_eq!(
            candidates_unfiltered.len(),
            candidates_filtered.len(),
            "Both paths should detect same number of stars"
        );
        assert_eq!(candidates_unfiltered.len(), 1);

        // Peak positions should be similar
        assert!(
            (candidates_unfiltered[0].peak_x as i32 - candidates_filtered[0].peak_x as i32).abs()
                <= 1,
            "Peak X should be similar"
        );
        assert!(
            (candidates_unfiltered[0].peak_y as i32 - candidates_filtered[0].peak_y as i32).abs()
                <= 1,
            "Peak Y should be similar"
        );
    }
}

// =============================================================================
// Sigma Threshold Tests
// =============================================================================

mod sigma_threshold_tests {
    use super::*;

    #[test]
    fn rejects_noise() {
        // Stars below sigma threshold should not be detected
        let width = 64;
        let height = 64;
        let background_level = 0.1f32;
        let noise_level = 0.02f32;

        let mut pixels = vec![background_level; width * height];

        // Add a faint "star" that's only 2-sigma above background
        // This should NOT be detected with default sigma_threshold=4
        let faint_signal = noise_level * 2.0; // 2-sigma
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let x = 20 + dx;
                let y = 20 + dy;
                let dist_sq = (dx * dx + dy * dy) as f32;
                let value = faint_signal * (-dist_sq / 2.0).exp();
                pixels[y as usize * width + x as usize] += value;
            }
        }

        // Add a bright star that's 10-sigma above background
        // This SHOULD be detected
        let bright_signal = noise_level * 10.0; // 10-sigma
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
                let x = 45 + dx;
                let y = 45 + dy;
                let dist_sq = (dx * dx + dy * dy) as f32;
                let value = bright_signal * (-dist_sq / 3.0).exp();
                pixels[y as usize * width + x as usize] += value;
            }
        }

        let pixels = Buffer2::new(width, height, pixels);
        let bg = background_map::uniform(width, height, background_level, noise_level);
        let config = StarDetectionConfig::default();

        let candidates = detect_stars(&pixels, None, &bg, &config);

        // Should only detect the bright star, not the faint one
        assert_eq!(
            candidates.len(),
            1,
            "Should detect only 1 star (bright one), faint should be below threshold"
        );
        assert!(
            (candidates[0].peak_x as i32 - 45).abs() <= 2,
            "Detected star should be the bright one at x=45"
        );
    }

    #[test]
    fn sensitivity() {
        // Lower sigma threshold should detect more (fainter) stars
        let width = 100;
        let height = 100;
        let background_level = 0.1f32;
        let noise_level = 0.01f32;

        let mut pixels = vec![background_level; width * height];

        // Add stars with varying brightness (3-sigma, 5-sigma, 10-sigma)
        let star_positions = [(20, 20, 3.0), (50, 50, 5.0), (80, 80, 10.0)];

        for (sx, sy, sigma_mult) in star_positions {
            let signal = noise_level * sigma_mult;
            for dy in -3i32..=3 {
                for dx in -3i32..=3 {
                    let x = sx + dx;
                    let y = sy + dy;
                    if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                        let dist_sq = (dx * dx + dy * dy) as f32;
                        let value = signal * (-dist_sq / 3.0).exp();
                        pixels[y as usize * width + x as usize] += value;
                    }
                }
            }
        }

        let pixels = Buffer2::new(width, height, pixels);
        let bg = background_map::uniform(width, height, background_level, noise_level);

        // High threshold (6-sigma) - should detect only the brightest
        let config_high = StarDetectionConfig {
            background_config: BackgroundConfig {
                sigma_threshold: 6.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let candidates_high = detect_stars(&pixels, None, &bg, &config_high);

        // Medium threshold (4-sigma) - should detect 2 stars
        let config_med = StarDetectionConfig {
            background_config: BackgroundConfig {
                sigma_threshold: 4.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let candidates_med = detect_stars(&pixels, None, &bg, &config_med);

        // Low threshold (2-sigma) - should detect all 3
        let config_low = StarDetectionConfig {
            background_config: BackgroundConfig {
                sigma_threshold: 2.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let candidates_low = detect_stars(&pixels, None, &bg, &config_low);

        assert!(
            candidates_high.len() <= candidates_med.len(),
            "Higher threshold should detect fewer or equal stars"
        );
        assert!(
            candidates_med.len() <= candidates_low.len(),
            "Medium threshold should detect fewer or equal stars than low"
        );
        // The 10-sigma star should always be detected
        assert!(
            !candidates_high.is_empty(),
            "Should always detect the brightest star"
        );
    }
}

// =============================================================================
// Mask Dilation Tests
// =============================================================================

mod dilation_tests {
    use super::*;
    use crate::star_detection::common::dilate_mask;

    #[test]
    fn connects_nearby_pixels() {
        // Verify that dilation with radius 1 connects pixels that are 2 apart
        let width = 10;
        let height = 10;

        // Two pixels 2 apart horizontally - should connect after radius-1 dilation
        let mut mask_data = vec![false; width * height];
        mask_data[5 * width + 3] = true; // pixel at (3, 5)
        mask_data[5 * width + 5] = true; // pixel at (5, 5) - 2 apart

        let mask = BitBuffer2::from_slice(width, height, &mask_data);
        let mut dilated = BitBuffer2::new_filled(width, height, false);
        dilate_mask(&mask, 1, &mut dilated);

        // After dilation, the gap at (4, 5) should be filled
        assert!(
            dilated.get(5 * width + 4),
            "Gap should be filled by dilation"
        );

        // The original pixels should still be set
        assert!(dilated.get(5 * width + 3), "Original pixel should remain");
        assert!(dilated.get(5 * width + 5), "Original pixel should remain");
    }

    #[test]
    fn structuring_element_3x3() {
        // Verify that radius-1 dilation creates a 3x3 structuring element
        let width = 10;
        let height = 10;

        // Single pixel at center
        let mut mask_data = vec![false; width * height];
        mask_data[5 * width + 5] = true; // pixel at (5, 5)

        let mask = BitBuffer2::from_slice(width, height, &mask_data);
        let mut dilated = BitBuffer2::new_filled(width, height, false);
        dilate_mask(&mask, 1, &mut dilated);

        // After radius-1 dilation, should have 3x3 region (cross pattern for 4-conn, or full 3x3)
        // Check that immediate neighbors are set
        assert!(dilated.get(5 * width + 5), "Center should be set");
        assert!(dilated.get(4 * width + 5), "Top should be set");
        assert!(dilated.get(6 * width + 5), "Bottom should be set");
        assert!(dilated.get(5 * width + 4), "Left should be set");
        assert!(dilated.get(5 * width + 6), "Right should be set");

        // Count total dilated pixels (depends on whether diagonal is included)
        let count: usize = (0..width * height).filter(|&i| dilated.get(i)).count();
        // Should be at least 5 (cross) or 9 (full 3x3)
        assert!(count >= 5, "Should dilate to at least cross pattern");
    }

    #[test]
    fn connects_bayer_artifacts() {
        // Verify the detection pipeline's dilation connects fragmented detections
        // This simulates Bayer pattern artifacts where a star might have gaps

        let width = 32;
        let height = 32;
        let mut pixels = vec![0.1f32; width * height];

        // Create a "fragmented" star with gaps (simulating Bayer artifacts)
        // Pattern:  # . #
        //           . # .
        //           # . #
        let star_center = (16, 16);
        for (dx, dy) in [(-1, -1), (1, -1), (0, 0), (-1, 1), (1, 1)] {
            let x = (star_center.0 + dx) as usize;
            let y = (star_center.1 + dy) as usize;
            pixels[y * width + x] = 0.9;
        }

        let pixels = Buffer2::new(width, height, pixels);
        let bg = BackgroundMap::new(
            &pixels,
            &BackgroundConfig {
                tile_size: 16,
                ..Default::default()
            },
        );
        let config = StarDetectionConfig::default();

        let candidates = detect_stars(&pixels, None, &bg, &config);

        // With dilation, the fragmented pixels should be connected into ONE star
        assert_eq!(
            candidates.len(),
            1,
            "Fragmented star should be connected by dilation into single detection"
        );
    }
}

// =============================================================================
// Regression Tests
// =============================================================================

mod regression_tests {
    use super::*;
    use crate::testing::synthetic::stamps::benchmark_star_field;

    #[test]
    fn detect_stars_6k() {
        // Regression test for image detection - ensures consistency
        let pixels = benchmark_star_field(256, 256, 100, 0.1, 0.01, 42);
        let bg = BackgroundMap::new(
            &pixels,
            &BackgroundConfig {
                tile_size: 32,
                ..Default::default()
            },
        );
        let config = StarDetectionConfig::default();

        let candidates = detect_stars(&pixels, None, &bg, &config);

        // Should detect some stars (exact count depends on random placement,
        // edge margin, overlaps, SNR thresholds, etc.)
        assert!(
            !candidates.is_empty(),
            "Should detect at least some stars, got {}",
            candidates.len()
        );
        assert!(
            candidates.len() <= 200,
            "Should not detect an unreasonable number of candidates, got {}",
            candidates.len()
        );
    }

    #[test]
    fn detect_stars_dense_field() {
        // Test with moderately dense star field - verifies deblending works
        // 256x256 with 50 stars is moderately dense but not overwhelming
        let pixels = benchmark_star_field(256, 256, 50, 0.1, 0.01, 42);
        let bg = BackgroundMap::new(
            &pixels,
            &BackgroundConfig {
                tile_size: 32,
                ..Default::default()
            },
        );
        let config = StarDetectionConfig {
            edge_margin: 5,
            ..Default::default()
        };

        let candidates = detect_stars(&pixels, None, &bg, &config);

        // Should detect some stars (exact count depends on overlaps and edge margin)
        assert!(
            !candidates.is_empty(),
            "Should detect some stars in dense field, got {}",
            candidates.len()
        );

        // Verify all candidates have valid properties
        for (i, c) in candidates.iter().enumerate() {
            assert!(c.area > 0, "Candidate {} should have area > 0", i);
            assert!(
                c.peak_value > 0.0,
                "Candidate {} should have positive peak",
                i
            );
            assert!(
                c.peak_x < 256 && c.peak_y < 256,
                "Candidate {} peak should be within bounds",
                i
            );
        }
    }

    #[test]
    fn large_scale_parallel_consistency() {
        // Test that parallel processing gives consistent results
        // Large enough to trigger parallel path (>100k pixels)
        let pixels = benchmark_star_field(400, 300, 50, 0.1, 0.01, 12345);
        let bg = BackgroundMap::new(
            &pixels,
            &BackgroundConfig {
                tile_size: 32,
                ..Default::default()
            },
        );
        let config = StarDetectionConfig::default();

        // Run detection multiple times - should give identical results
        let candidates1 = detect_stars(&pixels, None, &bg, &config);
        let candidates2 = detect_stars(&pixels, None, &bg, &config);

        assert_eq!(
            candidates1.len(),
            candidates2.len(),
            "Multiple runs should give same count"
        );

        // Sort by peak position for comparison (order may differ due to parallel processing)
        let mut sorted1: Vec<_> = candidates1.iter().collect();
        let mut sorted2: Vec<_> = candidates2.iter().collect();
        sorted1.sort_by_key(|c| (c.peak_y, c.peak_x));
        sorted2.sort_by_key(|c| (c.peak_y, c.peak_x));

        // Verify same peaks detected (after sorting)
        for (c1, c2) in sorted1.iter().zip(sorted2.iter()) {
            assert_eq!(c1.peak_x, c2.peak_x, "Peak X should be identical");
            assert_eq!(c1.peak_y, c2.peak_y, "Peak Y should be identical");
            assert_eq!(c1.area, c2.area, "Area should be identical");
        }
    }

    #[test]
    fn label_map_parallel_consistency() {
        // Verify parallel CCL gives consistent component structure
        // Note: Label IDs may differ between runs, but component grouping must be consistent
        let pixels = benchmark_star_field(400, 300, 100, 0.1, 0.01, 42);
        let bg = BackgroundMap::new(
            &pixels,
            &BackgroundConfig {
                tile_size: 32,
                ..Default::default()
            },
        );

        // Create threshold mask
        let mut mask = BitBuffer2::new_filled(400, 300, false);
        for y in 0..300 {
            for x in 0..400 {
                let idx = y * 400 + x;
                let threshold = bg.background[idx] + 4.0 * bg.noise[idx];
                if pixels[idx] > threshold {
                    mask.set(idx, true);
                }
            }
        }

        // Run labeling multiple times
        let label_map1 = LabelMap::from_mask(&mask);
        let label_map2 = LabelMap::from_mask(&mask);

        assert_eq!(
            label_map1.num_labels(),
            label_map2.num_labels(),
            "Parallel labeling should give same num_labels"
        );

        // Verify component structure is consistent:
        // If two pixels have the same label in map1, they should have the same label in map2
        // (and vice versa). We check this by building equivalence classes.
        let width = 400;
        let height = 300;

        // For each pair of adjacent pixels, if they're in the same component in one map,
        // they should be in the same component in the other map
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                if label_map1[idx] == 0 {
                    assert_eq!(label_map2[idx], 0, "Background pixels should be consistent");
                    continue;
                }

                // Check right neighbor
                if x + 1 < width {
                    let right_idx = y * width + (x + 1);
                    let same_in_1 = label_map1[idx] == label_map1[right_idx];
                    let same_in_2 = label_map2[idx] == label_map2[right_idx];
                    assert_eq!(
                        same_in_1, same_in_2,
                        "Component connectivity should be consistent at ({}, {})",
                        x, y
                    );
                }

                // Check bottom neighbor
                if y + 1 < height {
                    let bottom_idx = (y + 1) * width + x;
                    let same_in_1 = label_map1[idx] == label_map1[bottom_idx];
                    let same_in_2 = label_map2[idx] == label_map2[bottom_idx];
                    assert_eq!(
                        same_in_1, same_in_2,
                        "Component connectivity should be consistent at ({}, {})",
                        x, y
                    );
                }
            }
        }
    }
}

// =============================================================================
// Benchmarks
// =============================================================================

mod quick_benches {
    use super::*;
    use crate::testing::synthetic::stamps;
    use ::bench::quick_bench;

    #[quick_bench(warmup_iters = 2, iters = 5)]
    fn bench_detect_stars_filtered_1k(b: ::bench::Bencher) {
        let pixels = stamps::benchmark_star_field(1024, 1024, 100, 0.1, 0.01, 12345);
        let filtered = stamps::benchmark_star_field(1024, 1024, 100, 0.0, 0.01, 12345);
        let background = background_map::uniform(1024, 1024, 0.1, 0.01);
        let config = StarDetectionConfig::default();

        b.bench(|| detect_stars(&pixels, Some(&filtered), &background, &config));
    }

    #[quick_bench(warmup_iters = 1, iters = 3)]
    fn bench_detect_stars_filtered_6k(b: ::bench::Bencher) {
        let pixels = stamps::benchmark_star_field(6144, 6144, 3000, 0.1, 0.01, 12345);
        let filtered = stamps::benchmark_star_field(6144, 6144, 3000, 0.0, 0.01, 12345);
        let background = background_map::uniform(6144, 6144, 0.1, 0.01);
        let config = StarDetectionConfig::default();

        b.bench(|| detect_stars(&pixels, Some(&filtered), &background, &config));
    }
}
