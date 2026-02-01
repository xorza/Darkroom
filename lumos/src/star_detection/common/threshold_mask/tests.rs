//! Tests for threshold mask creation (packed BitBuffer2 version).

use super::{
    create_adaptive_threshold_mask, create_threshold_mask, create_threshold_mask_filtered,
};
use crate::common::{BitBuffer2, Buffer2};

/// Helper to create threshold mask for tests using packed version
fn create_threshold_mask_test(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma: f32,
    width: usize,
    height: usize,
) -> BitBuffer2 {
    let pixels = Buffer2::new(width, height, pixels.to_vec());
    let bg = Buffer2::new(width, height, bg.to_vec());
    let noise = Buffer2::new(width, height, noise.to_vec());
    let mut mask = BitBuffer2::new_filled(width, height, false);
    create_threshold_mask(&pixels, &bg, &noise, sigma, &mut mask);
    mask
}

/// Helper to create filtered threshold mask for tests
fn create_threshold_mask_filtered_test(
    filtered: &[f32],
    noise: &[f32],
    sigma: f32,
    width: usize,
    height: usize,
) -> BitBuffer2 {
    let filtered = Buffer2::new(width, height, filtered.to_vec());
    let noise = Buffer2::new(width, height, noise.to_vec());
    let mut mask = BitBuffer2::new_filled(width, height, false);
    create_threshold_mask_filtered(&filtered, &noise, sigma, &mut mask);
    mask
}

#[test]
fn test_threshold_mask_above() {
    let width = 10;
    let height = 1;
    let pixels = vec![100.0f32; width * height];
    let bg = vec![50.0f32; width * height];
    let noise = vec![10.0f32; width * height];
    // threshold = 50 + 3 * 10 = 80, pixels = 100 > 80
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, width, height);
    assert!(mask.iter().all(|v| v));
}

#[test]
fn test_threshold_mask_below() {
    let width = 10;
    let height = 1;
    let pixels = vec![60.0f32; width * height];
    let bg = vec![50.0f32; width * height];
    let noise = vec![10.0f32; width * height];
    // threshold = 50 + 3 * 10 = 80, pixels = 60 < 80
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, width, height);
    assert!(mask.iter().all(|v| !v));
}

#[test]
fn test_threshold_mask_filtered() {
    let width = 10;
    let height = 1;
    let filtered = vec![50.0f32; width * height];
    let noise = vec![10.0f32; width * height];
    // threshold = 3 * 10 = 30, filtered = 50 > 30
    let mask = create_threshold_mask_filtered_test(&filtered, &noise, 3.0, width, height);
    assert!(mask.iter().all(|v| v));
}

#[test]
fn test_various_lengths() {
    // Test edge cases for SIMD remainder handling
    for len in [1, 3, 4, 5, 15, 16, 17, 31, 32, 33, 63, 64, 65, 100] {
        let width = len;
        let height = 1;
        let pixels = vec![100.0f32; width * height];
        let bg = vec![50.0f32; width * height];
        let noise = vec![10.0f32; width * height];
        let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, width, height);
        assert!(mask.iter().all(|v| v), "failed for len={}", len);
    }
}

#[test]
fn test_all_below() {
    let pixels = vec![0.5f32; 4];
    let bg = vec![1.0f32; 4];
    let noise = vec![0.1f32; 4];
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 2, 2);
    assert!(mask.iter().all(|x| !x));
}

#[test]
fn test_all_above() {
    let pixels = vec![2.0f32; 4];
    let bg = vec![1.0f32; 4];
    let noise = vec![0.1f32; 4];
    // threshold = 1.0 + 3.0 * 0.1 = 1.3
    // pixels at 2.0 > 1.3, so all true
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 2, 2);
    assert!(mask.iter().all(|x| x));
}

#[test]
fn test_mixed() {
    let pixels = vec![1.0f32, 2.0, 0.5, 1.5];
    let bg = vec![1.0f32; 4];
    let noise = vec![0.1f32; 4];
    // threshold = 1.0 + 3.0 * 0.1 = 1.3
    // pixel 0: 1.0 <= 1.3 -> false
    // pixel 1: 2.0 > 1.3 -> true
    // pixel 2: 0.5 <= 1.3 -> false
    // pixel 3: 1.5 > 1.3 -> true
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 2, 2);

    assert!(!mask.get(0));
    assert!(mask.get(1));
    assert!(!mask.get(2));
    assert!(mask.get(3));
}

#[test]
fn test_variable_background() {
    let pixels = vec![1.5f32; 4];
    let bg = vec![1.0f32, 1.2, 1.4, 0.8];
    let noise = vec![0.1f32; 4];
    // thresholds: 1.3, 1.5, 1.7, 1.1
    // pixel 0: 1.5 > 1.3 -> true
    // pixel 1: 1.5 <= 1.5 -> false (not strictly greater)
    // pixel 2: 1.5 <= 1.7 -> false
    // pixel 3: 1.5 > 1.1 -> true
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 2, 2);

    assert!(mask.get(0));
    assert!(!mask.get(1));
    assert!(!mask.get(2));
    assert!(mask.get(3));
}

#[test]
fn test_zero_noise_uses_epsilon() {
    let pixels = vec![1.1f32, 0.9];
    let bg = vec![1.0f32; 2];
    let noise = vec![0.0f32; 2]; // Zero noise

    // With noise.max(1e-6), threshold ≈ 1.0 + 3.0 * 1e-6 ≈ 1.000003
    // pixel 0: 1.1 > 1.000003 -> true
    // pixel 1: 0.9 <= 1.000003 -> false
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 2, 1);

    assert!(mask.get(0));
    assert!(!mask.get(1));
}

#[test]
fn test_exact_threshold_is_false() {
    // Pixel exactly at threshold should NOT be detected (must be strictly greater)
    let pixels = vec![1.3f32, 1.30001];
    let bg = vec![1.0f32; 2];
    let noise = vec![0.1f32; 2];

    // threshold = 1.0 + 3.0 * 0.1 = 1.3
    // pixel 0: 1.3 is NOT > 1.3 -> false
    // pixel 1: 1.30001 > 1.3 -> true
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 2, 1);

    assert!(!mask.get(0), "Exact threshold value should be false");
    assert!(mask.get(1), "Just above threshold should be true");
}

#[test]
fn test_different_sigma_values() {
    let pixels = vec![1.5f32; 4];
    let bg = vec![1.0f32; 4];
    let noise = vec![0.1f32; 4];

    // sigma=3: threshold=1.3, 1.5 > 1.3 -> all true
    let mask_sigma3 = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 2, 2);
    assert!(mask_sigma3.iter().all(|x| x));

    // sigma=5: threshold=1.5, 1.5 is NOT > 1.5 -> all false
    let mask_sigma5 = create_threshold_mask_test(&pixels, &bg, &noise, 5.0, 2, 2);
    assert!(mask_sigma5.iter().all(|x| !x));

    // sigma=4: threshold=1.4, 1.5 > 1.4 -> all true
    let mask_sigma4 = create_threshold_mask_test(&pixels, &bg, &noise, 4.0, 2, 2);
    assert!(mask_sigma4.iter().all(|x| x));
}

#[test]
fn test_high_noise_region() {
    // High noise regions require higher pixel values
    let pixels = vec![2.0f32; 2];
    let bg = vec![1.0f32; 2];
    let noise = vec![0.1f32, 0.5]; // Second pixel has high noise

    // pixel 0: threshold = 1.0 + 3.0*0.1 = 1.3, 2.0 > 1.3 -> true
    // pixel 1: threshold = 1.0 + 3.0*0.5 = 2.5, 2.0 NOT > 2.5 -> false
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 2, 1);

    assert!(mask.get(0));
    assert!(
        !mask.get(1),
        "High noise region should require higher threshold"
    );
}

#[test]
fn test_remainder_handling() {
    // Test that remainder handling works correctly for all possible remainder sizes
    for remainder in 0..64 {
        let size = 128 + remainder; // 128 is cleanly divisible by 64
        let pixels: Vec<f32> = (0..size)
            .map(|i| if i % 2 == 0 { 2.0 } else { 0.5 })
            .collect();
        let bg = vec![1.0f32; size];
        let noise = vec![0.1f32; size];

        let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, size, 1);

        // Verify correctness: even indices should be true (2.0 > 1.3), odd should be false (0.5 < 1.3)
        for i in 0..size {
            let expected = i % 2 == 0;
            assert_eq!(
                mask.get(i),
                expected,
                "Index {} should be {} for size {}",
                i,
                expected,
                size
            );
        }
    }
}

#[test]
fn test_filtered_basic() {
    // filtered image is already background-subtracted, so threshold = sigma * noise
    let filtered = vec![0.2f32, 0.4, 0.6, 0.8];
    let noise = vec![0.1f32; 4];

    // threshold = 3.0 * 0.1 = 0.3
    // 0.2 <= 0.3 -> false
    // 0.4 > 0.3 -> true
    // 0.6 > 0.3 -> true
    // 0.8 > 0.3 -> true
    let mask = create_threshold_mask_filtered_test(&filtered, &noise, 3.0, 2, 2);

    assert!(!mask.get(0));
    assert!(mask.get(1));
    assert!(mask.get(2));
    assert!(mask.get(3));
}

#[test]
fn test_filtered_variable_noise() {
    let filtered = vec![0.5f32; 4];
    let noise = vec![0.1f32, 0.2, 0.3, 0.05];

    // thresholds: 0.3, 0.6, 0.9, 0.15
    // 0.5 > 0.3 -> true
    // 0.5 <= 0.6 -> false
    // 0.5 <= 0.9 -> false
    // 0.5 > 0.15 -> true
    let mask = create_threshold_mask_filtered_test(&filtered, &noise, 3.0, 2, 2);

    assert!(mask.get(0));
    assert!(!mask.get(1));
    assert!(!mask.get(2));
    assert!(mask.get(3));
}

#[test]
fn test_large_image() {
    // Test a realistic image size
    let width = 1024;
    let height = 1024;
    let size = width * height;

    let mut pixels = vec![0.5f32; size];
    let bg = vec![0.4f32; size];
    let noise = vec![0.1f32; size];

    // Set some pixels above threshold
    for i in (0..size).step_by(100) {
        pixels[i] = 1.0; // threshold = 0.4 + 3*0.1 = 0.7, so 1.0 > 0.7
    }

    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, width, height);

    // Verify the expected pixels are set
    for i in 0..size {
        let expected = i % 100 == 0;
        assert_eq!(mask.get(i), expected, "Index {} should be {}", i, expected);
    }
}

#[test]
fn test_negative_values() {
    // Test with negative pixel values (common in background-subtracted images)
    let pixels = vec![-0.5f32, 0.5, -1.0, 1.0];
    let bg = vec![0.0f32; 4];
    let noise = vec![0.1f32; 4];

    // threshold = 0.0 + 3.0 * 0.1 = 0.3
    // -0.5 <= 0.3 -> false
    // 0.5 > 0.3 -> true
    // -1.0 <= 0.3 -> false
    // 1.0 > 0.3 -> true
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 2, 2);

    assert!(!mask.get(0));
    assert!(mask.get(1));
    assert!(!mask.get(2));
    assert!(mask.get(3));
}

#[test]
fn test_negative_background() {
    // Negative background can occur in calibrated images
    let pixels = vec![0.0f32; 4];
    let bg = vec![-1.0f32, -0.5, 0.0, 0.5];
    let noise = vec![0.1f32; 4];

    // thresholds: -1.0 + 0.3 = -0.7, -0.5 + 0.3 = -0.2, 0.0 + 0.3 = 0.3, 0.5 + 0.3 = 0.8
    // 0.0 > -0.7 -> true
    // 0.0 > -0.2 -> true
    // 0.0 <= 0.3 -> false
    // 0.0 <= 0.8 -> false
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 2, 2);

    assert!(mask.get(0));
    assert!(mask.get(1));
    assert!(!mask.get(2));
    assert!(!mask.get(3));
}

#[test]
fn test_tiny_image_1x1() {
    let pixels = vec![2.0f32];
    let bg = vec![1.0f32];
    let noise = vec![0.1f32];

    // threshold = 1.0 + 3.0 * 0.1 = 1.3, pixel = 2.0 > 1.3
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 1, 1);
    assert!(mask.get(0));

    // Below threshold
    let pixels_below = vec![1.0f32];
    let mask_below = create_threshold_mask_test(&pixels_below, &bg, &noise, 3.0, 1, 1);
    assert!(!mask_below.get(0));
}

#[test]
fn test_tiny_image_2x2() {
    let pixels = vec![2.0f32, 1.0, 1.5, 0.5];
    let bg = vec![1.0f32; 4];
    let noise = vec![0.1f32; 4];

    // threshold = 1.3
    // 2.0 > 1.3 -> true
    // 1.0 <= 1.3 -> false
    // 1.5 > 1.3 -> true
    // 0.5 <= 1.3 -> false
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 2, 2);

    assert!(mask.get(0));
    assert!(!mask.get(1));
    assert!(mask.get(2));
    assert!(!mask.get(3));
}

#[test]
fn test_tiny_image_1xn() {
    // Single row images
    for width in 1..=10 {
        let pixels = vec![2.0f32; width];
        let bg = vec![1.0f32; width];
        let noise = vec![0.1f32; width];

        let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, width, 1);
        assert!(mask.iter().all(|v| v), "Failed for 1x{}", width);
    }
}

#[test]
fn test_tiny_image_nx1() {
    // Single column images
    for height in 1..=10 {
        let pixels = vec![2.0f32; height];
        let bg = vec![1.0f32; height];
        let noise = vec![0.1f32; height];

        let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 1, height);
        assert!(mask.iter().all(|v| v), "Failed for {}x1", height);
    }
}

#[test]
fn test_filtered_negative_values() {
    // Filtered images commonly have negative values where background was over-subtracted
    let filtered = vec![-0.5f32, 0.5, -0.1, 0.4];
    let noise = vec![0.1f32; 4];

    // threshold = 3.0 * 0.1 = 0.3
    // -0.5 <= 0.3 -> false
    // 0.5 > 0.3 -> true
    // -0.1 <= 0.3 -> false
    // 0.4 > 0.3 -> true
    let mask = create_threshold_mask_filtered_test(&filtered, &noise, 3.0, 2, 2);

    assert!(!mask.get(0));
    assert!(mask.get(1));
    assert!(!mask.get(2));
    assert!(mask.get(3));
}

#[test]
fn test_negative_noise_clamped() {
    // Negative noise should be clamped to epsilon (1e-6)
    let pixels = vec![1.1f32, 0.9];
    let bg = vec![1.0f32; 2];
    let noise = vec![-0.1f32; 2]; // Negative noise

    // With noise.max(1e-6), threshold ≈ 1.0 + 3.0 * 1e-6 ≈ 1.000003
    // 1.1 > 1.000003 -> true
    // 0.9 <= 1.000003 -> false
    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, 2, 1);

    assert!(mask.get(0));
    assert!(!mask.get(1));
}

// ============================================================================
// Adaptive threshold mask tests
// ============================================================================

/// Helper to create adaptive threshold mask for tests
fn create_adaptive_threshold_mask_test(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    adaptive_sigma: &[f32],
    width: usize,
    height: usize,
) -> BitBuffer2 {
    let pixels = Buffer2::new(width, height, pixels.to_vec());
    let bg = Buffer2::new(width, height, bg.to_vec());
    let noise = Buffer2::new(width, height, noise.to_vec());
    let adaptive_sigma = Buffer2::new(width, height, adaptive_sigma.to_vec());
    let mut mask = BitBuffer2::new_filled(width, height, false);
    create_adaptive_threshold_mask(&pixels, &bg, &noise, &adaptive_sigma, &mut mask);
    mask
}

#[test]
fn test_adaptive_threshold_uniform_sigma() {
    // With uniform adaptive_sigma, should behave like create_threshold_mask
    let width = 10;
    let height = 1;
    let pixels = vec![100.0f32; width * height];
    let bg = vec![50.0f32; width * height];
    let noise = vec![10.0f32; width * height];
    let adaptive_sigma = vec![3.0f32; width * height]; // uniform sigma = 3.0

    // threshold = 50 + 3 * 10 = 80, pixels = 100 > 80
    let mask =
        create_adaptive_threshold_mask_test(&pixels, &bg, &noise, &adaptive_sigma, width, height);
    assert!(mask.iter().all(|v| v));
}

#[test]
fn test_adaptive_threshold_variable_sigma() {
    // Different sigma values per pixel
    let pixels = vec![1.5f32; 4];
    let bg = vec![1.0f32; 4];
    let noise = vec![0.1f32; 4];
    // sigma values: 3.0, 5.0, 6.0, 2.0
    let adaptive_sigma = vec![3.0f32, 5.0, 6.0, 2.0];

    // thresholds: 1.0 + 3.0*0.1 = 1.3, 1.0 + 5.0*0.1 = 1.5, 1.0 + 6.0*0.1 = 1.6, 1.0 + 2.0*0.1 = 1.2
    // pixel 0: 1.5 > 1.3 -> true
    // pixel 1: 1.5 <= 1.5 -> false (not strictly greater)
    // pixel 2: 1.5 <= 1.6 -> false
    // pixel 3: 1.5 > 1.2 -> true
    let mask = create_adaptive_threshold_mask_test(&pixels, &bg, &noise, &adaptive_sigma, 2, 2);

    assert!(mask.get(0));
    assert!(!mask.get(1));
    assert!(!mask.get(2));
    assert!(mask.get(3));
}

#[test]
fn test_adaptive_threshold_high_sigma_suppresses_detection() {
    // High sigma in nebulous regions should suppress detections
    let pixels = vec![2.0f32; 4];
    let bg = vec![1.0f32; 4];
    let noise = vec![0.1f32; 4];
    // Low sigma (uniform sky) vs high sigma (nebula)
    let adaptive_sigma = vec![3.0f32, 3.0, 15.0, 15.0];

    // thresholds: 1.3, 1.3, 2.5, 2.5
    // pixels 0,1: 2.0 > 1.3 -> true (uniform sky, detected)
    // pixels 2,3: 2.0 <= 2.5 -> false (nebula, suppressed)
    let mask = create_adaptive_threshold_mask_test(&pixels, &bg, &noise, &adaptive_sigma, 2, 2);

    assert!(mask.get(0));
    assert!(mask.get(1));
    assert!(!mask.get(2));
    assert!(!mask.get(3));
}

#[test]
fn test_adaptive_threshold_matches_fixed_when_uniform() {
    // Adaptive mask with uniform sigma should match fixed threshold mask
    let width = 100;
    let height = 100;
    let size = width * height;

    let mut pixels = vec![0.0f32; size];
    let mut bg = vec![1.0f32; size];
    let mut noise = vec![0.1f32; size];

    for i in 0..size {
        pixels[i] = ((i * 17) % 100) as f32 / 50.0;
        bg[i] = 1.0 + ((i * 7) % 10) as f32 / 100.0;
        noise[i] = 0.05 + ((i * 3) % 10) as f32 / 100.0;
    }

    let sigma = 3.0;
    let adaptive_sigma = vec![sigma; size];

    // Create both masks
    let fixed_mask = create_threshold_mask_test(&pixels, &bg, &noise, sigma, width, height);
    let adaptive_mask =
        create_adaptive_threshold_mask_test(&pixels, &bg, &noise, &adaptive_sigma, width, height);

    // They should be identical
    for i in 0..size {
        assert_eq!(
            fixed_mask.get(i),
            adaptive_mask.get(i),
            "Mismatch at index {}: fixed={}, adaptive={}",
            i,
            fixed_mask.get(i),
            adaptive_mask.get(i)
        );
    }
}

#[test]
fn test_adaptive_threshold_various_lengths() {
    // Test edge cases for SIMD remainder handling with adaptive threshold
    for len in [1, 3, 4, 5, 15, 16, 17, 31, 32, 33, 63, 64, 65, 100] {
        let width = len;
        let height = 1;
        let pixels = vec![100.0f32; width * height];
        let bg = vec![50.0f32; width * height];
        let noise = vec![10.0f32; width * height];
        let adaptive_sigma = vec![3.0f32; width * height];

        let mask = create_adaptive_threshold_mask_test(
            &pixels,
            &bg,
            &noise,
            &adaptive_sigma,
            width,
            height,
        );
        assert!(mask.iter().all(|v| v), "failed for len={}", len);
    }
}

#[test]
fn test_adaptive_threshold_remainder_handling() {
    // Test that remainder handling works correctly for all possible remainder sizes
    for remainder in 0..64 {
        let size = 128 + remainder;
        // Alternating high/low sigma to create pattern
        let pixels = vec![1.5f32; size];
        let bg = vec![1.0f32; size];
        let noise = vec![0.1f32; size];
        // Even indices: low sigma (detected), odd indices: high sigma (not detected)
        let adaptive_sigma: Vec<f32> = (0..size)
            .map(|i| if i % 2 == 0 { 3.0 } else { 10.0 })
            .collect();

        // thresholds: even = 1.3 (1.5 > 1.3 -> true), odd = 2.0 (1.5 < 2.0 -> false)
        let mask =
            create_adaptive_threshold_mask_test(&pixels, &bg, &noise, &adaptive_sigma, size, 1);

        for i in 0..size {
            let expected = i % 2 == 0;
            assert_eq!(
                mask.get(i),
                expected,
                "Index {} should be {} for size {}",
                i,
                expected,
                size
            );
        }
    }
}

#[test]
fn test_adaptive_threshold_zero_noise() {
    let pixels = vec![1.1f32, 0.9];
    let bg = vec![1.0f32; 2];
    let noise = vec![0.0f32; 2]; // Zero noise
    let adaptive_sigma = vec![3.0f32; 2];

    // With noise.max(1e-6), threshold ≈ 1.0 + 3.0 * 1e-6 ≈ 1.000003
    let mask = create_adaptive_threshold_mask_test(&pixels, &bg, &noise, &adaptive_sigma, 2, 1);

    assert!(mask.get(0)); // 1.1 > 1.000003
    assert!(!mask.get(1)); // 0.9 <= 1.000003
}

// ============================================================================
// SIMD vs Scalar consistency tests
// ============================================================================

/// Reference scalar implementation for testing
fn scalar_threshold(pixels: &[f32], bg: &[f32], noise: &[f32], sigma: f32) -> Vec<bool> {
    pixels
        .iter()
        .zip(bg.iter())
        .zip(noise.iter())
        .map(|((&px, &b), &n)| {
            let threshold = b + sigma * n.max(1e-6);
            px > threshold
        })
        .collect()
}

#[test]
fn test_packed_matches_scalar() {
    let width = 100;
    let height = 100;
    let size = width * height;

    let mut pixels_data = vec![0.0f32; size];
    let mut bg_data = vec![1.0f32; size];
    let mut noise_data = vec![0.1f32; size];

    // Create some test pattern
    for i in 0..size {
        pixels_data[i] = ((i * 17) % 100) as f32 / 50.0;
        bg_data[i] = 1.0 + ((i * 7) % 10) as f32 / 100.0;
        noise_data[i] = 0.05 + ((i * 3) % 10) as f32 / 100.0;
    }

    let sigma = 3.0;

    // Compute with scalar reference
    let scalar_mask = scalar_threshold(&pixels_data, &bg_data, &noise_data, sigma);

    // Compute with packed BitBuffer2
    let pixels = Buffer2::new(width, height, pixels_data.clone());
    let bg = Buffer2::new(width, height, bg_data.clone());
    let noise = Buffer2::new(width, height, noise_data.clone());
    let mut packed_mask = BitBuffer2::new_filled(width, height, false);
    create_threshold_mask(&pixels, &bg, &noise, sigma, &mut packed_mask);

    // Compare results
    for (i, &scalar_val) in scalar_mask.iter().enumerate() {
        assert_eq!(
            scalar_val,
            packed_mask.get(i),
            "Mismatch at index {}: scalar={}, packed={}",
            i,
            scalar_val,
            packed_mask.get(i)
        );
    }
}

#[test]
fn test_packed_non_aligned_size() {
    // Test with size that doesn't align to 64 bits
    let width = 100;
    let height = 73; // 7300 pixels, not divisible by 64
    let size = width * height;

    let pixels = Buffer2::new_filled(width, height, 2.0f32); // All above threshold
    let bg = Buffer2::new_filled(width, height, 0.0f32);
    let noise = Buffer2::new_filled(width, height, 0.1f32);

    let mut mask = BitBuffer2::new_filled(width, height, false);
    create_threshold_mask(&pixels, &bg, &noise, 3.0, &mut mask);

    // All should be set
    assert_eq!(mask.count_ones(), size);
}
