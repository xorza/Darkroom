//! Tests for connected component labeling.

// Allow identity operations like `y * width + x` for clarity in 2D indexing
#![allow(clippy::identity_op, clippy::erasing_op)]

use super::*;
use crate::common::BitBuffer2;

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

/// Tests for 8-connectivity mode
mod eight_connectivity {
    use super::*;
    use crate::star_detection::config::Connectivity;

    #[test]
    fn diagonal_connected() {
        // 3x3 mask with diagonal pixels
        // #..
        // .#.
        // ..#
        // With 4-connectivity: 3 separate components
        // With 8-connectivity: 1 component
        let mut mask_data = vec![false; 9];
        mask_data[0 * 3 + 0] = true;
        mask_data[1 * 3 + 1] = true;
        mask_data[2 * 3 + 2] = true;
        let mask = BitBuffer2::from_slice(3, 3, &mask_data);

        // 4-connectivity: diagonals are separate
        let label_map_4 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Four);
        assert_eq!(label_map_4.num_labels(), 3);

        // 8-connectivity: diagonals are connected
        let label_map_8 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Eight);
        assert_eq!(label_map_8.num_labels(), 1);
    }

    #[test]
    fn anti_diagonal_connected() {
        // Anti-diagonal
        // ..#
        // .#.
        // #..
        let mut mask_data = vec![false; 9];
        mask_data[0 * 3 + 2] = true;
        mask_data[1 * 3 + 1] = true;
        mask_data[2 * 3 + 0] = true;
        let mask = BitBuffer2::from_slice(3, 3, &mask_data);

        // 4-connectivity: diagonals are separate
        let label_map_4 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Four);
        assert_eq!(label_map_4.num_labels(), 3);

        // 8-connectivity: diagonals are connected
        let label_map_8 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Eight);
        assert_eq!(label_map_8.num_labels(), 1);
    }

    #[test]
    fn checkerboard_8conn() {
        // 4x4 checkerboard - all connected with 8-connectivity
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

        // 4-connectivity: each pixel is isolated
        let label_map_4 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Four);
        assert_eq!(label_map_4.num_labels(), 8);

        // 8-connectivity: all are connected diagonally
        let label_map_8 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Eight);
        assert_eq!(label_map_8.num_labels(), 1);
    }

    #[test]
    fn adjacent_runs_diagonal() {
        // Test diagonal adjacency between runs
        // ###....
        // ....###
        // With 4-conn: 2 components (runs don't overlap)
        // With 8-conn: 1 component (runs touch at diagonal: run1.end=3, run2.start=4)
        let width = 7;
        let height = 2;
        let mut mask_data = vec![false; width * height];

        // Row 0: x=0-3
        for x in 0..3 {
            mask_data[0 * width + x] = true;
        }
        // Row 1: x=3-6 (diagonal touch at x=3)
        for x in 3..6 {
            mask_data[1 * width + x] = true;
        }

        let mask = BitBuffer2::from_slice(width, height, &mask_data);

        // 4-connectivity: no overlap (run1 ends at 3, run2 starts at 3)
        // Actually, run1=[0,3), run2=[3,6), so they share x=3? No, run1.end=3 exclusive
        // So run1 covers x=0,1,2 and run2 covers x=3,4,5 - no vertical overlap
        let label_map_4 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Four);
        assert_eq!(label_map_4.num_labels(), 2);

        // 8-connectivity: diagonal touch (x=2 in row0 touches x=3 in row1)
        let label_map_8 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Eight);
        assert_eq!(label_map_8.num_labels(), 1);
    }

    #[test]
    fn l_shape_diagonal_gap() {
        // L-shape with diagonal gap
        // ##....
        // ..##..
        // ....##
        // With 4-conn: 3 components
        // With 8-conn: 1 component (diagonal chain)
        let width = 6;
        let height = 3;
        let mut mask_data = vec![false; width * height];

        for x in 0..2 {
            mask_data[0 * width + x] = true;
        }
        for x in 2..4 {
            mask_data[1 * width + x] = true;
        }
        for x in 4..6 {
            mask_data[2 * width + x] = true;
        }

        let mask = BitBuffer2::from_slice(width, height, &mask_data);

        let label_map_4 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Four);
        assert_eq!(label_map_4.num_labels(), 3);

        let label_map_8 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Eight);
        assert_eq!(label_map_8.num_labels(), 1);
    }

    #[test]
    fn parallel_strip_boundary_diagonal() {
        // Test diagonal connectivity at strip boundaries (large image)
        let width = 400;
        let height = 300;
        let mut mask_data = vec![false; width * height];

        // Create a diagonal line that crosses strip boundaries
        // With 64-row strips, boundaries are at y=64, 128, 192
        for i in 0..150 {
            let x = 100 + i;
            let y = 50 + i; // Crosses y=64 boundary diagonally
            if x < width && y < height {
                mask_data[y * width + x] = true;
            }
        }

        let mask = BitBuffer2::from_slice(width, height, &mask_data);

        // 4-connectivity: each pixel is separate
        let label_map_4 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Four);
        assert_eq!(label_map_4.num_labels(), 150);

        // 8-connectivity: all pixels form one diagonal line
        let label_map_8 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Eight);
        assert_eq!(label_map_8.num_labels(), 1);
    }

    #[test]
    fn corner_touch_only() {
        // Two runs that only touch at corner
        // #.
        // .#
        let mut mask_data = vec![false; 4];
        mask_data[0] = true; // (0,0)
        mask_data[3] = true; // (1,1)
        let mask = BitBuffer2::from_slice(2, 2, &mask_data);

        // 4-conn: separate
        let label_map_4 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Four);
        assert_eq!(label_map_4.num_labels(), 2);

        // 8-conn: connected
        let label_map_8 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Eight);
        assert_eq!(label_map_8.num_labels(), 1);
    }

    #[test]
    fn horizontal_still_connected() {
        // Verify horizontal connectivity still works with 8-conn
        let mut mask_data = vec![false; 15];
        mask_data[1 * 5 + 0] = true;
        mask_data[1 * 5 + 1] = true;
        mask_data[1 * 5 + 2] = true;
        let mask = BitBuffer2::from_slice(5, 3, &mask_data);

        let label_map_8 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Eight);
        assert_eq!(label_map_8.num_labels(), 1);
    }

    #[test]
    fn vertical_still_connected() {
        // Verify vertical connectivity still works with 8-conn
        let mut mask_data = vec![false; 15];
        mask_data[0 * 3 + 1] = true;
        mask_data[1 * 3 + 1] = true;
        mask_data[2 * 3 + 1] = true;
        let mask = BitBuffer2::from_slice(3, 5, &mask_data);

        let label_map_8 = LabelMap::from_mask_with_connectivity(&mask, Connectivity::Eight);
        assert_eq!(label_map_8.num_labels(), 1);
    }
}
