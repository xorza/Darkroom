//! Tests for star detection.

// Allow identity operations like `y * width + x` for clarity in 2D indexing
#![allow(clippy::identity_op, clippy::erasing_op)]

use super::*;
use crate::common::{BitBuffer2, Buffer2};
use crate::star_detection::background::{BackgroundConfig, BackgroundMap};
use crate::star_detection::common::dilate_mask;

use crate::testing::synthetic::background_map;

/// Default deblend config for tests
const TEST_DEBLEND_CONFIG: DeblendConfig = DeblendConfig {
    min_separation: 3,
    min_prominence: 0.3,
    multi_threshold: false,
    n_thresholds: 32,
    min_contrast: 0.005,
};

/// Default max_area for tests (large enough to not filter anything in small test images)
const TEST_MAX_AREA: usize = 10000;

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

#[test]
fn test_detect_single_star() {
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
fn test_detect_multiple_stars() {
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
fn test_reject_edge_stars() {
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
fn test_reject_small_objects() {
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
fn test_empty_image() {
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

// =============================================================================
// Connected Components Tests (LabelMap::from_mask)
// =============================================================================

#[test]
fn test_label_map_empty_mask() {
    let mask = BitBuffer2::from_slice(4, 4, &[false; 16]);
    let label_map = LabelMap::from_mask(&mask);

    assert_eq!(label_map.num_labels(), 0);
    assert!(label_map.iter().all(|&l| l == 0));
}

#[test]
fn test_label_map_single_pixel() {
    // 4x4 mask with single pixel at (1, 1)
    let mut mask_data = vec![false; 16];
    mask_data[1 * 4 + 1] = true;
    let mask = BitBuffer2::from_slice(4, 4, &mask_data);

    let label_map = LabelMap::from_mask(&mask);

    assert_eq!(label_map.num_labels(), 1);
    assert_eq!(label_map[1 * 4 + 1], 1);
    assert_eq!(label_map.iter().filter(|&&l| l == 1).count(), 1);
}

#[test]
fn test_label_map_horizontal_line() {
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
fn test_label_map_vertical_line() {
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
fn test_label_map_two_separate_regions() {
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
fn test_label_map_l_shape() {
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
fn test_label_map_diagonal_not_connected() {
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
fn test_label_map_u_shape_union_find() {
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
fn test_label_map_checkerboard() {
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
fn test_label_map_filled_rectangle() {
    // 3x3 all true
    let mask = BitBuffer2::from_slice(3, 3, &[true; 9]);
    let label_map = LabelMap::from_mask(&mask);

    assert_eq!(label_map.num_labels(), 1);
    assert!(label_map.iter().all(|&l| l == 1));
}

#[test]
fn test_label_map_labels_are_sequential() {
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

// =============================================================================
// Dilate Mask Tests
// =============================================================================

#[test]
fn test_dilate_mask_empty() {
    let mask = BitBuffer2::from_slice(3, 3, &[false; 9]);
    let mut dilated = BitBuffer2::new_filled(3, 3, false);
    dilate_mask(&mask, 1, &mut dilated);
    assert!(dilated.iter().all(|x| !x));
}

#[test]
fn test_dilate_mask_single_pixel_radius_0() {
    // Radius 0 should not expand
    let mut mask_data = vec![false; 9];
    mask_data[4] = true; // center
    let mask = BitBuffer2::from_slice(3, 3, &mask_data);
    let mut dilated = BitBuffer2::new_filled(3, 3, false);
    dilate_mask(&mask, 0, &mut dilated);

    assert_eq!(dilated.iter().filter(|x| *x).count(), 1);
    assert!(dilated.get(4));
}

#[test]
fn test_dilate_mask_single_pixel_radius_1() {
    // 3x3 mask with center pixel, radius 1 should create 3x3 square
    let mut mask_data = vec![false; 25]; // 5x5
    mask_data[2 * 5 + 2] = true; // center at (2, 2)
    let mask = BitBuffer2::from_slice(5, 5, &mask_data);
    let mut dilated = BitBuffer2::new_filled(5, 5, false);
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
fn test_dilate_mask_single_pixel_radius_2() {
    // 7x7 mask with center pixel, radius 2 should create 5x5 square
    let mut mask_data = vec![false; 49];
    mask_data[3 * 7 + 3] = true; // center at (3, 3)
    let mask = BitBuffer2::from_slice(7, 7, &mask_data);
    let mut dilated = BitBuffer2::new_filled(7, 7, false);
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
fn test_dilate_mask_corner_pixel() {
    // Pixel at corner (0,0), dilation should be clipped to image bounds
    let mut mask_data = vec![false; 16];
    mask_data[0] = true;
    let mask = BitBuffer2::from_slice(4, 4, &mask_data);
    let mut dilated = BitBuffer2::new_filled(4, 4, false);
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
fn test_dilate_mask_edge_pixel() {
    // Pixel at edge (0, 2) in 5x5
    let mut mask_data = vec![false; 25];
    mask_data[2 * 5 + 0] = true;
    let mask = BitBuffer2::from_slice(5, 5, &mask_data);
    let mut dilated = BitBuffer2::new_filled(5, 5, false);
    dilate_mask(&mask, 1, &mut dilated);

    // Should expand but clip at left edge
    assert!(dilated.get(1 * 5 + 0));
    assert!(dilated.get(1 * 5 + 1));
    assert!(dilated.get(2 * 5 + 0));
    assert!(dilated.get(2 * 5 + 1));
    assert!(dilated.get(3 * 5 + 0));
    assert!(dilated.get(3 * 5 + 1));
}

#[test]
fn test_dilate_mask_merges_nearby_pixels() {
    // Two pixels separated by gap, dilation should merge them
    // 7x1: #..#...
    let mut mask_data = vec![false; 7];
    mask_data[0] = true;
    mask_data[3] = true;
    let mask = BitBuffer2::from_slice(7, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(7, 1, false);
    dilate_mask(&mask, 2, &mut dilated);

    // Both should expand and merge
    // Pixel 0 expands to 0,1,2
    // Pixel 3 expands to 1,2,3,4,5
    // Merged: 0,1,2,3,4,5
    for i in 0..6 {
        assert!(dilated.get(i), "Pixel {} should be true after dilation", i);
    }
    assert!(!dilated.get(6));
}

// =============================================================================
// Additional Dilate Mask Tests
// =============================================================================

#[test]
fn test_dilate_mask_large_radius() {
    // 11x11 image with center pixel, radius 5 should fill most of image
    let mut mask_data = vec![false; 121];
    mask_data[5 * 11 + 5] = true; // center
    let mask = BitBuffer2::from_slice(11, 11, &mask_data);
    let mut dilated = BitBuffer2::new_filled(11, 11, false);
    dilate_mask(&mask, 5, &mut dilated);

    // Should create 11x11 square (capped at image bounds)
    assert!(dilated.iter().all(|x| x), "All pixels should be dilated");
}

#[test]
fn test_dilate_mask_radius_larger_than_image() {
    // Radius larger than image dimensions
    let mut mask_data = vec![false; 9];
    mask_data[4] = true; // center of 3x3
    let mask = BitBuffer2::from_slice(3, 3, &mask_data);
    let mut dilated = BitBuffer2::new_filled(3, 3, false);
    dilate_mask(&mask, 100, &mut dilated);

    // Should fill entire image
    assert!(dilated.iter().all(|x| x));
}

#[test]
fn test_dilate_mask_all_corners() {
    // All four corners set
    let mut mask_data = vec![false; 25]; // 5x5
    mask_data[0 * 5 + 0] = true; // top-left
    mask_data[0 * 5 + 4] = true; // top-right
    mask_data[4 * 5 + 0] = true; // bottom-left
    mask_data[4 * 5 + 4] = true; // bottom-right
    let mask = BitBuffer2::from_slice(5, 5, &mask_data);
    let mut dilated = BitBuffer2::new_filled(5, 5, false);
    dilate_mask(&mask, 1, &mut dilated);

    // Check corner expansions
    // Top-left expands to (0,0), (0,1), (1,0), (1,1)
    assert!(dilated.get(0 * 5 + 0));
    assert!(dilated.get(0 * 5 + 1));
    assert!(dilated.get(1 * 5 + 0));
    assert!(dilated.get(1 * 5 + 1));

    // Center should still be false (corners don't reach it with radius 1)
    assert!(!dilated.get(2 * 5 + 2));
}

#[test]
fn test_dilate_mask_full_coverage_radius_2() {
    // Two pixels that should merge with radius 2
    // 9x1: #...#....
    let mut mask_data = vec![false; 9];
    mask_data[0] = true;
    mask_data[4] = true;
    let mask = BitBuffer2::from_slice(9, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(9, 1, false);
    dilate_mask(&mask, 2, &mut dilated);

    // Pixel 0 expands to 0,1,2
    // Pixel 4 expands to 2,3,4,5,6
    // Together: 0,1,2,3,4,5,6 (overlap at 2)
    for i in 0..7 {
        assert!(dilated.get(i), "Pixel {} should be true", i);
    }
    assert!(!dilated.get(7));
    assert!(!dilated.get(8));
}

#[test]
fn test_dilate_mask_non_square_image() {
    // 7x3 image with pixel at (3, 1)
    let mut mask_data = vec![false; 21];
    mask_data[1 * 7 + 3] = true;
    let mask = BitBuffer2::from_slice(7, 3, &mask_data);
    let mut dilated = BitBuffer2::new_filled(7, 3, false);
    dilate_mask(&mask, 1, &mut dilated);

    // Should create 3x3 square centered at (3, 1)
    for y in 0..3 {
        for x in 2..=4 {
            assert!(
                dilated.get(y * 7 + x),
                "Pixel ({}, {}) should be true",
                x,
                y
            );
        }
    }
    // Outside should be false
    assert!(!dilated.get(0 * 7 + 0));
    assert!(!dilated.get(0 * 7 + 6));
}

#[test]
fn test_dilate_mask_preserves_original_pixels() {
    // Original pixels should always be in dilated result
    let mut mask_data = vec![false; 25];
    mask_data[0] = true;
    mask_data[12] = true; // center
    mask_data[24] = true;
    let mask = BitBuffer2::from_slice(5, 5, &mask_data);
    let mut dilated = BitBuffer2::new_filled(5, 5, false);
    dilate_mask(&mask, 1, &mut dilated);

    // All original pixels must be present
    assert!(dilated.get(0));
    assert!(dilated.get(12));
    assert!(dilated.get(24));
}

// =============================================================================
// Extract Candidates Tests
// =============================================================================

#[test]
fn test_extract_candidates_empty() {
    let pixels = Buffer2::new(3, 3, vec![0.5; 9]);
    let label_map = LabelMap::from_raw(Buffer2::new(3, 3, vec![0u32; 9]), 0);

    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

    assert!(candidates.is_empty());
}

#[test]
fn test_extract_candidates_single_component() {
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

    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

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
fn test_extract_candidates_two_components() {
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

    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

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
fn test_extract_candidates_bounding_box() {
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
    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

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
fn test_extract_candidates_width_height() {
    let pixels = Buffer2::new(3, 2, vec![0.5; 6]);
    // 3x2 component covering full image
    let label_map = LabelMap::from_raw(Buffer2::new(3, 2, vec![1u32; 6]), 1);

    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

    assert_eq!(candidates.len(), 1);
    let c = &candidates[0];
    assert_eq!(c.width(), 3);
    assert_eq!(c.height(), 2);
}

#[test]
fn test_extract_candidates_max_area_filter() {
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
    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, 10);
    assert_eq!(candidates.len(), 1, "Only small component should be found");
    assert_eq!(candidates[0].area, 5);

    // With max_area=100, both should be found
    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, 100);
    assert_eq!(candidates.len(), 2, "Both components should be found");
}

#[test]
fn test_extract_candidates_multiple_peaks_same_value() {
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

    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

    assert_eq!(candidates.len(), 1);
    let c = &candidates[0];
    // Peak is at one of the positions with value 0.9
    assert_eq!(c.peak_y, 1);
    assert!(c.peak_x <= 2); // One of 0, 1, or 2
    assert!((c.peak_value - 0.9).abs() < 1e-6);
}

#[test]
fn test_extract_candidates_peak_at_corner() {
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

    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

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
fn test_extract_candidates_single_pixel_component() {
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

    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

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
fn test_extract_candidates_diagonal_component() {
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

    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

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
fn test_extract_candidates_sparse_labels() {
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
    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

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
fn test_extract_candidates_full_image_component() {
    // Component covering entire image
    let pixels = Buffer2::new(3, 3, (0..9).map(|i| 0.1 + i as f32 * 0.1).collect());
    let label_map = LabelMap::from_raw(Buffer2::new(3, 3, vec![1u32; 9]), 1);

    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

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
fn test_extract_candidates_negative_pixel_values() {
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

    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

    assert_eq!(candidates.len(), 1);
    let c = &candidates[0];
    // Peak is at (1, 1) with value 0.3
    assert_eq!(c.peak_x, 1);
    assert_eq!(c.peak_y, 1);
    assert!((c.peak_value - 0.3).abs() < 1e-6);
}

#[test]
fn test_extract_candidates_many_components() {
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
    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

    assert_eq!(candidates.len(), 10);
    for (i, c) in candidates.iter().enumerate() {
        assert_eq!(c.area, 1);
        assert_eq!(c.peak_x, i);
        assert_eq!(c.peak_y, i);
    }
}

#[test]
fn test_extract_candidates_non_square_image() {
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

    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

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

// =============================================================================
// Integration Tests for Connected Components + Extract Candidates
// =============================================================================

#[test]
fn test_connected_components_and_extract_integration() {
    // Create a 10x10 image with two star-like regions
    let mut mask = vec![false; 100];
    let mut pixels = vec![0.1f32; 100];

    // Star 1 at (2, 2) - 3x3 region
    for dy in 0..3 {
        for dx in 0..3 {
            let idx = (2 + dy) * 10 + (2 + dx);
            mask[idx] = true;
            pixels[idx] =
                0.5 + 0.4 * (1.0 - ((dx as f32 - 1.0).powi(2) + (dy as f32 - 1.0).powi(2)) / 2.0);
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
    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

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
fn test_connected_components_complex_merge() {
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

// =============================================================================
// Deblending Tests
// =============================================================================

#[test]
fn test_deblend_star_pair() {
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
    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

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
fn test_no_deblend_for_close_peaks() {
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
    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

    // Should NOT deblend - only one candidate because peaks are too close
    assert_eq!(
        candidates.len(),
        1,
        "Close peaks should not be deblended, got {} candidates",
        candidates.len()
    );
}

#[test]
fn test_deblend_respects_prominence() {
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
    let candidates = extract_candidates(&pixels, &label_map, &TEST_DEBLEND_CONFIG, TEST_MAX_AREA);

    // Should NOT deblend - secondary peak is not prominent enough
    assert_eq!(
        candidates.len(),
        1,
        "Non-prominent secondary peak should not cause deblending, got {} candidates",
        candidates.len()
    );
}

// =============================================================================
// Multi-Threshold Deblending Tests
// =============================================================================

#[test]
fn test_multi_threshold_deblend_star_pair() {
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
        multi_threshold: true,
        n_thresholds: 32,
        min_contrast: 0.005,
    };

    let pixels = Buffer2::new(width, height, pixels);
    let label_map = LabelMap::from_raw(Buffer2::new(width, height, labels_data), 1);
    let candidates = extract_candidates(&pixels, &label_map, &mt_config, TEST_MAX_AREA);

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
fn test_multi_threshold_vs_simple_deblend_consistency() {
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

    // Simple deblending
    let simple_config = DeblendConfig {
        min_separation: 3,
        min_prominence: 0.3,
        multi_threshold: false,
        n_thresholds: 32,
        min_contrast: 0.005,
    };
    let pixels = Buffer2::new(width, height, pixels);
    let label_map = LabelMap::from_raw(Buffer2::new(width, height, labels_data), 1);
    let simple_candidates = extract_candidates(&pixels, &label_map, &simple_config, TEST_MAX_AREA);

    // Multi-threshold deblending
    let mt_config = DeblendConfig {
        min_separation: 3,
        min_prominence: 0.3,
        multi_threshold: true,
        n_thresholds: 32,
        min_contrast: 0.005,
    };
    let mt_candidates = extract_candidates(&pixels, &label_map, &mt_config, TEST_MAX_AREA);

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
fn test_multi_threshold_deblend_high_contrast_disables() {
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
        multi_threshold: true,
        n_thresholds: 32,
        min_contrast: 1.0, // Disabled
    };

    let pixels = Buffer2::new(width, height, pixels);
    let label_map = LabelMap::from_raw(Buffer2::new(width, height, labels_data), 1);
    let candidates = extract_candidates(&pixels, &label_map, &config, TEST_MAX_AREA);

    // Should return single candidate (deblending disabled)
    assert_eq!(
        candidates.len(),
        1,
        "High min_contrast should disable multi-threshold deblending, got {} candidates",
        candidates.len()
    );
}

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
