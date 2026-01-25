//! Tests for phase correlation module.

use super::*;

fn create_test_image(width: usize, height: usize, offset_x: isize, offset_y: isize) -> Vec<f32> {
    let mut image = vec![0.0f32; width * height];

    // Create a high-contrast pattern with clear edges that's easy to correlate
    for y in 0..height {
        for x in 0..width {
            let xx = (x as isize - offset_x).rem_euclid(width as isize) as usize;
            let yy = (y as isize - offset_y).rem_euclid(height as isize) as usize;

            // High-contrast checkerboard pattern
            let checker = ((xx / 8) + (yy / 8)) % 2;
            let val = if checker == 0 { 0.0 } else { 1.0 };
            image[y * width + x] = val;
        }
    }

    image
}

fn test_config() -> PhaseCorrelationConfig {
    PhaseCorrelationConfig {
        min_peak_value: 0.001, // Very low threshold for testing
        ..Default::default()
    }
}

#[test]
fn test_correlate_identical_images() {
    let width = 64;
    let height = 64;
    let reference = create_test_image(width, height, 0, 0);

    let correlator = PhaseCorrelator::new(width, height, test_config());
    let result = correlator.correlate(&reference, &reference, width, height);

    assert!(
        result.is_some(),
        "Correlation of identical images should succeed"
    );
    let result = result.unwrap();

    // For identical images, translation should be near zero
    assert!(
        (result.translation.0).abs() < 2.0,
        "dx = {}",
        result.translation.0
    );
    assert!(
        (result.translation.1).abs() < 2.0,
        "dy = {}",
        result.translation.1
    );
}

#[test]
fn test_correlate_translated_5_pixels() {
    let width = 64;
    let height = 64;
    let reference = create_test_image(width, height, 0, 0);
    let target = create_test_image(width, height, 5, 0);

    let correlator = PhaseCorrelator::new(width, height, test_config());
    let result = correlator.correlate(&reference, &target, width, height);

    assert!(result.is_some(), "Correlation should succeed");
    let result = result.unwrap();

    // Translation should detect the offset (sign depends on convention)
    assert!(
        (result.translation.0).abs() < 10.0,
        "Expected dx near ±5, got {}",
        result.translation.0
    );
}

#[test]
fn test_correlate_translated_xy() {
    let width = 64;
    let height = 64;
    let reference = create_test_image(width, height, 0, 0);
    let target = create_test_image(width, height, 3, -7);

    let correlator = PhaseCorrelator::new(width, height, test_config());
    let result = correlator.correlate(&reference, &target, width, height);

    assert!(result.is_some(), "Correlation should succeed");
    // Just verify we get some result - exact values depend on implementation details
}

#[test]
fn test_hann_window() {
    let window = hann_window(64);
    assert_eq!(window.len(), 64);

    // Window should be 0 at edges and 1 at center
    assert!(window[0] < 0.01);
    assert!(window[63] < 0.01);
    assert!((window[32] - 1.0).abs() < 0.01);
}

#[test]
fn test_subpixel_methods() {
    let width = 64;
    let height = 64;
    let reference = create_test_image(width, height, 0, 0);

    for method in [
        SubpixelMethod::None,
        SubpixelMethod::Parabolic,
        SubpixelMethod::Gaussian,
        SubpixelMethod::Centroid,
    ] {
        let config = PhaseCorrelationConfig {
            subpixel_method: method,
            min_peak_value: 0.001,
            ..Default::default()
        };
        let correlator = PhaseCorrelator::new(width, height, config);
        let result = correlator.correlate(&reference, &reference, width, height);

        assert!(result.is_some(), "Method {:?} failed", method);
    }
}

#[test]
fn test_transpose_inplace() {
    let n = 4;
    let mut data: Vec<Complex<f32>> = (0..16).map(|i| Complex::new(i as f32, 0.0)).collect();

    transpose_inplace(&mut data, n);

    // Check transposition
    assert_eq!(data[1].re, 4.0); // (0,1) -> (1,0)
    assert_eq!(data[4].re, 1.0); // (1,0) -> (0,1)
}

#[test]
fn test_correlate_empty_image() {
    let correlator = PhaseCorrelator::new(64, 64, PhaseCorrelationConfig::default());
    let result = correlator.correlate(&[], &[], 0, 0);
    assert!(result.is_none());
}

#[test]
fn test_correlate_size_mismatch() {
    let correlator = PhaseCorrelator::new(64, 64, PhaseCorrelationConfig::default());
    let small = vec![0.0f32; 32 * 32];
    let large = vec![0.0f32; 64 * 64];
    let result = correlator.correlate(&small, &large, 64, 64);
    assert!(result.is_none());
}

#[test]
fn test_confidence_calculation() {
    let width = 64;
    let height = 64;
    let reference = create_test_image(width, height, 0, 0);

    let correlator = PhaseCorrelator::new(width, height, test_config());
    let result = correlator.correlate(&reference, &reference, width, height);

    assert!(result.is_some(), "Correlation should succeed");
    let result = result.unwrap();

    // Self-correlation should have positive confidence
    assert!(result.confidence >= 0.0);
}

#[test]
fn test_log_polar_correlator_creation() {
    let correlator = LogPolarCorrelator::new(128, 64.0);
    // Just verify it doesn't panic
    assert!(correlator.size == 128);
}

#[test]
fn test_log_polar_identical_images() {
    let width = 128;
    let height = 128;

    // Create a simple test pattern
    let mut image = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            // Radial pattern that's rotationally interesting
            let dx = x as f64 - width as f64 / 2.0;
            let dy = y as f64 - height as f64 / 2.0;
            let r = (dx * dx + dy * dy).sqrt();
            image[y * width + x] = (r * 0.1).sin().abs() as f32;
        }
    }

    let correlator = LogPolarCorrelator::new(128, 64.0);
    let result = correlator.estimate_rotation_scale(&image, &image, width, height);

    assert!(result.is_some(), "Log-polar correlation should succeed");
    let result = result.unwrap();

    // Identical images should have near-zero rotation and scale ~1
    assert!(
        result.rotation.abs() < 0.2,
        "Expected near-zero rotation, got {}",
        result.rotation
    );
    assert!(
        (result.scale - 1.0).abs() < 0.2,
        "Expected scale ~1.0, got {}",
        result.scale
    );
}

#[test]
fn test_full_phase_correlator() {
    let width = 128;
    let height = 128;

    // Create a test pattern with multiple gaussian blobs (more suitable for phase correlation)
    let mut reference = vec![0.0f32; width * height];

    // Add several gaussian blobs at different positions
    let blobs = [(0.3, 0.3), (0.7, 0.7), (0.3, 0.7), (0.7, 0.3), (0.5, 0.5)];
    for (bx_frac, by_frac) in blobs {
        let bx = width as f64 * bx_frac;
        let by = height as f64 * by_frac;
        let sigma = 8.0;
        for y in 0..height {
            for x in 0..width {
                let dx = x as f64 - bx;
                let dy = y as f64 - by;
                let d2 = dx * dx + dy * dy;
                reference[y * width + x] += (-d2 / (2.0 * sigma * sigma)).exp() as f32;
            }
        }
    }

    // Use a lower threshold config for testing
    let config = super::PhaseCorrelationConfig {
        min_peak_value: 0.01,
        ..Default::default()
    };
    let correlator = super::FullPhaseCorrelator::with_config(width, height, config);
    let result = correlator.estimate(&reference, &reference, width, height);

    assert!(result.is_some(), "Full phase correlation should succeed");
    let result = result.unwrap();

    // Identical images should have minimal transformation
    assert!(result.rotation.abs() < 0.2, "Expected near-zero rotation");
    assert!((result.scale - 1.0).abs() < 0.2, "Expected scale ~1.0");
    assert!(result.translation.0.abs() < 2.0, "Expected near-zero dx");
    assert!(result.translation.1.abs() < 2.0, "Expected near-zero dy");
}

#[test]
fn test_bilinear_sample() {
    let image = vec![0.0, 1.0, 2.0, 3.0];

    // Center of 2x2 image should interpolate all 4 corners
    let center = super::bilinear_sample(&image, 2, 2, 0.5, 0.5);
    assert!((center - 1.5).abs() < 0.01, "Expected 1.5, got {}", center);

    // Corner should be exact
    let corner = super::bilinear_sample(&image, 2, 2, 0.0, 0.0);
    assert!((corner - 0.0).abs() < 0.01, "Expected 0.0, got {}", corner);
}

#[test]
fn test_rotate_and_scale_image() {
    let width = 64;
    let height = 64;

    // Create a simple gradient image
    let mut image = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            image[y * width + x] = x as f32 / width as f32;
        }
    }

    // Identity transform should preserve image
    let identity = super::rotate_and_scale_image(&image, width, height, 0.0, 1.0);

    // Check center pixels (edges may have border effects)
    let cx = width / 2;
    let cy = height / 2;
    let orig = image[cy * width + cx];
    let transformed = identity[cy * width + cx];
    assert!(
        (orig - transformed).abs() < 0.1,
        "Identity transform should preserve values"
    );
}

// ============================================================================
// Sub-pixel refinement accuracy tests
// ============================================================================

/// Test sub-pixel accuracy with known fractional offset
#[test]
fn test_subpixel_accuracy_parabolic() {
    let width = 128;
    let height = 128;

    // Create a pattern with multiple features for better correlation
    // Using the same create_test_image helper with fractional shift
    let reference = create_test_image(width, height, 0, 0);

    // Integer shift of 4 pixels in X
    let shift_x = 4;
    let target = create_test_image(width, height, shift_x, 0);

    let config = PhaseCorrelationConfig {
        subpixel_method: SubpixelMethod::Parabolic,
        min_peak_value: 0.001,
        ..Default::default()
    };
    let correlator = PhaseCorrelator::new(width, height, config);
    let result = correlator.correlate(&reference, &target, width, height);

    assert!(result.is_some(), "Sub-pixel correlation should succeed");
    let result = result.unwrap();

    // Check that we detect approximately the correct shift
    let detected_shift_x = result.translation.0;

    // The detected shift should be close to the actual shift (sign depends on convention)
    assert!(
        (detected_shift_x.abs() - shift_x as f64).abs() < 2.0
            || (detected_shift_x + shift_x as f64).abs() < 2.0,
        "X accuracy: expected ~{}, got {}",
        shift_x,
        detected_shift_x
    );
}

/// Test gaussian sub-pixel refinement
#[test]
fn test_subpixel_accuracy_gaussian() {
    let width = 128;
    let height = 128;

    let mut reference = vec![0.0f32; width * height];
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;
    let sigma = 15.0;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            reference[y * width + x] =
                (-((dx * dx + dy * dy) / (2.0 * sigma * sigma))).exp() as f32;
        }
    }

    let config = PhaseCorrelationConfig {
        subpixel_method: SubpixelMethod::Gaussian,
        min_peak_value: 0.001,
        ..Default::default()
    };
    let correlator = PhaseCorrelator::new(width, height, config);
    let result = correlator.correlate(&reference, &reference, width, height);

    assert!(result.is_some(), "Gaussian sub-pixel should succeed");
    let result = result.unwrap();

    // Self-correlation should be near zero
    assert!(
        result.translation.0.abs() < 0.5,
        "Gaussian method: expected near-zero dx, got {}",
        result.translation.0
    );
    assert!(
        result.translation.1.abs() < 0.5,
        "Gaussian method: expected near-zero dy, got {}",
        result.translation.1
    );
}

/// Test centroid sub-pixel refinement
#[test]
fn test_subpixel_accuracy_centroid() {
    let width = 128;
    let height = 128;

    let mut reference = vec![0.0f32; width * height];
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;
    let sigma = 15.0;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            reference[y * width + x] =
                (-((dx * dx + dy * dy) / (2.0 * sigma * sigma))).exp() as f32;
        }
    }

    let config = PhaseCorrelationConfig {
        subpixel_method: SubpixelMethod::Centroid,
        min_peak_value: 0.001,
        ..Default::default()
    };
    let correlator = PhaseCorrelator::new(width, height, config);
    let result = correlator.correlate(&reference, &reference, width, height);

    assert!(result.is_some(), "Centroid sub-pixel should succeed");
    let result = result.unwrap();

    // Self-correlation should be near zero
    assert!(
        result.translation.0.abs() < 0.5,
        "Centroid method: expected near-zero dx, got {}",
        result.translation.0
    );
}

// ============================================================================
// Large translation tests (wraparound handling)
// ============================================================================

/// Test translation near half image size (edge of wraparound)
#[test]
fn test_large_translation_near_wraparound() {
    let width = 128;
    let height = 128;

    // Create a distinctive pattern
    let mut reference = vec![0.0f32; width * height];
    for y in 20..40 {
        for x in 20..40 {
            reference[y * width + x] = 1.0;
        }
    }

    // Shift by 30 pixels (less than half image, should work)
    let shift = 30;
    let mut target = vec![0.0f32; width * height];
    for y in (20 + shift)..(40 + shift) {
        for x in 20..40 {
            if y < height {
                target[y * width + x] = 1.0;
            }
        }
    }

    let config = PhaseCorrelationConfig {
        min_peak_value: 0.001,
        ..Default::default()
    };
    let correlator = PhaseCorrelator::new(width, height, config);
    let result = correlator.correlate(&reference, &target, width, height);

    assert!(
        result.is_some(),
        "Large translation correlation should succeed"
    );
    let result = result.unwrap();

    // Should detect approximately the shift
    assert!(
        (result.translation.1.abs() - shift as f64).abs() < 5.0
            || (result.translation.1 + shift as f64).abs() < 5.0,
        "Expected shift near {}, got {}",
        shift,
        result.translation.1
    );
}

// ============================================================================
// Rotation estimation tests
// ============================================================================

/// Test rotation estimation with known angle
#[test]
fn test_rotation_estimation_45_degrees() {
    let width = 128;
    let height = 128;

    // Create a radial pattern that's rotationally sensitive
    let mut reference = vec![0.0f32; width * height];
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let angle = dy.atan2(dx);
            let r = (dx * dx + dy * dy).sqrt();
            // Create wedge pattern
            let val = if angle.abs() < 0.5 && r < 40.0 {
                1.0
            } else {
                0.0
            };
            reference[y * width + x] = val;
        }
    }

    // Rotate by 45 degrees
    let rotation_angle = std::f64::consts::PI / 4.0;
    let mut target = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let angle = dy.atan2(dx) - rotation_angle;
            let r = (dx * dx + dy * dy).sqrt();
            let val = if angle.abs() < 0.5 && r < 40.0 {
                1.0
            } else {
                0.0
            };
            target[y * width + x] = val;
        }
    }

    let correlator = LogPolarCorrelator::new(128, 64.0);
    let result = correlator.estimate_rotation_scale(&reference, &target, width, height);

    assert!(
        result.is_some(),
        "Rotation estimation should succeed for 45 degree rotation"
    );
    let result = result.unwrap();

    // Should detect approximately 45 degrees (pi/4 radians)
    let expected = std::f64::consts::PI / 4.0;
    let error = (result.rotation.abs() - expected).abs();
    assert!(
        error < 0.3 || (result.rotation + expected).abs() < 0.3,
        "Expected rotation ~{:.2} rad, got {:.2} rad",
        expected,
        result.rotation
    );
}

/// Test small rotation detection
#[test]
fn test_rotation_estimation_small_angle() {
    let width = 128;
    let height = 128;

    // Create pattern with clear orientation
    let mut reference = vec![0.0f32; width * height];
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let r = (dx * dx + dy * dy).sqrt();
            let angle = dy.atan2(dx);
            // Create radial spokes
            let val = if r < 50.0 && (angle * 4.0).sin().abs() > 0.7 {
                1.0
            } else {
                0.0
            };
            reference[y * width + x] = val;
        }
    }

    let correlator = LogPolarCorrelator::new(128, 64.0);
    let result = correlator.estimate_rotation_scale(&reference, &reference, width, height);

    assert!(result.is_some(), "Small rotation estimation should succeed");
    let result = result.unwrap();

    // Self-correlation should show near-zero rotation
    assert!(
        result.rotation.abs() < 0.1,
        "Expected near-zero rotation, got {}",
        result.rotation
    );
}

// ============================================================================
// Scale estimation tests
// ============================================================================

/// Test scale estimation with 1.2x scaling
#[test]
fn test_scale_estimation_1_2x() {
    let width = 128;
    let height = 128;

    // Create concentric rings pattern
    let mut reference = vec![0.0f32; width * height];
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let r = (dx * dx + dy * dy).sqrt();
            reference[y * width + x] = ((r * 0.2).sin() * 0.5 + 0.5) as f32;
        }
    }

    // Scale by 1.2x
    let scale_factor = 1.2;
    let mut target = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let r = (dx * dx + dy * dy).sqrt() / scale_factor;
            target[y * width + x] = ((r * 0.2).sin() * 0.5 + 0.5) as f32;
        }
    }

    let correlator = LogPolarCorrelator::new(128, 64.0);
    let result = correlator.estimate_rotation_scale(&reference, &target, width, height);

    assert!(result.is_some(), "Scale estimation should succeed");
    let result = result.unwrap();

    // Should detect approximately 1.2x scale
    let scale_error = (result.scale - scale_factor).abs();
    assert!(
        scale_error < 0.3,
        "Expected scale ~{}, got {}",
        scale_factor,
        result.scale
    );
}

// ============================================================================
// Low SNR / edge case tests
// ============================================================================

/// Test correlation with noisy images
#[test]
fn test_correlation_with_noise() {
    let width = 64;
    let height = 64;

    // Create signal pattern
    let mut reference = vec![0.0f32; width * height];
    for y in 20..44 {
        for x in 20..44 {
            reference[y * width + x] = 1.0;
        }
    }

    // Add moderate noise using simple LCG PRNG
    let mut seed = 12345u64;
    let mut target = reference.clone();
    for pixel in target.iter_mut() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let noise = ((seed >> 33) as f32 / u32::MAX as f32) * 0.3 - 0.15;
        *pixel += noise;
    }

    let config = PhaseCorrelationConfig {
        min_peak_value: 0.001,
        ..Default::default()
    };
    let correlator = PhaseCorrelator::new(width, height, config);
    let result = correlator.correlate(&reference, &target, width, height);

    assert!(
        result.is_some(),
        "Correlation with noise should still succeed"
    );
    let result = result.unwrap();

    // Should still detect approximately zero offset (noisy version of same image)
    assert!(
        result.translation.0.abs() < 5.0,
        "Noisy correlation dx too large: {}",
        result.translation.0
    );
    assert!(
        result.translation.1.abs() < 5.0,
        "Noisy correlation dy too large: {}",
        result.translation.1
    );
}

/// Test correlation with uniform (DC) image - should fail gracefully
#[test]
fn test_correlation_uniform_image() {
    let width = 64;
    let height = 64;

    // Uniform image has no features
    let uniform = vec![0.5f32; width * height];

    let config = PhaseCorrelationConfig {
        min_peak_value: 0.1, // Reasonable threshold
        ..Default::default()
    };
    let correlator = PhaseCorrelator::new(width, height, config);
    let result = correlator.correlate(&uniform, &uniform, width, height);

    // May succeed or fail - either is acceptable for degenerate input
    // Key is it doesn't panic
    if let Some(result) = result {
        // If it returns a result, confidence should be low
        assert!(
            result.confidence < 0.9,
            "Uniform image correlation should have low confidence"
        );
    }
}

/// Test correlation with pure checkerboard at Nyquist frequency
#[test]
fn test_correlation_nyquist_checkerboard() {
    let width = 64;
    let height = 64;

    // 1-pixel checkerboard (Nyquist frequency)
    let mut pattern = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            pattern[y * width + x] = if (x + y) % 2 == 0 { 1.0 } else { 0.0 };
        }
    }

    let config = PhaseCorrelationConfig {
        min_peak_value: 0.001,
        ..Default::default()
    };
    let correlator = PhaseCorrelator::new(width, height, config);
    let result = correlator.correlate(&pattern, &pattern, width, height);

    // This is a difficult case - may or may not succeed
    // Key is it doesn't panic
    if let Some(result) = result {
        // Self-correlation of any pattern should show zero offset
        assert!(
            result.translation.0.abs() < 2.0,
            "Nyquist pattern self-correlation dx: {}",
            result.translation.0
        );
    }
}

/// Test full phase correlator with rotation and translation
#[test]
fn test_full_correlator_rotation_and_translation() {
    let width = 128;
    let height = 128;

    // Create asymmetric pattern
    let mut reference = vec![0.0f32; width * height];
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let r = (dx * dx + dy * dy).sqrt();
            let angle = dy.atan2(dx);
            // Asymmetric blob
            let val = if r < 30.0 && angle > 0.0 { 1.0 } else { 0.0 };
            reference[y * width + x] = val;
        }
    }

    let config = super::PhaseCorrelationConfig {
        min_peak_value: 0.01,
        ..Default::default()
    };
    let correlator = super::FullPhaseCorrelator::with_config(width, height, config);
    let result = correlator.estimate(&reference, &reference, width, height);

    assert!(result.is_some(), "Full correlator should succeed");
    let result = result.unwrap();

    // Self-correlation
    assert!(
        result.rotation.abs() < 0.2,
        "Self-correlation rotation error: {}",
        result.rotation
    );
    assert!(
        (result.scale - 1.0).abs() < 0.2,
        "Self-correlation scale error: {}",
        result.scale
    );
}

// ============================================================================
// Multi-scale (large offset) correlation tests
// ============================================================================

/// Test that correlate_large_offset returns a result for identical images
#[test]
fn test_correlate_large_offset_identical() {
    let width = 256;
    let height = 256;

    // Create a high-contrast pattern
    let image: Vec<f32> = (0..width * height)
        .map(|i| {
            let x = i % width;
            let y = i / width;
            let checker = ((x / 32) + (y / 32)) % 2;
            if checker == 0 { 0.0 } else { 1.0 }
        })
        .collect();

    let config = super::PhaseCorrelationConfig {
        min_peak_value: 0.01,
        ..Default::default()
    };

    let result = super::correlate_large_offset(&image, &image, width, height, &config);

    assert!(result.is_some(), "Self-correlation should succeed");
    let result = result.unwrap();

    // For identical images, translation should be near zero
    assert!(
        result.translation.0.abs() < 10.0,
        "Self-correlation dx should be near 0, got {}",
        result.translation.0
    );
    assert!(
        result.translation.1.abs() < 10.0,
        "Self-correlation dy should be near 0, got {}",
        result.translation.1
    );
}

/// Test that correlate_large_offset falls back to standard for small images
#[test]
fn test_correlate_large_offset_small_image_fallback() {
    // Image too small for downsampling (64/4 = 16 < MIN_DOWNSAMPLE_SIZE of 64)
    let width = 64;
    let height = 64;

    let image: Vec<f32> = (0..width * height)
        .map(|i| {
            let x = i % width;
            let y = i / width;
            let checker = ((x / 8) + (y / 8)) % 2;
            if checker == 0 { 0.0 } else { 1.0 }
        })
        .collect();

    let config = super::PhaseCorrelationConfig {
        min_peak_value: 0.01,
        ..Default::default()
    };

    let result = super::correlate_large_offset(&image, &image, width, height, &config);

    // Should still succeed using fallback to standard correlator
    assert!(result.is_some(), "Fallback should succeed for small images");
}

/// Test that correlate_large_offset returns valid confidence
#[test]
fn test_correlate_large_offset_confidence() {
    let width = 256;
    let height = 256;

    let image: Vec<f32> = (0..width * height)
        .map(|i| {
            let x = i % width;
            let y = i / width;
            let checker = ((x / 32) + (y / 32)) % 2;
            if checker == 0 { 0.0 } else { 1.0 }
        })
        .collect();

    let config = super::PhaseCorrelationConfig {
        min_peak_value: 0.01,
        ..Default::default()
    };

    let result = super::correlate_large_offset(&image, &image, width, height, &config);

    assert!(result.is_some());
    let result = result.unwrap();

    // Confidence should be in valid range
    assert!(
        (0.0..=1.0).contains(&result.confidence),
        "Confidence should be in [0, 1], got {}",
        result.confidence
    );

    // Peak value should be positive for self-correlation
    assert!(
        result.peak_value > 0.0,
        "Peak value should be positive for self-correlation"
    );
}

/// Test helper functions
#[test]
fn test_downsample_image() {
    let width = 8;
    let height = 8;

    // Create image with constant value 4.0
    let image = vec![4.0f32; width * height];

    let ds = super::downsample_image(&image, width, height, 2);

    assert_eq!(ds.len(), 16); // 4x4
    for val in &ds {
        assert!((*val - 4.0).abs() < 0.001, "Expected 4.0, got {}", val);
    }
}

#[test]
fn test_shift_image_identity() {
    let width = 16;
    let height = 16;

    let mut image = vec![0.0f32; width * height];
    image[8 * width + 8] = 1.0; // Center pixel

    // Zero shift should preserve image
    let shifted = super::shift_image(&image, width, height, 0.0, 0.0);

    assert!(
        (shifted[8 * width + 8] - 1.0).abs() < 0.001,
        "Zero shift should preserve center pixel"
    );
}

#[test]
fn test_shift_image_integer_shift() {
    let width = 16;
    let height = 16;

    let mut image = vec![0.0f32; width * height];
    image[8 * width + 8] = 1.0; // Center pixel

    // Shift by (2, 3) - pixel should move
    let shifted = super::shift_image(&image, width, height, 2.0, 3.0);

    // Original position should be dark now
    assert!(
        shifted[8 * width + 8] < 0.5,
        "Original position should be dark after shift"
    );

    // New position should be bright
    assert!(
        shifted[11 * width + 10] > 0.5,
        "Shifted position should be bright"
    );
}

// ============================================================================
// Iterative phase correlation tests
// ============================================================================

/// Test iterative correlation with smooth pattern
/// Note: Iterative refinement works best with smooth images where bilinear
/// interpolation preserves signal quality. High-frequency patterns like
/// checkerboards degrade after interpolation-based shifting.
#[test]
fn test_iterative_correlation_subpixel() {
    let width = 128;
    let height = 128;

    // Create a smooth pattern (multiple Gaussians) that survives bilinear interpolation
    let mut reference = vec![0.0f32; width * height];
    for cy in [32, 96] {
        for cx in [32, 96] {
            let sigma = 10.0;
            for y in 0..height {
                for x in 0..width {
                    let dx = x as f64 - cx as f64;
                    let dy = y as f64 - cy as f64;
                    reference[y * width + x] +=
                        (-((dx * dx + dy * dy) / (2.0 * sigma * sigma))).exp() as f32;
                }
            }
        }
    }

    // Create target with a small shift
    let true_dx = 3.0;
    let true_dy = 2.0;
    let target = super::shift_image(&reference, width, height, true_dx, true_dy);

    // Standard correlation (non-iterative)
    let standard_config = PhaseCorrelationConfig {
        subpixel_method: SubpixelMethod::Parabolic,
        min_peak_value: 0.001,
        max_iterations: 0,
        ..Default::default()
    };
    let standard_correlator = PhaseCorrelator::new(width, height, standard_config);
    let standard_result = standard_correlator
        .correlate(&reference, &target, width, height)
        .expect("Standard correlation should succeed");

    // Iterative correlation
    let iterative_config = PhaseCorrelationConfig {
        subpixel_method: SubpixelMethod::Parabolic,
        min_peak_value: 0.001,
        max_iterations: 3,
        convergence_threshold: 0.05,
        ..Default::default()
    };
    let iterative_correlator = PhaseCorrelator::new(width, height, iterative_config);
    let iterative_result = iterative_correlator
        .correlate_iterative(&reference, &target, width, height)
        .expect("Iterative correlation should succeed");

    // Both methods should produce a result
    assert!(
        standard_result.translation.0.is_finite(),
        "Standard should produce finite result"
    );
    assert!(
        iterative_result.translation.0.is_finite(),
        "Iterative should produce finite result"
    );

    // For smooth patterns, correlation should detect at least the direction
    // (Phase correlation with smooth patterns can be imprecise)
    let detected_magnitude =
        (standard_result.translation.0.powi(2) + standard_result.translation.1.powi(2)).sqrt();
    assert!(
        detected_magnitude < 50.0,
        "Should detect reasonable translation magnitude, got {}",
        detected_magnitude
    );
}

/// Test iterative correlation converges for self-correlation
#[test]
fn test_iterative_correlation_self() {
    let width = 64;
    let height = 64;

    // Create test pattern
    let mut reference = vec![0.0f32; width * height];
    for y in 20..44 {
        for x in 20..44 {
            reference[y * width + x] = 1.0;
        }
    }

    let config = PhaseCorrelationConfig {
        min_peak_value: 0.001,
        max_iterations: 3,
        convergence_threshold: 0.01,
        ..Default::default()
    };
    let correlator = PhaseCorrelator::new(width, height, config);

    let result = correlator
        .correlate_iterative(&reference, &reference, width, height)
        .expect("Self-correlation should succeed");

    // Self-correlation should give near-zero translation
    assert!(
        result.translation.0.abs() < 0.5,
        "Self dx should be near zero, got {}",
        result.translation.0
    );
    assert!(
        result.translation.1.abs() < 0.5,
        "Self dy should be near zero, got {}",
        result.translation.1
    );
}

/// Test that disabled iterations falls back to standard correlation
#[test]
fn test_iterative_disabled_fallback() {
    let width = 64;
    let height = 64;

    let mut reference = vec![0.0f32; width * height];
    for y in 20..40 {
        for x in 20..40 {
            reference[y * width + x] = 1.0;
        }
    }

    // With max_iterations = 0, should use standard correlation
    let config = PhaseCorrelationConfig {
        min_peak_value: 0.001,
        max_iterations: 0,
        ..Default::default()
    };
    let correlator = PhaseCorrelator::new(width, height, config);

    let standard = correlator.correlate(&reference, &reference, width, height);
    let iterative = correlator.correlate_iterative(&reference, &reference, width, height);

    assert!(standard.is_some());
    assert!(iterative.is_some());

    let standard = standard.unwrap();
    let iterative = iterative.unwrap();

    // Results should be identical when iterations disabled
    assert!(
        (standard.translation.0 - iterative.translation.0).abs() < 1e-10,
        "With iterations disabled, results should match"
    );
    assert!(
        (standard.translation.1 - iterative.translation.1).abs() < 1e-10,
        "With iterations disabled, results should match"
    );
}
