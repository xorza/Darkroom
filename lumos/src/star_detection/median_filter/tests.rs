//! Tests for 3x3 median filter.

use super::*;
use crate::common::Buffer2;

#[test]
fn test_uniform_image() {
    let pixels = Buffer2::new_filled(100, 100, 0.5f32);
    let mut output = Buffer2::new_default(100, 100);
    median_filter_3x3(&pixels, &mut output);

    for (i, &val) in output.iter().enumerate() {
        assert!(
            (val - 0.5).abs() < 1e-6,
            "Pixel {} should be 0.5, got {}",
            i,
            val
        );
    }
}

#[test]
fn test_single_hot_pixel() {
    // 5x5 image with a hot pixel in center
    let mut pixels = Buffer2::new_filled(5, 5, 0.1f32);
    pixels[(2, 2)] = 1.0; // Center pixel

    let mut output = Buffer2::new_default(5, 5);
    median_filter_3x3(&pixels, &mut output);

    // Hot pixel should be replaced with median of neighbors (0.1)
    assert!(
        (output[(2, 2)] - 0.1).abs() < 1e-6,
        "Hot pixel should be filtered to 0.1, got {}",
        output[(2, 2)]
    );
}

#[test]
fn test_preserves_edges() {
    // Gradient image - edges should be mostly preserved
    let width = 10;
    let height = 10;
    let data: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 20.0))
        .collect();
    let pixels = Buffer2::new(width, height, data);

    let mut output = Buffer2::new_default(width, height);
    median_filter_3x3(&pixels, &mut output);

    // Check that general gradient direction is preserved
    assert!(output[0] < output[99], "Gradient should be preserved");
}

#[test]
fn test_small_image_2x2() {
    let pixels = Buffer2::new(2, 2, vec![0.1, 0.2, 0.3, 0.4]);
    let mut output = Buffer2::new_default(2, 2);
    median_filter_3x3(&pixels, &mut output);

    // Image too small, should return copy
    assert_eq!(output.len(), 4);
}

#[test]
fn test_small_image_1x1() {
    let pixels = Buffer2::new(1, 1, vec![0.5]);
    let mut output = Buffer2::new_default(1, 1);
    median_filter_3x3(&pixels, &mut output);

    assert_eq!(output.len(), 1);
    assert!((output[0] - 0.5).abs() < 1e-6);
}

#[test]
fn test_3x3_image() {
    // Exactly 3x3 - each pixel has different neighborhood size
    #[rustfmt::skip]
    let pixels = Buffer2::new(3, 3, vec![
        0.1, 0.2, 0.3,
        0.4, 0.5, 0.6,
        0.7, 0.8, 0.9,
    ]);

    let mut output = Buffer2::new_default(3, 3);
    median_filter_3x3(&pixels, &mut output);

    // Center pixel has full 9-element neighborhood
    // Median of [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9] = 0.5
    assert!(
        (output[(1, 1)] - 0.5).abs() < 1e-6,
        "Center median should be 0.5, got {}",
        output[(1, 1)]
    );
}

#[test]
fn test_corner_pixels() {
    // 4x4 image
    #[rustfmt::skip]
    let pixels = Buffer2::new(4, 4, vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ]);

    let mut output = Buffer2::new_default(4, 4);
    median_filter_3x3(&pixels, &mut output);

    // Top-left corner has 4 neighbors: [0.1, 0.2, 0.5, 0.6]
    // Median of 4 = average of middle two = (0.2 + 0.5) / 2 = 0.35
    assert!(
        (output[(0, 0)] - 0.35).abs() < 1e-6,
        "Top-left corner median should be 0.35, got {}",
        output[(0, 0)]
    );
}

#[test]
fn test_edge_pixels() {
    // 4x4 image
    #[rustfmt::skip]
    let pixels = Buffer2::new(4, 4, vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ]);

    let mut output = Buffer2::new_default(4, 4);
    median_filter_3x3(&pixels, &mut output);

    // Top edge (1,0) has 6 neighbors: [0.1, 0.2, 0.3, 0.5, 0.6, 0.7]
    // Sorted: [0.1, 0.2, 0.3, 0.5, 0.6, 0.7]
    // Median of 6 = average of middle two = (0.3 + 0.5) / 2 = 0.4
    assert!(
        (output[(1, 0)] - 0.4).abs() < 1e-6,
        "Top edge median should be 0.4, got {}",
        output[(1, 0)]
    );
}

#[test]
fn test_salt_and_pepper_noise() {
    // Image with salt and pepper noise
    let mut pixels = Buffer2::new_filled(10, 10, 0.5f32);
    // Add noise
    pixels[23] = 0.0; // pepper
    pixels[45] = 1.0; // salt
    pixels[67] = 0.0; // pepper
    pixels[89] = 1.0; // salt

    let mut output = Buffer2::new_default(10, 10);
    median_filter_3x3(&pixels, &mut output);

    // All noisy pixels should be close to 0.5 after filtering
    assert!(
        (output[23] - 0.5).abs() < 0.1,
        "Pepper noise should be filtered"
    );
    assert!(
        (output[45] - 0.5).abs() < 0.1,
        "Salt noise should be filtered"
    );
    assert!(
        (output[67] - 0.5).abs() < 0.1,
        "Pepper noise should be filtered"
    );
    assert!(
        (output[89] - 0.5).abs() < 0.1,
        "Salt noise should be filtered"
    );
}

#[test]
fn test_large_image_parallel() {
    // Test that parallel processing works correctly
    let width = 256;
    let height = 256;
    let data: Vec<f32> = (0..width * height)
        .map(|i| (i % 256) as f32 / 255.0)
        .collect();
    let pixels = Buffer2::new(width, height, data);

    let mut output = Buffer2::new_default(width, height);
    median_filter_3x3(&pixels, &mut output);

    assert_eq!(output.len(), width * height);

    // Check no NaN or Inf
    for (i, &val) in output.iter().enumerate() {
        assert!(val.is_finite(), "Pixel {} is not finite: {}", i, val);
        assert!(
            (0.0..=1.0).contains(&val),
            "Pixel {} out of range: {}",
            i,
            val
        );
    }
}

#[test]
fn test_median3() {
    let mut v = [0.3, 0.1, 0.2];
    assert!((median3(&mut v) - 0.2).abs() < 1e-6);

    let mut v = [0.1, 0.2, 0.3];
    assert!((median3(&mut v) - 0.2).abs() < 1e-6);

    let mut v = [0.3, 0.2, 0.1];
    assert!((median3(&mut v) - 0.2).abs() < 1e-6);
}

#[test]
fn test_median4() {
    let mut v = [0.4, 0.1, 0.3, 0.2];
    // Sorted: [0.1, 0.2, 0.3, 0.4], median = (0.2 + 0.3) / 2 = 0.25
    assert!((median4(&mut v) - 0.25).abs() < 1e-6);
}

#[test]
fn test_median5() {
    let mut v = [0.5, 0.1, 0.4, 0.2, 0.3];
    // Sorted: [0.1, 0.2, 0.3, 0.4, 0.5], median = 0.3
    assert!((median5(&mut v) - 0.3).abs() < 1e-6);
}

#[test]
fn test_median6() {
    let mut v = [0.6, 0.1, 0.5, 0.2, 0.4, 0.3];
    // Sorted: [0.1, 0.2, 0.3, 0.4, 0.5, 0.6], median = (0.3 + 0.4) / 2 = 0.35
    assert!((median6(&mut v) - 0.35).abs() < 1e-6);
}

#[test]
fn test_median9() {
    let mut v = [0.9, 0.1, 0.8, 0.2, 0.7, 0.3, 0.6, 0.4, 0.5];
    // Sorted: [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9], median = 0.5
    assert!((median9(&mut v) - 0.5).abs() < 1e-6);
}

#[test]
fn test_median9_already_sorted() {
    let mut v = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9];
    assert!((median9(&mut v) - 0.5).abs() < 1e-6);
}

#[test]
fn test_median9_reverse_sorted() {
    let mut v = [0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1];
    assert!((median9(&mut v) - 0.5).abs() < 1e-6);
}

#[test]
fn test_non_square_image() {
    let width = 20;
    let height = 10;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    let mut output = Buffer2::new_default(width, height);
    median_filter_3x3(&pixels, &mut output);

    assert_eq!(output.len(), width * height);
}

#[test]
#[should_panic(expected = "pixels length must equal width * height")]
fn test_wrong_pixel_count() {
    let pixels = Buffer2::new(20, 10, vec![0.5f32; 100]); // Expects 200 pixels
    let mut output = Buffer2::new_default(20, 10);
    median_filter_3x3(&pixels, &mut output);
}

#[test]
fn test_bayer_pattern_removal() {
    // Simulate Bayer pattern with alternating row brightness
    let width = 10;
    let height = 10;
    let data: Vec<f32> = (0..height)
        .flat_map(|y| {
            let base = if y % 2 == 0 { 0.4 } else { 0.6 };
            (0..width).map(move |_| base)
        })
        .collect();
    let pixels = Buffer2::new(width, height, data);

    let mut output = Buffer2::new_default(width, height);
    median_filter_3x3(&pixels, &mut output);

    // After filtering, the pattern should be smoothed
    // Interior pixels should be close to 0.5
    let interior_mean: f32 =
        output[11..89].iter().filter(|&&v| v > 0.0).sum::<f32>() / output[11..89].len() as f32;

    assert!(
        (interior_mean - 0.5).abs() < 0.15,
        "Bayer pattern should be smoothed, got mean {}",
        interior_mean
    );
}

// --- Tests for median_of_n ---

#[test]
fn test_median_of_n_empty() {
    let mut v: [f32; 0] = [];
    assert!((median_of_n(&mut v) - 0.0).abs() < 1e-6);
}

#[test]
fn test_median_of_n_single() {
    let mut v = [0.42];
    assert!((median_of_n(&mut v) - 0.42).abs() < 1e-6);
}

#[test]
fn test_median_of_n_two() {
    let mut v = [0.2, 0.8];
    // Average of two = 0.5
    assert!((median_of_n(&mut v) - 0.5).abs() < 1e-6);
}

#[test]
fn test_median_of_n_seven() {
    // 7 elements - uses fallback sort
    let mut v = [0.7, 0.1, 0.6, 0.2, 0.5, 0.3, 0.4];
    // Sorted: [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7], median = v[3] = 0.4
    assert!((median_of_n(&mut v) - 0.4).abs() < 1e-6);
}

#[test]
fn test_median_of_n_eight() {
    // 8 elements - uses fallback sort
    let mut v = [0.8, 0.1, 0.7, 0.2, 0.6, 0.3, 0.5, 0.4];
    // Sorted: [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8], median = v[4] = 0.5
    assert!((median_of_n(&mut v) - 0.5).abs() < 1e-6);
}

// --- Tests for median_at_left_edge ---

#[test]
fn test_median_at_left_edge() {
    // 5x5 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4, 0.5,
        0.6, 0.7, 0.8, 0.9, 1.0,
        1.1, 1.2, 1.3, 1.4, 1.5,
        1.6, 1.7, 1.8, 1.9, 2.0,
        2.1, 2.2, 2.3, 2.4, 2.5,
    ];

    // Left edge at y=1 (interior row), x=0
    // Neighbors: [0.1, 0.2, 0.6, 0.7, 1.1, 1.2]
    // Sorted: [0.1, 0.2, 0.6, 0.7, 1.1, 1.2]
    // Median of 6 = (0.6 + 0.7) / 2 = 0.65
    let result = median_at_left_edge(&pixels, 5, 1);
    assert!(
        (result - 0.65).abs() < 1e-6,
        "Left edge median should be 0.65, got {}",
        result
    );
}

// --- Tests for median_at_right_edge ---

#[test]
fn test_median_at_right_edge() {
    // 5x5 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4, 0.5,
        0.6, 0.7, 0.8, 0.9, 1.0,
        1.1, 1.2, 1.3, 1.4, 1.5,
        1.6, 1.7, 1.8, 1.9, 2.0,
        2.1, 2.2, 2.3, 2.4, 2.5,
    ];

    // Right edge at y=1 (interior row), x=4
    // Neighbors: [0.4, 0.5, 0.9, 1.0, 1.4, 1.5]
    // Sorted: [0.4, 0.5, 0.9, 1.0, 1.4, 1.5]
    // Median of 6 = (0.9 + 1.0) / 2 = 0.95
    let result = median_at_right_edge(&pixels, 5, 1);
    assert!(
        (result - 0.95).abs() < 1e-6,
        "Right edge median should be 0.95, got {}",
        result
    );
}

// --- Tests for median_at_edge ---

#[test]
fn test_median_at_edge_top_left_corner() {
    // 4x4 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ];

    // Top-left corner (0,0): neighbors [0.1, 0.2, 0.5, 0.6]
    // Median of 4 = (0.2 + 0.5) / 2 = 0.35
    let result = median_at_edge(&pixels, 4, 4, 0, 0);
    assert!(
        (result - 0.35).abs() < 1e-6,
        "Top-left corner should be 0.35, got {}",
        result
    );
}

#[test]
fn test_median_at_edge_top_right_corner() {
    // 4x4 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ];

    // Top-right corner (3,0): neighbors [0.3, 0.4, 0.7, 0.8]
    // Median of 4 = (0.4 + 0.7) / 2 = 0.55
    let result = median_at_edge(&pixels, 4, 4, 3, 0);
    assert!(
        (result - 0.55).abs() < 1e-6,
        "Top-right corner should be 0.55, got {}",
        result
    );
}

#[test]
fn test_median_at_edge_bottom_left_corner() {
    // 4x4 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ];

    // Bottom-left corner (0,3): neighbors [0.9, 1.0, 1.3, 1.4]
    // Median of 4 = (1.0 + 1.3) / 2 = 1.15
    let result = median_at_edge(&pixels, 4, 4, 0, 3);
    assert!(
        (result - 1.15).abs() < 1e-6,
        "Bottom-left corner should be 1.15, got {}",
        result
    );
}

#[test]
fn test_median_at_edge_bottom_right_corner() {
    // 4x4 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ];

    // Bottom-right corner (3,3): neighbors [1.1, 1.2, 1.5, 1.6]
    // Median of 4 = (1.2 + 1.5) / 2 = 1.35
    let result = median_at_edge(&pixels, 4, 4, 3, 3);
    assert!(
        (result - 1.35).abs() < 1e-6,
        "Bottom-right corner should be 1.35, got {}",
        result
    );
}

#[test]
fn test_median_at_edge_top_row() {
    // 4x4 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ];

    // Top row middle (1,0): neighbors [0.1, 0.2, 0.3, 0.5, 0.6, 0.7]
    // Median of 6 = (0.3 + 0.5) / 2 = 0.4
    let result = median_at_edge(&pixels, 4, 4, 1, 0);
    assert!(
        (result - 0.4).abs() < 1e-6,
        "Top row should be 0.4, got {}",
        result
    );
}

#[test]
fn test_median_at_edge_bottom_row() {
    // 4x4 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ];

    // Bottom row middle (1,3): neighbors [0.9, 1.0, 1.1, 1.3, 1.4, 1.5]
    // Median of 6 = (1.1 + 1.3) / 2 = 1.2
    let result = median_at_edge(&pixels, 4, 4, 1, 3);
    assert!(
        (result - 1.2).abs() < 1e-6,
        "Bottom row should be 1.2, got {}",
        result
    );
}

// --- Tests for simd::median9_scalar ---

#[test]
fn test_median9_scalar() {
    // Test the SIMD module's scalar median9 function
    let result = simd::median9_scalar(0.9, 0.1, 0.8, 0.2, 0.7, 0.3, 0.6, 0.4, 0.5);
    assert!(
        (result - 0.5).abs() < 1e-6,
        "median9_scalar should be 0.5, got {}",
        result
    );
}

#[test]
fn test_median9_scalar_all_same() {
    let result = simd::median9_scalar(0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5);
    assert!(
        (result - 0.5).abs() < 1e-6,
        "median9_scalar all same should be 0.5, got {}",
        result
    );
}

// --- Tests for filter_interior_row ---

#[test]
fn test_filter_interior_row() {
    // 5x5 image with known values
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.1, 0.1, 0.1, 0.1,
        0.1, 0.5, 0.5, 0.5, 0.1,
        0.1, 0.5, 1.0, 0.5, 0.1,  // Hot pixel in center
        0.1, 0.5, 0.5, 0.5, 0.1,
        0.1, 0.1, 0.1, 0.1, 0.1,
    ];

    let mut output_row = vec![0.0f32; 5];
    filter_interior_row(&pixels, 5, 2, &mut output_row);

    // Center pixel (x=2) should be filtered
    // Neighborhood: [0.5, 0.5, 0.5, 0.5, 1.0, 0.5, 0.5, 0.5, 0.5]
    // Median = 0.5
    assert!(
        (output_row[2] - 0.5).abs() < 1e-6,
        "Interior row center should be 0.5, got {}",
        output_row[2]
    );
}

// --- Tests for filter_edge_row ---

#[test]
fn test_filter_edge_row_top() {
    // 5x5 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4, 0.5,
        0.6, 0.7, 0.8, 0.9, 1.0,
        1.1, 1.2, 1.3, 1.4, 1.5,
        1.6, 1.7, 1.8, 1.9, 2.0,
        2.1, 2.2, 2.3, 2.4, 2.5,
    ];

    let mut output_row = vec![0.0f32; 5];
    filter_edge_row(&pixels, 5, 5, 0, &mut output_row);

    // Check first pixel (corner)
    // Neighbors: [0.1, 0.2, 0.6, 0.7] -> median = (0.2 + 0.6) / 2 = 0.4
    assert!(
        (output_row[0] - 0.4).abs() < 1e-6,
        "Top-left should be 0.4, got {}",
        output_row[0]
    );
}

#[test]
fn test_filter_edge_row_bottom() {
    // 5x5 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4, 0.5,
        0.6, 0.7, 0.8, 0.9, 1.0,
        1.1, 1.2, 1.3, 1.4, 1.5,
        1.6, 1.7, 1.8, 1.9, 2.0,
        2.1, 2.2, 2.3, 2.4, 2.5,
    ];

    let mut output_row = vec![0.0f32; 5];
    filter_edge_row(&pixels, 5, 5, 4, &mut output_row);

    // Check last pixel (corner)
    // Neighbors: [1.9, 2.0, 2.4, 2.5] -> median = (2.0 + 2.4) / 2 = 2.2
    assert!(
        (output_row[4] - 2.2).abs() < 1e-6,
        "Bottom-right should be 2.2, got {}",
        output_row[4]
    );
}

// --- Additional edge case tests ---

#[test]
fn test_4x4_all_corners_and_edges() {
    // Test all positions in a 4x4 image
    #[rustfmt::skip]
    let pixels = Buffer2::new(4, 4, vec![
        1.0, 2.0, 3.0, 4.0,
        5.0, 6.0, 7.0, 8.0,
        9.0, 10.0, 11.0, 12.0,
        13.0, 14.0, 15.0, 16.0,
    ]);

    let mut output = Buffer2::new_default(4, 4);
    median_filter_3x3(&pixels, &mut output);

    // All 16 pixels should be computed
    assert_eq!(output.len(), 16);

    // Check all values are finite and reasonable
    for (i, &val) in output.iter().enumerate() {
        assert!(val.is_finite(), "Pixel {} is not finite", i);
        assert!(
            (1.0..=16.0).contains(&val),
            "Pixel {} out of range: {}",
            i,
            val
        );
    }
}

#[test]
fn test_wide_image() {
    // Very wide, short image
    let width = 100;
    let height = 4;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    let mut output = Buffer2::new_default(width, height);
    median_filter_3x3(&pixels, &mut output);

    assert_eq!(output.len(), width * height);
    for &val in output.iter() {
        assert!((val - 0.5).abs() < 1e-6);
    }
}

#[test]
fn test_tall_image() {
    // Very tall, narrow image
    let width = 4;
    let height = 100;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    let mut output = Buffer2::new_default(width, height);
    median_filter_3x3(&pixels, &mut output);

    assert_eq!(output.len(), width * height);
    for &val in output.iter() {
        assert!((val - 0.5).abs() < 1e-6);
    }
}

#[test]
fn test_median3_all_permutations() {
    // Test all 6 permutations of [0.1, 0.2, 0.3]
    let permutations = [
        [0.1, 0.2, 0.3],
        [0.1, 0.3, 0.2],
        [0.2, 0.1, 0.3],
        [0.2, 0.3, 0.1],
        [0.3, 0.1, 0.2],
        [0.3, 0.2, 0.1],
    ];

    for perm in permutations {
        let mut v = perm;
        let result = median3(&mut v);
        assert!(
            (result - 0.2).abs() < 1e-6,
            "median3 of {:?} should be 0.2, got {}",
            perm,
            result
        );
    }
}

#[test]
fn test_median_with_duplicates() {
    // Test median functions with duplicate values
    let mut v3 = [0.5, 0.5, 0.5];
    assert!((median3(&mut v3) - 0.5).abs() < 1e-6);

    let mut v4 = [0.3, 0.3, 0.7, 0.7];
    assert!((median4(&mut v4) - 0.5).abs() < 1e-6);

    let mut v5 = [0.1, 0.5, 0.5, 0.5, 0.9];
    assert!((median5(&mut v5) - 0.5).abs() < 1e-6);

    let mut v6 = [0.1, 0.1, 0.5, 0.5, 0.9, 0.9];
    assert!((median6(&mut v6) - 0.5).abs() < 1e-6);

    let mut v9 = [0.1, 0.1, 0.1, 0.5, 0.5, 0.5, 0.9, 0.9, 0.9];
    assert!((median9(&mut v9) - 0.5).abs() < 1e-6);
}

#[test]
fn test_extreme_values() {
    // Test with extreme float values
    let mut v = [0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0];
    assert!((median9(&mut v) - 0.5).abs() < 1e-6);

    let mut v = [f32::MIN_POSITIVE, 0.5, 1.0 - f32::EPSILON];
    assert!((median3(&mut v) - 0.5).abs() < 1e-6);
}

#[test]
fn test_chunk_boundary() {
    // Test image height that's not a multiple of ROWS_PER_CHUNK (8)
    // This ensures chunk boundary handling is correct
    for height in [7, 9, 15, 17, 23, 25] {
        let width = 10;
        let pixels = Buffer2::new_filled(width, height, 0.5f32);

        let mut output = Buffer2::new_default(width, height);
        median_filter_3x3(&pixels, &mut output);

        assert_eq!(output.len(), width * height);
        for (i, &val) in output.iter().enumerate() {
            assert!(
                (val - 0.5).abs() < 1e-6,
                "Height {}: Pixel {} should be 0.5, got {}",
                height,
                i,
                val
            );
        }
    }
}
