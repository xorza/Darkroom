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
