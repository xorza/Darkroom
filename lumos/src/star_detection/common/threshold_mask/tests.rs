//! Tests for threshold mask creation.

use super::*;
use crate::testing::synthetic::background_map;

fn make_bg(bg: Vec<f32>, noise: Vec<f32>, width: usize, height: usize) -> BackgroundMap {
    BackgroundMap {
        background: Buffer2::new(width, height, bg),
        noise: Buffer2::new(width, height, noise),
    }
}

/// Helper to create threshold mask for tests
fn create_threshold_mask_test(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma: f32,
) -> Buffer2<bool> {
    let mut mask = Buffer2::new_filled(pixels.width(), pixels.height(), false);
    create_threshold_mask(pixels, background, sigma, &mut mask);
    mask
}

#[test]
fn test_threshold_mask_above() {
    let width = 10;
    let height = 1;
    let pixels = Buffer2::new_filled(width, height, 100.0);
    let background = BackgroundMap {
        background: Buffer2::new_filled(width, height, 50.0),
        noise: Buffer2::new_filled(width, height, 10.0),
    };
    // threshold = 50 + 3 * 10 = 80, pixels = 100 > 80
    let mask = create_threshold_mask_test(&pixels, &background, 3.0);
    assert!(mask.iter().all(|&v| v));
}

#[test]
fn test_threshold_mask_below() {
    let width = 10;
    let height = 1;
    let pixels = Buffer2::new_filled(width, height, 60.0);
    let background = BackgroundMap {
        background: Buffer2::new_filled(width, height, 50.0),
        noise: Buffer2::new_filled(width, height, 10.0),
    };
    // threshold = 50 + 3 * 10 = 80, pixels = 60 < 80
    let mask = create_threshold_mask_test(&pixels, &background, 3.0);
    assert!(mask.iter().all(|&v| !v));
}

#[test]
fn test_threshold_mask_filtered() {
    let width = 10;
    let height = 1;
    let filtered = Buffer2::new_filled(width, height, 50.0);
    let background = BackgroundMap {
        background: Buffer2::new_filled(width, height, 0.0), // not used
        noise: Buffer2::new_filled(width, height, 10.0),
    };
    let mut mask = Buffer2::new_filled(width, height, false);
    // threshold = 3 * 10 = 30, filtered = 50 > 30
    create_threshold_mask_filtered(&filtered, &background, 3.0, &mut mask);
    assert!(mask.iter().all(|&v| v));
}

#[test]
fn test_various_lengths() {
    // Test edge cases for SIMD remainder handling
    for len in [1, 3, 4, 5, 15, 16, 17, 31, 32, 33, 100] {
        let width = len;
        let height = 1;
        let pixels = Buffer2::new_filled(width, height, 100.0);
        let background = BackgroundMap {
            background: Buffer2::new_filled(width, height, 50.0),
            noise: Buffer2::new_filled(width, height, 10.0),
        };
        let mask = create_threshold_mask_test(&pixels, &background, 3.0);
        assert!(mask.iter().all(|&v| v), "failed for len={}", len);
    }
}

#[test]
fn test_all_below() {
    let pixels = Buffer2::new(2, 2, vec![0.5, 0.5, 0.5, 0.5]);
    let background = background_map::uniform(2, 2, 1.0, 0.1);
    let mask = create_threshold_mask_test(&pixels, &background, 3.0);

    assert!(mask.iter().all(|&x| !x));
}

#[test]
fn test_all_above() {
    let pixels = Buffer2::new(2, 2, vec![2.0, 2.0, 2.0, 2.0]);
    let background = background_map::uniform(2, 2, 1.0, 0.1);

    // threshold = 1.0 + 3.0 * 0.1 = 1.3
    // pixels at 2.0 > 1.3, so all true
    let mask = create_threshold_mask_test(&pixels, &background, 3.0);

    assert!(mask.iter().all(|&x| x));
}

#[test]
fn test_mixed() {
    let pixels = Buffer2::new(2, 2, vec![1.0, 2.0, 0.5, 1.5]);
    let background = background_map::uniform(2, 2, 1.0, 0.1);

    // threshold = 1.0 + 3.0 * 0.1 = 1.3
    // pixel 0: 1.0 <= 1.3 -> false
    // pixel 1: 2.0 > 1.3 -> true
    // pixel 2: 0.5 <= 1.3 -> false
    // pixel 3: 1.5 > 1.3 -> true
    let mask = create_threshold_mask_test(&pixels, &background, 3.0);

    assert!(!mask[0]);
    assert!(mask[1]);
    assert!(!mask[2]);
    assert!(mask[3]);
}

#[test]
fn test_variable_background() {
    let pixels = Buffer2::new(2, 2, vec![1.5, 1.5, 1.5, 1.5]);
    let background = make_bg(vec![1.0, 1.2, 1.4, 0.8], vec![0.1, 0.1, 0.1, 0.1], 2, 2);

    // thresholds: 1.3, 1.5, 1.7, 1.1
    // pixel 0: 1.5 > 1.3 -> true
    // pixel 1: 1.5 <= 1.5 -> false (not strictly greater)
    // pixel 2: 1.5 <= 1.7 -> false
    // pixel 3: 1.5 > 1.1 -> true
    let mask = create_threshold_mask_test(&pixels, &background, 3.0);

    assert!(mask[0]);
    assert!(!mask[1]);
    assert!(!mask[2]);
    assert!(mask[3]);
}

// =============================================================================
// Quick Benchmarks
// =============================================================================

mod quick_benches {
    use super::*;
    use ::bench::quick_bench;
    use std::hint::black_box;

    fn create_bench_data(size: usize) -> (Buffer2<f32>, BackgroundMap, Buffer2<bool>) {
        let mut pixels_data = vec![0.0f32; size];
        let mut bg = vec![1.0f32; size];
        let mut noise = vec![0.1f32; size];

        for i in 0..size {
            pixels_data[i] = ((i * 17) % 100) as f32 / 50.0;
            bg[i] = 1.0 + ((i * 7) % 10) as f32 / 100.0;
            noise[i] = 0.05 + ((i * 3) % 10) as f32 / 100.0;
        }

        let width = (size as f64).sqrt() as usize;
        let height = size / width;
        let actual_size = width * height;

        let pixels = Buffer2::new(width, height, pixels_data[..actual_size].to_vec());
        let background = BackgroundMap {
            background: Buffer2::new(width, height, bg[..actual_size].to_vec()),
            noise: Buffer2::new(width, height, noise[..actual_size].to_vec()),
        };
        let mask = Buffer2::new_filled(width, height, false);

        (pixels, background, mask)
    }

    #[quick_bench(warmup_iters = 3, iters = 10)]
    fn bench_threshold_mask_4k(b: ::bench::Bencher) {
        let (pixels, background, mut mask) = create_bench_data(4096 * 4096);

        b.bench_labeled("scalar", || {
            scalar::process_chunk_scalar::<true>(
                black_box(pixels.pixels()),
                black_box(background.background.pixels()),
                black_box(background.noise.pixels()),
                black_box(3.0),
                black_box(mask.pixels_mut()),
            );
        });

        b.bench_labeled("simd+parallel", || {
            create_threshold_mask(
                black_box(&pixels),
                black_box(&background),
                black_box(3.0),
                black_box(&mut mask),
            );
        });
    }
}

#[test]
fn test_zero_noise_uses_epsilon() {
    let pixels = Buffer2::new(2, 1, vec![1.1, 0.9]);
    let background = make_bg(vec![1.0, 1.0], vec![0.0, 0.0], 2, 1); // Zero noise

    // With noise.max(1e-6), threshold ≈ 1.0 + 3.0 * 1e-6 ≈ 1.000003
    // pixel 0: 1.1 > 1.000003 -> true
    // pixel 1: 0.9 <= 1.000003 -> false
    let mask = create_threshold_mask_test(&pixels, &background, 3.0);

    assert!(mask[0]);
    assert!(!mask[1]);
}

#[test]
fn test_exact_threshold_is_false() {
    // Pixel exactly at threshold should NOT be detected (must be strictly greater)
    let pixels = Buffer2::new(2, 1, vec![1.3, 1.30001]);
    let background = make_bg(vec![1.0, 1.0], vec![0.1, 0.1], 2, 1);

    // threshold = 1.0 + 3.0 * 0.1 = 1.3
    // pixel 0: 1.3 is NOT > 1.3 -> false
    // pixel 1: 1.30001 > 1.3 -> true
    let mask = create_threshold_mask_test(&pixels, &background, 3.0);

    assert!(!mask[0], "Exact threshold value should be false");
    assert!(mask[1], "Just above threshold should be true");
}

#[test]
fn test_different_sigma_values() {
    let pixels = Buffer2::new(2, 2, vec![1.5, 1.5, 1.5, 1.5]);
    let background = background_map::uniform(2, 2, 1.0, 0.1);

    // sigma=3: threshold=1.3, 1.5 > 1.3 -> all true
    let mask_sigma3 = create_threshold_mask_test(&pixels, &background, 3.0);
    assert!(mask_sigma3.iter().all(|&x| x));

    // sigma=5: threshold=1.5, 1.5 is NOT > 1.5 -> all false
    let mask_sigma5 = create_threshold_mask_test(&pixels, &background, 5.0);
    assert!(mask_sigma5.iter().all(|&x| !x));

    // sigma=4: threshold=1.4, 1.5 > 1.4 -> all true
    let mask_sigma4 = create_threshold_mask_test(&pixels, &background, 4.0);
    assert!(mask_sigma4.iter().all(|&x| x));
}

#[test]
fn test_high_noise_region() {
    // High noise regions require higher pixel values
    let pixels = Buffer2::new(2, 1, vec![2.0, 2.0]);
    let background = make_bg(vec![1.0, 1.0], vec![0.1, 0.5], 2, 1); // Second pixel has high noise

    // pixel 0: threshold = 1.0 + 3.0*0.1 = 1.3, 2.0 > 1.3 -> true
    // pixel 1: threshold = 1.0 + 3.0*0.5 = 2.5, 2.0 NOT > 2.5 -> false
    let mask = create_threshold_mask_test(&pixels, &background, 3.0);

    assert!(mask[0]);
    assert!(
        !mask[1],
        "High noise region should require higher threshold"
    );
}

// =============================================================================
// SIMD vs Scalar Consistency Tests
// =============================================================================

/// Helper to create threshold mask using scalar implementation
fn create_threshold_mask_scalar(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma: f32,
) -> Buffer2<bool> {
    let mut mask = Buffer2::new_filled(pixels.width(), pixels.height(), false);
    scalar::process_chunk_scalar::<true>(
        pixels.pixels(),
        background.background.pixels(),
        background.noise.pixels(),
        sigma,
        mask.pixels_mut(),
    );
    mask
}

/// Helper to create filtered threshold mask using scalar implementation
fn create_threshold_mask_filtered_scalar(
    filtered: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma: f32,
) -> Buffer2<bool> {
    let mut mask = Buffer2::new_filled(filtered.width(), filtered.height(), false);
    scalar::process_chunk_scalar::<false>(
        filtered.pixels(),
        background.background.pixels(),
        background.noise.pixels(),
        sigma,
        mask.pixels_mut(),
    );
    mask
}

/// Helper to create filtered threshold mask for tests (uses dispatch)
fn create_threshold_mask_filtered_test(
    filtered: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma: f32,
) -> Buffer2<bool> {
    let mut mask = Buffer2::new_filled(filtered.width(), filtered.height(), false);
    create_threshold_mask_filtered(filtered, background, sigma, &mut mask);
    mask
}

#[test]
fn test_simd_vs_scalar_consistency_small() {
    // Small input that fits in one SIMD vector
    let pixels = Buffer2::new(2, 2, vec![0.5, 1.5, 2.0, 0.8]);
    let background = background_map::uniform(2, 2, 1.0, 0.1);

    let dispatch_mask = create_threshold_mask_test(&pixels, &background, 3.0);
    let scalar_mask = create_threshold_mask_scalar(&pixels, &background, 3.0);

    assert_eq!(
        dispatch_mask, scalar_mask,
        "SIMD and scalar should match for small input"
    );
}

#[test]
fn test_simd_vs_scalar_consistency_unaligned() {
    // Input sizes that don't align with SIMD width (4) or unroll factor (16)
    for size in [1, 2, 3, 5, 7, 13, 15, 17, 19, 31, 33] {
        let pixels: Buffer2<f32> =
            Buffer2::new(size, 1, (0..size).map(|i| (i as f32) * 0.1).collect());
        let background = background_map::uniform(size, 1, 0.5, 0.1);

        let dispatch_mask = create_threshold_mask_test(&pixels, &background, 3.0);
        let scalar_mask = create_threshold_mask_scalar(&pixels, &background, 3.0);

        assert_eq!(
            dispatch_mask, scalar_mask,
            "SIMD and scalar should match for size {}",
            size
        );
    }
}

#[test]
fn test_simd_vs_scalar_consistency_large() {
    // Large input that exercises unrolled loop
    let size = 1024;
    let mut pixels_data = vec![0.0f32; size];
    let mut bg = vec![1.0f32; size];
    let mut noise = vec![0.1f32; size];

    // Create varied data
    for i in 0..size {
        pixels_data[i] = ((i * 17) % 100) as f32 / 50.0; // 0.0 to 2.0
        bg[i] = 1.0 + ((i * 7) % 10) as f32 / 100.0; // 1.0 to 1.1
        noise[i] = 0.05 + ((i * 3) % 10) as f32 / 100.0; // 0.05 to 0.15
    }

    let background = make_bg(bg, noise, size, 1);
    let pixels = Buffer2::new(size, 1, pixels_data);

    let dispatch_mask = create_threshold_mask_test(&pixels, &background, 3.0);
    let scalar_mask = create_threshold_mask_scalar(&pixels, &background, 3.0);

    assert_eq!(
        dispatch_mask, scalar_mask,
        "SIMD and scalar should match for large input"
    );
}

#[test]
fn test_simd_vs_scalar_consistency_filtered() {
    // Test filtered variant consistency
    let size = 100;
    let filtered = Buffer2::new(size, 1, (0..size).map(|i| (i as f32) * 0.05).collect());
    let background = background_map::uniform(size, 1, 0.0, 0.1); // background not used for filtered

    let dispatch_mask = create_threshold_mask_filtered_test(&filtered, &background, 3.0);
    let scalar_mask = create_threshold_mask_filtered_scalar(&filtered, &background, 3.0);

    assert_eq!(
        dispatch_mask, scalar_mask,
        "Filtered: SIMD and scalar should match"
    );
}

#[test]
fn test_simd_remainder_handling() {
    // Test that remainder handling works correctly for all possible remainder sizes
    for remainder in 0..16 {
        let size = 64 + remainder; // 64 is cleanly divisible by 16
        let pixels = Buffer2::new(
            size,
            1,
            (0..size)
                .map(|i| if i % 2 == 0 { 2.0 } else { 0.5 })
                .collect(),
        );
        let background = background_map::uniform(size, 1, 1.0, 0.1);

        let dispatch_mask = create_threshold_mask_test(&pixels, &background, 3.0);
        let scalar_mask = create_threshold_mask_scalar(&pixels, &background, 3.0);

        assert_eq!(
            dispatch_mask, scalar_mask,
            "Remainder {} should be handled correctly",
            remainder
        );

        // Verify correctness: even indices should be true (2.0 > 1.3), odd should be false (0.5 < 1.3)
        for (i, &val) in dispatch_mask.iter().enumerate() {
            let expected = i % 2 == 0;
            assert_eq!(val, expected, "Index {} should be {}", i, expected);
        }
    }
}

#[test]
fn test_filtered_basic() {
    // filtered image is already background-subtracted, so threshold = sigma * noise
    let filtered = Buffer2::new(2, 2, vec![0.2, 0.4, 0.6, 0.8]);
    let background = background_map::uniform(2, 2, 0.0, 0.1); // Not used

    // threshold = 3.0 * 0.1 = 0.3
    // 0.2 <= 0.3 -> false
    // 0.4 > 0.3 -> true
    // 0.6 > 0.3 -> true
    // 0.8 > 0.3 -> true
    let mask = create_threshold_mask_filtered_test(&filtered, &background, 3.0);

    assert!(!mask[0]);
    assert!(mask[1]);
    assert!(mask[2]);
    assert!(mask[3]);
}

#[test]
fn test_filtered_variable_noise() {
    let filtered = Buffer2::new(2, 2, vec![0.5, 0.5, 0.5, 0.5]);
    let background = make_bg(vec![0.0; 4], vec![0.1, 0.2, 0.3, 0.05], 2, 2);

    // thresholds: 0.3, 0.6, 0.9, 0.15
    // 0.5 > 0.3 -> true
    // 0.5 <= 0.6 -> false
    // 0.5 <= 0.9 -> false
    // 0.5 > 0.15 -> true
    let mask = create_threshold_mask_filtered_test(&filtered, &background, 3.0);

    assert!(mask[0]);
    assert!(!mask[1]);
    assert!(!mask[2]);
    assert!(mask[3]);
}
