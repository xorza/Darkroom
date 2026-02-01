//! Tests for morphological dilation.

use super::dilate_mask;
use crate::common::BitBuffer2;

// =============================================================================
// Basic Dilate Mask Tests
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
// Word Boundary Tests (critical for word-level bit operations)
// =============================================================================

#[test]
fn test_dilate_mask_width_64_exact_word() {
    // Exact single word width - pixel at bit 63
    let mut mask_data = vec![false; 64];
    mask_data[63] = true; // last bit in word
    let mask = BitBuffer2::from_slice(64, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(64, 1, false);
    dilate_mask(&mask, 2, &mut dilated);

    // Should expand left but not past edge
    assert!(dilated.get(61));
    assert!(dilated.get(62));
    assert!(dilated.get(63));
    assert!(!dilated.get(60)); // radius 2, so 61-63 only
}

#[test]
fn test_dilate_mask_width_65_crosses_word_boundary() {
    // Width 65 - crosses into second word
    // Pixel at position 63 should dilate into word 1
    let mut mask_data = vec![false; 65];
    mask_data[63] = true; // last bit in first word
    let mask = BitBuffer2::from_slice(65, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(65, 1, false);
    dilate_mask(&mask, 2, &mut dilated);

    // Should expand: 61, 62, 63, 64
    assert!(dilated.get(61));
    assert!(dilated.get(62));
    assert!(dilated.get(63));
    assert!(dilated.get(64)); // crosses into second word
    assert!(!dilated.get(60));
}

#[test]
fn test_dilate_mask_width_65_pixel_at_boundary() {
    // Pixel at position 64 (first bit of second word)
    let mut mask_data = vec![false; 65];
    mask_data[64] = true; // first bit in second word
    let mask = BitBuffer2::from_slice(65, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(65, 1, false);
    dilate_mask(&mask, 2, &mut dilated);

    // Should expand: 62, 63, 64
    assert!(dilated.get(62)); // crosses back into first word
    assert!(dilated.get(63));
    assert!(dilated.get(64));
    assert!(!dilated.get(61));
}

#[test]
fn test_dilate_mask_width_128_two_full_words() {
    // Two full words
    let mut mask_data = vec![false; 128];
    mask_data[64] = true; // first bit of second word
    let mask = BitBuffer2::from_slice(128, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(128, 1, false);
    dilate_mask(&mask, 3, &mut dilated);

    // Should expand: 61, 62, 63, 64, 65, 66, 67
    for i in 61..=67 {
        assert!(dilated.get(i), "Pixel {} should be set", i);
    }
    assert!(!dilated.get(60));
    assert!(!dilated.get(68));
}

#[test]
fn test_dilate_mask_width_128_pixel_at_word_end() {
    // Pixel at last bit of first word
    let mut mask_data = vec![false; 128];
    mask_data[63] = true;
    let mask = BitBuffer2::from_slice(128, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(128, 1, false);
    dilate_mask(&mask, 3, &mut dilated);

    // Should expand: 60, 61, 62, 63, 64, 65, 66
    for i in 60..=66 {
        assert!(dilated.get(i), "Pixel {} should be set", i);
    }
    assert!(!dilated.get(59));
    assert!(!dilated.get(67));
}

#[test]
fn test_dilate_mask_wide_image_200_pixels() {
    // Wide image spanning multiple words (200 pixels = 4 words)
    let width = 200;
    let mut mask_data = vec![false; width];
    mask_data[100] = true; // middle
    let mask = BitBuffer2::from_slice(width, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(width, 1, false);
    dilate_mask(&mask, 5, &mut dilated);

    // Should expand: 95-105
    for i in 95..=105 {
        assert!(dilated.get(i), "Pixel {} should be set", i);
    }
    assert!(!dilated.get(94));
    assert!(!dilated.get(106));
}

#[test]
fn test_dilate_mask_large_radius_64() {
    // Radius 64 - larger than a word
    let width = 200;
    let mut mask_data = vec![false; width];
    mask_data[100] = true;
    let mask = BitBuffer2::from_slice(width, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(width, 1, false);
    dilate_mask(&mask, 64, &mut dilated);

    // Should expand: 36-164
    for i in 36..=164 {
        assert!(dilated.get(i), "Pixel {} should be set", i);
    }
    assert!(!dilated.get(35));
    assert!(!dilated.get(165));
}

#[test]
fn test_dilate_mask_large_radius_70() {
    // Radius 70 - spans multiple words
    let width = 200;
    let mut mask_data = vec![false; width];
    mask_data[100] = true;
    let mask = BitBuffer2::from_slice(width, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(width, 1, false);
    dilate_mask(&mask, 70, &mut dilated);

    // Should expand: 30-170
    for i in 30..=170 {
        assert!(dilated.get(i), "Pixel {} should be set", i);
    }
    assert!(!dilated.get(29));
    assert!(!dilated.get(171));
}

#[test]
fn test_dilate_mask_vertical_word_boundary() {
    // 2D: 70 wide, 10 tall - pixel in middle
    let width = 70;
    let height = 10;
    let mut mask_data = vec![false; width * height];
    mask_data[5 * width + 35] = true; // center at (35, 5)
    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let mut dilated = BitBuffer2::new_filled(width, height, false);
    dilate_mask(&mask, 3, &mut dilated);

    // Check horizontal expansion
    for x in 32..=38 {
        assert!(dilated.get_xy(x, 5), "Pixel ({}, 5) should be set", x);
    }
    // Check vertical expansion
    for y in 2..=8 {
        assert!(dilated.get_xy(35, y), "Pixel (35, {}) should be set", y);
    }
    // Check corners of dilated square are set
    assert!(dilated.get_xy(32, 2));
    assert!(dilated.get_xy(38, 8));
}

#[test]
fn test_dilate_mask_multiple_pixels_across_words() {
    // Multiple pixels spanning word boundaries
    let width = 130;
    let mut mask_data = vec![false; width];
    mask_data[62] = true; // near end of word 0
    mask_data[66] = true; // near start of word 1
    let mask = BitBuffer2::from_slice(width, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(width, 1, false);
    dilate_mask(&mask, 2, &mut dilated);

    // 62 expands to 60-64, 66 expands to 64-68, merged: 60-68
    for i in 60..=68 {
        assert!(dilated.get(i), "Pixel {} should be set", i);
    }
    assert!(!dilated.get(59));
    assert!(!dilated.get(69));
}

#[test]
fn test_dilate_mask_width_1_single_column() {
    // Edge case: single column
    let mut mask_data = vec![false; 5];
    mask_data[2] = true;
    let mask = BitBuffer2::from_slice(1, 5, &mask_data);
    let mut dilated = BitBuffer2::new_filled(1, 5, false);
    dilate_mask(&mask, 1, &mut dilated);

    // Should expand vertically: rows 1, 2, 3
    assert!(!dilated.get(0));
    assert!(dilated.get(1));
    assert!(dilated.get(2));
    assert!(dilated.get(3));
    assert!(!dilated.get(4));
}

#[test]
fn test_dilate_mask_height_1_single_row() {
    // Edge case: single row, 100 pixels wide
    let width = 100;
    let mut mask_data = vec![false; width];
    mask_data[50] = true;
    let mask = BitBuffer2::from_slice(width, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(width, 1, false);
    dilate_mask(&mask, 5, &mut dilated);

    // Should only expand horizontally
    for i in 45..=55 {
        assert!(dilated.get(i), "Pixel {} should be set", i);
    }
    assert!(!dilated.get(44));
    assert!(!dilated.get(56));
}

#[test]
fn test_dilate_mask_all_pixels_set() {
    // All pixels already set - should remain all set
    let mask_data = vec![true; 100];
    let mask = BitBuffer2::from_slice(100, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(100, 1, false);
    dilate_mask(&mask, 5, &mut dilated);

    assert!(dilated.iter().all(|x| x));
}

#[test]
fn test_dilate_mask_alternating_bits() {
    // Alternating pattern - stress test for bit operations
    let width = 128;
    let mut mask_data = vec![false; width];
    for i in (0..width).step_by(2) {
        mask_data[i] = true;
    }
    let mask = BitBuffer2::from_slice(width, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(width, 1, false);
    dilate_mask(&mask, 1, &mut dilated);

    // With radius 1, alternating should fill everything
    assert!(
        dilated.iter().all(|x| x),
        "All pixels should be set after dilation"
    );
}

#[test]
fn test_dilate_mask_sparse_pattern_across_words() {
    // Sparse pattern with pixels in different words
    let width = 256;
    let mut mask_data = vec![false; width];
    mask_data[10] = true; // word 0
    mask_data[70] = true; // word 1
    mask_data[130] = true; // word 2
    mask_data[200] = true; // word 3
    let mask = BitBuffer2::from_slice(width, 1, &mask_data);
    let mut dilated = BitBuffer2::new_filled(width, 1, false);
    dilate_mask(&mask, 3, &mut dilated);

    // Check each pixel's expansion
    for center in [10, 70, 130, 200] {
        for offset in -3i32..=3 {
            let pos = (center as i32 + offset) as usize;
            if pos < width {
                assert!(
                    dilated.get(pos),
                    "Pixel {} should be set (center {})",
                    pos,
                    center
                );
            }
        }
    }
    // Check gaps
    assert!(!dilated.get(14)); // between 10 and 70
    assert!(!dilated.get(66));
}

#[test]
fn test_dilate_mask_2d_crosses_word_boundary() {
    // 2D image where dilation crosses word boundary in each row
    let width = 70; // 64 + 6
    let height = 5;
    let mut mask_data = vec![false; width * height];
    // Pixel at (63, 2) - last bit of first word in row 2
    mask_data[2 * width + 63] = true;
    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let mut dilated = BitBuffer2::new_filled(width, height, false);
    dilate_mask(&mask, 2, &mut dilated);

    // Check horizontal expansion crosses word boundary
    for x in 61..=65 {
        assert!(dilated.get_xy(x, 2), "Pixel ({}, 2) should be set", x);
    }
    // Check vertical expansion
    for y in 0..=4 {
        assert!(dilated.get_xy(63, y), "Pixel (63, {}) should be set", y);
    }
}

#[test]
fn test_dilate_mask_compare_with_naive() {
    // Compare optimized version with naive implementation
    let width = 100;
    let height = 50;
    let radius = 4;

    // Create random-ish pattern
    let mut mask_data = vec![false; width * height];
    for i in 0..mask_data.len() {
        mask_data[i] = (i * 7 + i / 13) % 11 == 0;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let mut dilated = BitBuffer2::new_filled(width, height, false);
    dilate_mask(&mask, radius, &mut dilated);

    // Naive verification
    for y in 0..height {
        for x in 0..width {
            let mut expected = false;
            // Check if any source pixel within radius is set
            for sy in y.saturating_sub(radius)..=(y + radius).min(height - 1) {
                for sx in x.saturating_sub(radius)..=(x + radius).min(width - 1) {
                    if mask.get_xy(sx, sy) {
                        expected = true;
                        break;
                    }
                }
                if expected {
                    break;
                }
            }
            assert_eq!(
                dilated.get_xy(x, y),
                expected,
                "Mismatch at ({}, {}): got {}, expected {}",
                x,
                y,
                dilated.get_xy(x, y),
                expected
            );
        }
    }
}
