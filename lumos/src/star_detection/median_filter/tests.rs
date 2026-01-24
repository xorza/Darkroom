//! Tests for 3x3 median filter.

use super::*;

#[test]
fn test_uniform_image() {
    let pixels = vec![0.5f32; 100 * 100];
    let result = median_filter_3x3(&pixels, 100, 100);

    for (i, &val) in result.iter().enumerate() {
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
    let mut pixels = vec![0.1f32; 25];
    pixels[12] = 1.0; // Center pixel

    let result = median_filter_3x3(&pixels, 5, 5);

    // Hot pixel should be replaced with median of neighbors (0.1)
    assert!(
        (result[12] - 0.1).abs() < 1e-6,
        "Hot pixel should be filtered to 0.1, got {}",
        result[12]
    );
}

#[test]
fn test_preserves_edges() {
    // Gradient image - edges should be mostly preserved
    let width = 10;
    let height = 10;
    let pixels: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 20.0))
        .collect();

    let result = median_filter_3x3(&pixels, width, height);

    // Check that general gradient direction is preserved
    assert!(result[0] < result[99], "Gradient should be preserved");
}

#[test]
fn test_small_image_2x2() {
    let pixels = vec![0.1, 0.2, 0.3, 0.4];
    let result = median_filter_3x3(&pixels, 2, 2);

    // Image too small, should return copy
    assert_eq!(result.len(), 4);
}

#[test]
fn test_small_image_1x1() {
    let pixels = vec![0.5];
    let result = median_filter_3x3(&pixels, 1, 1);

    assert_eq!(result.len(), 1);
    assert!((result[0] - 0.5).abs() < 1e-6);
}

#[test]
fn test_3x3_image() {
    // Exactly 3x3 - each pixel has different neighborhood size
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3,
        0.4, 0.5, 0.6,
        0.7, 0.8, 0.9,
    ];

    let result = median_filter_3x3(&pixels, 3, 3);

    // Center pixel has full 9-element neighborhood
    // Median of [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9] = 0.5
    assert!(
        (result[4] - 0.5).abs() < 1e-6,
        "Center median should be 0.5, got {}",
        result[4]
    );
}

#[test]
fn test_corner_pixels() {
    // 4x4 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ];

    let result = median_filter_3x3(&pixels, 4, 4);

    // Top-left corner has 4 neighbors: [0.1, 0.2, 0.5, 0.6]
    // Median of 4 = average of middle two = (0.2 + 0.5) / 2 = 0.35
    assert!(
        (result[0] - 0.35).abs() < 1e-6,
        "Top-left corner median should be 0.35, got {}",
        result[0]
    );
}

#[test]
fn test_edge_pixels() {
    // 4x4 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ];

    let result = median_filter_3x3(&pixels, 4, 4);

    // Top edge (1,0) has 6 neighbors: [0.1, 0.2, 0.3, 0.5, 0.6, 0.7]
    // Sorted: [0.1, 0.2, 0.3, 0.5, 0.6, 0.7]
    // Median of 6 = average of middle two = (0.3 + 0.5) / 2 = 0.4
    assert!(
        (result[1] - 0.4).abs() < 1e-6,
        "Top edge median should be 0.4, got {}",
        result[1]
    );
}

#[test]
fn test_salt_and_pepper_noise() {
    // Image with salt and pepper noise
    let mut pixels = vec![0.5f32; 100];
    // Add noise
    pixels[23] = 0.0; // pepper
    pixels[45] = 1.0; // salt
    pixels[67] = 0.0; // pepper
    pixels[89] = 1.0; // salt

    let result = median_filter_3x3(&pixels, 10, 10);

    // All noisy pixels should be close to 0.5 after filtering
    assert!(
        (result[23] - 0.5).abs() < 0.1,
        "Pepper noise should be filtered"
    );
    assert!(
        (result[45] - 0.5).abs() < 0.1,
        "Salt noise should be filtered"
    );
    assert!(
        (result[67] - 0.5).abs() < 0.1,
        "Pepper noise should be filtered"
    );
    assert!(
        (result[89] - 0.5).abs() < 0.1,
        "Salt noise should be filtered"
    );
}

#[test]
fn test_large_image_parallel() {
    // Test that parallel processing works correctly
    let width = 256;
    let height = 256;
    let pixels: Vec<f32> = (0..width * height)
        .map(|i| (i % 256) as f32 / 255.0)
        .collect();

    let result = median_filter_3x3(&pixels, width, height);

    assert_eq!(result.len(), width * height);

    // Check no NaN or Inf
    for (i, &val) in result.iter().enumerate() {
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
    let pixels = vec![0.5f32; width * height];

    let result = median_filter_3x3(&pixels, width, height);

    assert_eq!(result.len(), width * height);
}

#[test]
#[should_panic(expected = "Pixel count must match")]
fn test_wrong_pixel_count() {
    let pixels = vec![0.5f32; 100];
    median_filter_3x3(&pixels, 20, 10); // Expects 200 pixels
}

#[test]
fn test_bayer_pattern_removal() {
    // Simulate Bayer pattern with alternating row brightness
    let width = 10;
    let height = 10;
    let pixels: Vec<f32> = (0..height)
        .flat_map(|y| {
            let base = if y % 2 == 0 { 0.4 } else { 0.6 };
            (0..width).map(move |_| base)
        })
        .collect();

    let result = median_filter_3x3(&pixels, width, height);

    // After filtering, the pattern should be smoothed
    // Interior pixels should be close to 0.5
    let interior_mean: f32 =
        result[11..89].iter().filter(|&&v| v > 0.0).sum::<f32>() / result[11..89].len() as f32;

    assert!(
        (interior_mean - 0.5).abs() < 0.15,
        "Bayer pattern should be smoothed, got mean {}",
        interior_mean
    );
}
