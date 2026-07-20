//! Tests for threshold mask creation (packed BitBuffer2 version).
//!
//! Test organization:
//! - Basic threshold tests: Core functionality for standard thresholding
//! - Edge cases: Boundary conditions, special values, tiny images
//! - SIMD validation: Remainder handling, alignment, SIMD vs scalar consistency
//! - Filtered threshold tests: Background-subtracted image thresholding
//! - Multi-row tests: 2D image patterns and row boundary handling

use crate::bit_buffer2::BitBuffer2;
use crate::stacking::star_detection::threshold_mask::{
    create_threshold_mask, create_threshold_mask_filtered,
};
use imaginarium::Buffer2;

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

/// Reference scalar implementation for testing SIMD correctness
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

/// Reference scalar implementation for filtered threshold
fn scalar_threshold_filtered(pixels: &[f32], noise: &[f32], sigma: f32) -> Vec<bool> {
    pixels
        .iter()
        .zip(noise.iter())
        .map(|(&px, &n)| {
            let threshold = sigma * n.max(1e-6);
            px > threshold
        })
        .collect()
}

#[derive(Debug)]
struct ThresholdMaskCase {
    name: &'static str,
    pixels: &'static [f32],
    background: &'static [f32],
    noise: &'static [f32],
    sigma: f32,
    width: usize,
    height: usize,
    expected: &'static [bool],
}

#[test]
fn test_threshold_mask_truth_table() {
    let cases = [
        ThresholdMaskCase {
            name: "standard_above",
            pixels: &[100.0; 4],
            background: &[50.0; 4],
            noise: &[10.0; 4],
            sigma: 3.0,
            width: 4,
            height: 1,
            expected: &[true; 4],
        },
        ThresholdMaskCase {
            name: "standard_below",
            pixels: &[60.0; 4],
            background: &[50.0; 4],
            noise: &[10.0; 4],
            sigma: 3.0,
            width: 2,
            height: 2,
            expected: &[false; 4],
        },
        ThresholdMaskCase {
            name: "mixed",
            pixels: &[1.0, 2.0, 0.5, 1.5],
            background: &[1.0; 4],
            noise: &[0.1; 4],
            sigma: 3.0,
            width: 2,
            height: 2,
            expected: &[false, true, false, true],
        },
        ThresholdMaskCase {
            name: "variable_background",
            pixels: &[1.5; 4],
            background: &[1.0, 1.2, 1.4, 0.8],
            noise: &[0.1; 4],
            sigma: 3.0,
            width: 2,
            height: 2,
            expected: &[true, false, false, true],
        },
        ThresholdMaskCase {
            name: "high_noise_region",
            pixels: &[2.0; 2],
            background: &[1.0; 2],
            noise: &[0.1, 0.5],
            sigma: 3.0,
            width: 2,
            height: 1,
            expected: &[true, false],
        },
        ThresholdMaskCase {
            name: "negative_pixels",
            pixels: &[-0.5, 0.5, -1.0, 1.0],
            background: &[0.0; 4],
            noise: &[0.1; 4],
            sigma: 3.0,
            width: 2,
            height: 2,
            expected: &[false, true, false, true],
        },
        ThresholdMaskCase {
            name: "negative_background",
            pixels: &[0.0; 4],
            background: &[-1.0, -0.5, 0.0, 0.5],
            noise: &[0.1; 4],
            sigma: 3.0,
            width: 2,
            height: 2,
            expected: &[true, true, false, false],
        },
        ThresholdMaskCase {
            name: "zero_noise_uses_epsilon",
            pixels: &[1.1, 0.9],
            background: &[1.0; 2],
            noise: &[0.0; 2],
            sigma: 3.0,
            width: 2,
            height: 1,
            expected: &[true, false],
        },
        ThresholdMaskCase {
            name: "negative_noise_uses_epsilon",
            pixels: &[1.1, 0.9],
            background: &[1.0; 2],
            noise: &[-0.1; 2],
            sigma: 3.0,
            width: 2,
            height: 1,
            expected: &[true, false],
        },
        ThresholdMaskCase {
            name: "tiny_1x1_above",
            pixels: &[2.0],
            background: &[1.0],
            noise: &[0.1],
            sigma: 3.0,
            width: 1,
            height: 1,
            expected: &[true],
        },
        ThresholdMaskCase {
            name: "tiny_1x1_below",
            pixels: &[1.0],
            background: &[1.0],
            noise: &[0.1],
            sigma: 3.0,
            width: 1,
            height: 1,
            expected: &[false],
        },
        ThresholdMaskCase {
            name: "exact_threshold_is_false",
            pixels: &[1.3, 1.30001],
            background: &[1.0; 2],
            noise: &[0.1; 2],
            sigma: 3.0,
            width: 2,
            height: 1,
            expected: &[false, true],
        },
        ThresholdMaskCase {
            name: "sigma_3",
            pixels: &[1.5; 4],
            background: &[1.0; 4],
            noise: &[0.1; 4],
            sigma: 3.0,
            width: 2,
            height: 2,
            expected: &[true; 4],
        },
        ThresholdMaskCase {
            name: "sigma_5",
            pixels: &[1.5; 4],
            background: &[1.0; 4],
            noise: &[0.1; 4],
            sigma: 5.0,
            width: 2,
            height: 2,
            expected: &[false; 4],
        },
        ThresholdMaskCase {
            name: "sigma_4",
            pixels: &[1.5; 4],
            background: &[1.0; 4],
            noise: &[0.1; 4],
            sigma: 4.0,
            width: 2,
            height: 2,
            expected: &[true; 4],
        },
    ];
    let mut sigma_three = None;
    let mut sigma_five = None;

    for case in cases {
        let mask = create_threshold_mask_test(
            case.pixels,
            case.background,
            case.noise,
            case.sigma,
            case.width,
            case.height,
        );
        let actual: Vec<bool> = mask.iter().collect();

        assert_eq!(actual.as_slice(), case.expected, "{case:?}");
        assert_eq!(
            scalar_threshold(case.pixels, case.background, case.noise, case.sigma),
            case.expected,
            "scalar reference disagrees for {case:?}"
        );

        match case.name {
            "sigma_3" => sigma_three = Some(actual),
            "sigma_5" => sigma_five = Some(actual),
            _ => {}
        }
    }

    assert_ne!(sigma_three.unwrap(), sigma_five.unwrap());
}

#[derive(Debug)]
struct FilteredThresholdMaskCase {
    name: &'static str,
    pixels: &'static [f32],
    noise: &'static [f32],
    sigma: f32,
    width: usize,
    height: usize,
    expected: &'static [bool],
}

#[test]
fn test_filtered_threshold_mask_truth_table() {
    let cases = [
        FilteredThresholdMaskCase {
            name: "constant_above",
            pixels: &[50.0; 4],
            noise: &[10.0; 4],
            sigma: 3.0,
            width: 4,
            height: 1,
            expected: &[true; 4],
        },
        FilteredThresholdMaskCase {
            name: "mixed",
            pixels: &[0.2, 0.4, 0.6, 0.8],
            noise: &[0.1; 4],
            sigma: 3.0,
            width: 2,
            height: 2,
            expected: &[false, true, true, true],
        },
        FilteredThresholdMaskCase {
            name: "variable_noise",
            pixels: &[0.5; 4],
            noise: &[0.1, 0.2, 0.3, 0.05],
            sigma: 3.0,
            width: 2,
            height: 2,
            expected: &[true, false, false, true],
        },
        FilteredThresholdMaskCase {
            name: "negative_pixels",
            pixels: &[-0.5, 0.5, -0.1, 0.4],
            noise: &[0.1; 4],
            sigma: 3.0,
            width: 2,
            height: 2,
            expected: &[false, true, false, true],
        },
        FilteredThresholdMaskCase {
            name: "zero_noise_uses_epsilon",
            pixels: &[0.1, -0.1],
            noise: &[0.0; 2],
            sigma: 3.0,
            width: 2,
            height: 1,
            expected: &[true, false],
        },
    ];

    for case in cases {
        let mask = create_threshold_mask_filtered_test(
            case.pixels,
            case.noise,
            case.sigma,
            case.width,
            case.height,
        );
        let actual: Vec<bool> = mask.iter().collect();

        assert_eq!(actual.as_slice(), case.expected, "{}: {case:?}", case.name);
        assert_eq!(
            scalar_threshold_filtered(case.pixels, case.noise, case.sigma),
            case.expected,
            "scalar reference disagrees for {}: {case:?}",
            case.name
        );
    }
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

#[test]
fn test_filtered_matches_scalar() {
    let width = 100;
    let height = 100;
    let size = width * height;

    let mut pixels_data = vec![0.0f32; size];
    let mut noise_data = vec![0.1f32; size];

    for i in 0..size {
        pixels_data[i] = ((i * 17) % 100) as f32 / 100.0;
        noise_data[i] = 0.05 + ((i * 3) % 10) as f32 / 100.0;
    }

    let sigma = 3.0;

    // Compute with scalar reference
    let scalar_mask = scalar_threshold_filtered(&pixels_data, &noise_data, sigma);

    // Compute with packed BitBuffer2
    let mask = create_threshold_mask_filtered_test(&pixels_data, &noise_data, sigma, width, height);

    // Compare results
    for (i, &scalar_val) in scalar_mask.iter().enumerate() {
        assert_eq!(
            scalar_val,
            mask.get(i),
            "Filtered mismatch at index {}: scalar={}, packed={}",
            i,
            scalar_val,
            mask.get(i)
        );
    }
}

#[test]
fn test_multirow_checkerboard_pattern() {
    // Test 2D checkerboard pattern to verify row handling
    let width = 10;
    let height = 10;
    let size = width * height;

    let mut pixels = vec![0.5f32; size];
    let bg = vec![1.0f32; size];
    let noise = vec![0.1f32; size];

    // Create checkerboard: (x + y) % 2 == 0 -> above threshold
    for y in 0..height {
        for x in 0..width {
            if (x + y) % 2 == 0 {
                pixels[y * width + x] = 2.0; // Above threshold (1.3)
            }
        }
    }

    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, width, height);

    for y in 0..height {
        for x in 0..width {
            let expected = (x + y) % 2 == 0;
            assert_eq!(
                mask.get(y * width + x),
                expected,
                "Checkerboard mismatch at ({}, {})",
                x,
                y
            );
        }
    }
}

#[test]
fn test_multirow_horizontal_stripes() {
    // Test horizontal stripe pattern
    let width = 100;
    let height = 10;
    let size = width * height;

    let mut pixels = vec![0.5f32; size];
    let bg = vec![1.0f32; size];
    let noise = vec![0.1f32; size];

    // Even rows above threshold, odd rows below
    for y in 0..height {
        if y % 2 == 0 {
            for x in 0..width {
                pixels[y * width + x] = 2.0;
            }
        }
    }

    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, width, height);

    for y in 0..height {
        for x in 0..width {
            let expected = y % 2 == 0;
            assert_eq!(
                mask.get(y * width + x),
                expected,
                "Stripe mismatch at ({}, {})",
                x,
                y
            );
        }
    }
}

#[test]
fn test_multirow_vertical_stripes() {
    // Test vertical stripe pattern
    let width = 100;
    let height = 10;
    let size = width * height;

    let mut pixels = vec![0.5f32; size];
    let bg = vec![1.0f32; size];
    let noise = vec![0.1f32; size];

    // Even columns above threshold, odd columns below
    for y in 0..height {
        for x in 0..width {
            if x % 2 == 0 {
                pixels[y * width + x] = 2.0;
            }
        }
    }

    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, width, height);

    for y in 0..height {
        for x in 0..width {
            let expected = x % 2 == 0;
            assert_eq!(
                mask.get(y * width + x),
                expected,
                "Vertical stripe mismatch at ({}, {})",
                x,
                y
            );
        }
    }
}

#[test]
fn test_row_boundary_at_word_edge() {
    // Test where row width aligns exactly with 64-bit word boundary
    let width = 64;
    let height = 4;
    let size = width * height;

    let mut pixels = vec![0.5f32; size];
    let bg = vec![1.0f32; size];
    let noise = vec![0.1f32; size];

    // Set specific pixels at row boundaries
    pixels[63] = 2.0; // Last pixel of row 0
    pixels[64] = 2.0; // First pixel of row 1
    pixels[127] = 2.0; // Last pixel of row 1
    pixels[128] = 2.0; // First pixel of row 2

    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, width, height);

    assert!(mask.get(63), "Last pixel of row 0");
    assert!(mask.get(64), "First pixel of row 1");
    assert!(mask.get(127), "Last pixel of row 1");
    assert!(mask.get(128), "First pixel of row 2");

    // Verify neighbors are not set
    assert!(!mask.get(62));
    assert!(!mask.get(65));
    assert!(!mask.get(126));
    assert!(!mask.get(129));
}

#[test]
fn test_row_boundary_non_aligned() {
    // Test where row width doesn't align with word boundary
    let width = 70; // Not divisible by 64
    let height = 4;
    let size = width * height;

    let mut pixels = vec![0.5f32; size];
    let bg = vec![1.0f32; size];
    let noise = vec![0.1f32; size];

    // Set pixels at row boundaries
    pixels[69] = 2.0; // Last pixel of row 0
    pixels[70] = 2.0; // First pixel of row 1
    pixels[139] = 2.0; // Last pixel of row 1
    pixels[140] = 2.0; // First pixel of row 2

    let mask = create_threshold_mask_test(&pixels, &bg, &noise, 3.0, width, height);

    assert!(mask.get(69), "Last pixel of row 0");
    assert!(mask.get(70), "First pixel of row 1");
    assert!(mask.get(139), "Last pixel of row 1");
    assert!(mask.get(140), "First pixel of row 2");

    // Verify neighbors are not set
    assert!(!mask.get(68));
    assert!(!mask.get(71));
}

#[test]
fn test_filtered_remainder_handling() {
    // Test remainder handling for filtered variant
    for remainder in 0..64 {
        let size = 128 + remainder;
        let filtered: Vec<f32> = (0..size)
            .map(|i| if i % 2 == 0 { 0.5 } else { 0.1 })
            .collect();
        let noise = vec![0.1f32; size];

        // threshold = 3.0 * 0.1 = 0.3
        // even indices: 0.5 > 0.3 -> true
        // odd indices: 0.1 <= 0.3 -> false
        let mask = create_threshold_mask_filtered_test(&filtered, &noise, 3.0, size, 1);

        for i in 0..size {
            let expected = i % 2 == 0;
            assert_eq!(
                mask.get(i),
                expected,
                "Filtered remainder: index {} should be {} for size {}",
                i,
                expected,
                size
            );
        }
    }
}

#[test]
fn test_filtered_large_image() {
    let width = 512;
    let height = 512;
    let size = width * height;

    let mut filtered = vec![0.1f32; size];
    let noise = vec![0.1f32; size];

    // Set diagonal pixels above threshold
    for i in 0..width.min(height) {
        filtered[i * width + i] = 0.5; // threshold = 0.3, 0.5 > 0.3
    }

    let mask = create_threshold_mask_filtered_test(&filtered, &noise, 3.0, width, height);

    for y in 0..height {
        for x in 0..width {
            let expected = x == y && x < width.min(height);
            assert_eq!(
                mask.get(y * width + x),
                expected,
                "Filtered diagonal at ({}, {})",
                x,
                y
            );
        }
    }
}

/// The AVX2 packed kernel must produce bit-identical words to the scalar reference, including the
/// partial-word tail and `px == threshold` boundary pixels (strict `>`, so never set).
#[cfg(target_arch = "x86_64")]
#[test]
fn avx2_matches_scalar_packed() {
    use crate::stacking::star_detection::threshold_mask::avx2::process_words_avx2;
    use crate::stacking::star_detection::threshold_mask::process_words_scalar;

    if !imaginarium::cpu_features::has_avx2() {
        return; // backend not present on this host
    }

    // 130 px = 2 full 64-px words + a 2-bit partial word → exercises the scalar tail path.
    let n = 130;
    let sigma = 3.0f32;
    let mut pixels = vec![0.0f32; n];
    let mut bg = vec![0.0f32; n];
    let mut noise = vec![0.0f32; n];
    for i in 0..n {
        let h = (i as u32).wrapping_mul(2654435761);
        pixels[i] = (h as f32 / u32::MAX as f32) * 2.0; // [0, 2)
        bg[i] = 0.1 + (i % 5) as f32 * 0.05;
        noise[i] = 0.02 + (i % 7) as f32 * 0.01;
    }
    // Force exact-boundary pixels (px == bg + σ·noise) across a word edge and the tail.
    for &i in &[3usize, 63, 64, 70, 129] {
        pixels[i] = bg[i] + sigma * noise[i].max(1e-6);
    }

    let words_len = n.div_ceil(64);
    let mut w_avx = vec![0u64; words_len];
    let mut w_scalar = vec![0u64; words_len];

    unsafe { process_words_avx2::<true>(&pixels, &bg, &noise, sigma, &mut w_avx, 0, n) };
    process_words_scalar::<true>(&pixels, &bg, &noise, sigma, &mut w_scalar, 0, n);
    assert_eq!(w_avx, w_scalar, "AVX2 vs scalar mismatch (WITH_BG=true)");

    // Matched-filter case: WITH_BG=false, bg empty, threshold = σ·noise.
    w_avx.iter_mut().for_each(|w| *w = 0);
    w_scalar.iter_mut().for_each(|w| *w = 0);
    unsafe { process_words_avx2::<false>(&pixels, &[], &noise, sigma, &mut w_avx, 0, n) };
    process_words_scalar::<false>(&pixels, &[], &noise, sigma, &mut w_scalar, 0, n);
    assert_eq!(w_avx, w_scalar, "AVX2 vs scalar mismatch (WITH_BG=false)");
}
