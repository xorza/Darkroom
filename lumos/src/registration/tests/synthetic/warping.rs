//! Warping tests using synthetic star field images.
//!
//! These tests verify that warping an image with a known transform
//! and then aligning it back produces images that match.
//!
//! Tests cover:
//! - All TransformType variants (Translation, Euclidean, Similarity, Affine, Homography)
//! - All InterpolationMethod variants (Nearest, Bilinear, Bicubic, Lanczos2/3/4)

use crate::registration::interpolation::{InterpolationMethod, WarpConfig, warp_image};
use crate::registration::pipeline::warp_to_reference_image;
use crate::registration::types::{TransformMatrix, TransformType};
use crate::star_detection::StarDetector;
use crate::testing::synthetic::{self, StarFieldConfig};
use crate::{AstroImage, ImageDimensions};

/// Compute mean squared error between two images.
fn compute_mse(a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len());
    let sum: f64 = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| (*x as f64 - *y as f64).powi(2))
        .sum();
    sum / a.len() as f64
}

/// Compute peak signal-to-noise ratio (PSNR) between two images.
/// Higher is better. >40 dB is excellent, >30 dB is good.
fn compute_psnr(a: &[f32], b: &[f32], max_val: f32) -> f64 {
    let mse = compute_mse(a, b);
    if mse < 1e-10 {
        return f64::INFINITY;
    }
    10.0 * ((max_val as f64).powi(2) / mse).log10()
}

/// Compute normalized cross-correlation between two images.
/// Returns value in [-1, 1], where 1 means perfect correlation.
fn compute_ncc(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len() as f64;
    let mean_a: f64 = a.iter().map(|&x| x as f64).sum::<f64>() / n;
    let mean_b: f64 = b.iter().map(|&x| x as f64).sum::<f64>() / n;

    let mut cov = 0.0;
    let mut var_a = 0.0;
    let mut var_b = 0.0;

    for (&x, &y) in a.iter().zip(b.iter()) {
        let dx = x as f64 - mean_a;
        let dy = y as f64 - mean_b;
        cov += dx * dy;
        var_a += dx * dx;
        var_b += dy * dy;
    }

    if var_a < 1e-10 || var_b < 1e-10 {
        return 0.0;
    }

    cov / (var_a.sqrt() * var_b.sqrt())
}

/// Generate a synthetic star field for warping tests.
/// Uses smaller 256x256 images for faster tests.
fn generate_test_field(seed: u64) -> (Vec<f32>, usize, usize) {
    let config = StarFieldConfig {
        width: 256,
        height: 256,
        num_stars: 30,
        seed,
        ..synthetic::sparse_field_config()
    };
    let (pixels, _) = synthetic::generate_star_field(&config);
    (pixels, config.width, config.height)
}

/// All interpolation methods to test.
fn all_interpolation_methods() -> Vec<InterpolationMethod> {
    vec![
        InterpolationMethod::Nearest,
        InterpolationMethod::Bilinear,
        InterpolationMethod::Bicubic,
        InterpolationMethod::Lanczos2,
        InterpolationMethod::Lanczos3,
        InterpolationMethod::Lanczos4,
    ]
}

/// Representative interpolation methods for roundtrip tests.
/// Tests Nearest (baseline), Bilinear (fast), and Lanczos3 (quality).
fn representative_interpolation_methods() -> Vec<InterpolationMethod> {
    vec![
        InterpolationMethod::Nearest,
        InterpolationMethod::Bilinear,
        InterpolationMethod::Lanczos3,
    ]
}

/// Test that warping with identity transform preserves the image.
#[test]
fn test_warp_identity_all_methods() {
    let (ref_pixels, width, height) = generate_test_field(12345);

    let identity = TransformMatrix::identity();

    for method in all_interpolation_methods() {
        let config = WarpConfig {
            method,
            border_value: 0.0,
            normalize_kernel: true,
            clamp_output: false,
        };

        let warped = warp_image(
            &ref_pixels,
            width,
            height,
            width,
            height,
            &identity,
            &config,
        );

        let psnr = compute_psnr(&ref_pixels, &warped, 1.0);
        let ncc = compute_ncc(&ref_pixels, &warped);

        // Identity transform should produce nearly identical output
        // (Nearest should be exact, others very close)
        if method == InterpolationMethod::Nearest {
            assert!(
                psnr > 100.0 || psnr.is_infinite(),
                "{:?}: PSNR should be very high for identity, got {}",
                method,
                psnr
            );
        } else {
            assert!(
                psnr > 40.0,
                "{:?}: PSNR should be > 40 dB for identity, got {}",
                method,
                psnr
            );
        }
        assert!(
            ncc > 0.999,
            "{:?}: NCC should be > 0.999 for identity, got {}",
            method,
            ncc
        );
    }
}

// ============================================================================
// Translation tests
// ============================================================================

#[test]
fn test_warp_translation_roundtrip() {
    let (ref_pixels, width, height) = generate_test_field(11111);

    let dx = 10.5;
    let dy = -7.3;

    // Forward transform: moves image by (dx, dy)
    let forward = TransformMatrix::translation(dx, dy);
    // Inverse brings it back
    let inverse = forward.inverse();

    for method in representative_interpolation_methods() {
        let config = WarpConfig {
            method,
            border_value: 0.0,
            normalize_kernel: true,
            clamp_output: false,
        };

        // Warp forward, then inverse
        let warped = warp_image(&ref_pixels, width, height, width, height, &forward, &config);
        let restored = warp_image(&warped, width, height, width, height, &inverse, &config);

        // Compare central region (avoid border artifacts)
        let margin = 20;
        let (central_ref, central_restored) =
            extract_central_region(&ref_pixels, &restored, width, height, margin);

        let psnr = compute_psnr(&central_ref, &central_restored, 1.0);
        let ncc = compute_ncc(&central_ref, &central_restored);

        // Roundtrip warping introduces cumulative error from two interpolation passes.
        // Quality depends on method, but all lose some detail.
        // We use modest thresholds that verify the warp is working correctly.
        let (min_psnr, min_ncc) = match method {
            InterpolationMethod::Nearest => (15.0, 0.25),
            InterpolationMethod::Bilinear => (22.0, 0.70),
            InterpolationMethod::Bicubic => (25.0, 0.80),
            _ => (25.0, 0.80), // Lanczos variants
        };

        assert!(
            psnr > min_psnr,
            "Translation {:?}: PSNR {} < {} dB",
            method,
            psnr,
            min_psnr
        );
        assert!(
            ncc > min_ncc,
            "Translation {:?}: NCC {} < {}",
            method,
            ncc,
            min_ncc
        );
    }
}

// ============================================================================
// Euclidean (rotation) tests
// ============================================================================

#[test]
fn test_warp_euclidean_roundtrip() {
    let (ref_pixels, width, height) = generate_test_field(22222);

    let dx = 5.0;
    let dy = -3.0;
    let angle_rad = 2.0_f64.to_radians();

    let forward = TransformMatrix::euclidean(dx, dy, angle_rad);
    let inverse = forward.inverse();

    for method in representative_interpolation_methods() {
        let config = WarpConfig {
            method,
            border_value: 0.0,
            normalize_kernel: true,
            clamp_output: false,
        };

        let warped = warp_image(&ref_pixels, width, height, width, height, &forward, &config);
        let restored = warp_image(&warped, width, height, width, height, &inverse, &config);

        let margin = 30;
        let (central_ref, central_restored) =
            extract_central_region(&ref_pixels, &restored, width, height, margin);

        let psnr = compute_psnr(&central_ref, &central_restored, 1.0);
        let ncc = compute_ncc(&central_ref, &central_restored);

        // Note: Nearest neighbor has very poor sub-pixel accuracy
        let (min_psnr, min_ncc) = match method {
            InterpolationMethod::Nearest => (12.0, 0.25),
            InterpolationMethod::Bilinear => (25.0, 0.85),
            _ => (30.0, 0.90),
        };

        assert!(
            psnr > min_psnr,
            "Euclidean {:?}: PSNR {} < {} dB",
            method,
            psnr,
            min_psnr
        );
        assert!(
            ncc > min_ncc,
            "Euclidean {:?}: NCC {} < {}",
            method,
            ncc,
            min_ncc
        );
    }
}

// ============================================================================
// Similarity (rotation + scale) tests
// ============================================================================

#[test]
fn test_warp_similarity_roundtrip() {
    let (ref_pixels, width, height) = generate_test_field(33333);

    let dx = 8.0;
    let dy = -5.0;
    let angle_rad = 1.5_f64.to_radians();
    let scale = 1.02;

    let forward = TransformMatrix::similarity(dx, dy, angle_rad, scale);
    let inverse = forward.inverse();

    for method in representative_interpolation_methods() {
        let config = WarpConfig {
            method,
            border_value: 0.0,
            normalize_kernel: true,
            clamp_output: false,
        };

        let warped = warp_image(&ref_pixels, width, height, width, height, &forward, &config);
        let restored = warp_image(&warped, width, height, width, height, &inverse, &config);

        let margin = 40;
        let (central_ref, central_restored) =
            extract_central_region(&ref_pixels, &restored, width, height, margin);

        let psnr = compute_psnr(&central_ref, &central_restored, 1.0);
        let ncc = compute_ncc(&central_ref, &central_restored);

        let min_psnr = match method {
            InterpolationMethod::Nearest => 12.0,
            InterpolationMethod::Bilinear => 22.0,
            _ => 28.0,
        };

        assert!(
            psnr > min_psnr,
            "Similarity {:?}: PSNR {} < {} dB",
            method,
            psnr,
            min_psnr
        );
        assert!(ncc > 0.85, "Similarity {:?}: NCC {} < 0.85", method, ncc);
    }
}

// ============================================================================
// Affine tests
// ============================================================================

#[test]
fn test_warp_affine_roundtrip() {
    let (ref_pixels, width, height) = generate_test_field(44444);

    // Affine with slight differential scaling
    let scale_x = 1.01;
    let scale_y = 0.99;
    let angle_rad = 0.5_f64.to_radians();
    let dx = 6.0;
    let dy = -4.0;

    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();

    let params = [
        scale_x * cos_a,
        -scale_y * sin_a,
        dx,
        scale_x * sin_a,
        scale_y * cos_a,
        dy,
    ];
    let forward = TransformMatrix::affine(params);
    let inverse = forward.inverse();

    assert_eq!(forward.transform_type, TransformType::Affine);

    for method in representative_interpolation_methods() {
        let config = WarpConfig {
            method,
            border_value: 0.0,
            normalize_kernel: true,
            clamp_output: false,
        };

        let warped = warp_image(&ref_pixels, width, height, width, height, &forward, &config);
        let restored = warp_image(&warped, width, height, width, height, &inverse, &config);

        let margin = 40;
        let (central_ref, central_restored) =
            extract_central_region(&ref_pixels, &restored, width, height, margin);

        let psnr = compute_psnr(&central_ref, &central_restored, 1.0);
        let ncc = compute_ncc(&central_ref, &central_restored);

        let min_psnr = match method {
            InterpolationMethod::Nearest => 12.0,
            InterpolationMethod::Bilinear => 22.0,
            _ => 26.0,
        };

        assert!(
            psnr > min_psnr,
            "Affine {:?}: PSNR {} < {} dB",
            method,
            psnr,
            min_psnr
        );
        assert!(ncc > 0.85, "Affine {:?}: NCC {} < 0.85", method, ncc);
    }
}

// ============================================================================
// Homography tests
// ============================================================================

#[test]
fn test_warp_homography_roundtrip() {
    let (ref_pixels, width, height) = generate_test_field(55555);

    // Mild perspective distortion
    let dx = 5.0;
    let dy = -3.0;
    let h6 = 0.00005;
    let h7 = 0.00003;

    let params = [1.0, 0.0, dx, 0.0, 1.0, dy, h6, h7];
    let forward = TransformMatrix::homography(params);
    let inverse = forward.inverse();

    assert_eq!(forward.transform_type, TransformType::Homography);

    for method in representative_interpolation_methods() {
        let config = WarpConfig {
            method,
            border_value: 0.0,
            normalize_kernel: true,
            clamp_output: false,
        };

        let warped = warp_image(&ref_pixels, width, height, width, height, &forward, &config);
        let restored = warp_image(&warped, width, height, width, height, &inverse, &config);

        let margin = 50;
        let (central_ref, central_restored) =
            extract_central_region(&ref_pixels, &restored, width, height, margin);

        let psnr = compute_psnr(&central_ref, &central_restored, 1.0);
        let ncc = compute_ncc(&central_ref, &central_restored);

        let min_psnr = match method {
            InterpolationMethod::Nearest => 10.0,
            InterpolationMethod::Bilinear => 20.0,
            _ => 24.0,
        };

        assert!(
            psnr > min_psnr,
            "Homography {:?}: PSNR {} < {} dB",
            method,
            psnr,
            min_psnr
        );
        assert!(ncc > 0.80, "Homography {:?}: NCC {} < 0.80", method, ncc);
    }
}

// ============================================================================
// End-to-end: detect, register, warp, compare
// ============================================================================

#[test]
fn test_warp_with_detected_transform() {
    use crate::AstroImage;

    use crate::registration::{RegistrationConfig, Registrator};
    use crate::star_detection::StarDetectionConfig;

    let config = StarFieldConfig {
        width: 256,
        height: 256,
        num_stars: 40,
        seed: 66666,
        ..synthetic::sparse_field_config()
    };
    let width = config.width;
    let height = config.height;

    let (ref_pixels, _) = synthetic::generate_star_field(&config);

    // Apply a known transform
    let dx = 12.0;
    let dy = -8.0;
    let angle_rad = 0.8_f64.to_radians();

    let true_transform = TransformMatrix::euclidean(dx, dy, angle_rad);

    // Create target by warping reference
    let warp_config = WarpConfig {
        method: InterpolationMethod::Lanczos3,
        border_value: 0.0,
        normalize_kernel: true,
        clamp_output: false,
    };
    let target_pixels = warp_image(
        &ref_pixels,
        width,
        height,
        width,
        height,
        &true_transform,
        &warp_config,
    );

    // Detect stars in both images
    let det = StarDetector::from_config(StarDetectionConfig {
        expected_fwhm: 0.0,
        detection_sigma: 3.0,
        min_snr: 5.0,
        ..Default::default()
    });

    let ref_image =
        AstroImage::from_pixels(ImageDimensions::new(width, height, 1), ref_pixels.clone());
    let target_image = AstroImage::from_pixels(
        ImageDimensions::new(width, height, 1),
        target_pixels.clone(),
    );

    let ref_result = det.detect(&ref_image);
    let target_result = det.detect(&target_image);

    let ref_stars: Vec<(f64, f64)> = ref_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();
    let target_stars: Vec<(f64, f64)> = target_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();

    // Register to find transform
    let reg_config = RegistrationConfig::builder()
        .with_rotation()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(reg_config);
    let result = registrator
        .register_positions(&ref_stars, &target_stars)
        .expect("Registration should succeed");

    // Use warp_to_reference_image to align target back to reference frame
    let target_astro =
        AstroImage::from_pixels(ImageDimensions::new(width, height, 1), target_pixels);
    let aligned = warp_to_reference_image(
        &target_astro,
        &result.transform,
        InterpolationMethod::Lanczos3,
    );

    // Compare aligned image to reference
    let margin = 40;
    let (central_ref, central_aligned) =
        extract_central_region(&ref_pixels, aligned.channel(0), width, height, margin);

    let psnr = compute_psnr(&central_ref, &central_aligned, 1.0);
    let ncc = compute_ncc(&central_ref, &central_aligned);

    assert!(psnr > 25.0, "End-to-end alignment PSNR {} < 25 dB", psnr);
    assert!(ncc > 0.90, "End-to-end alignment NCC {} < 0.90", ncc);
}

// ============================================================================
// Quality comparison between methods
// ============================================================================

#[test]
fn test_interpolation_quality_ordering() {
    let (ref_pixels, width, height) = generate_test_field(77777);

    // Apply a transform that requires interpolation
    let forward = TransformMatrix::similarity(3.7, -2.3, 1.0_f64.to_radians(), 1.01);
    let inverse = forward.inverse();

    let mut results: Vec<(InterpolationMethod, f64)> = Vec::new();

    for method in all_interpolation_methods() {
        let config = WarpConfig {
            method,
            border_value: 0.0,
            normalize_kernel: true,
            clamp_output: false,
        };

        let warped = warp_image(&ref_pixels, width, height, width, height, &forward, &config);
        let restored = warp_image(&warped, width, height, width, height, &inverse, &config);

        let margin = 50;
        let (central_ref, central_restored) =
            extract_central_region(&ref_pixels, &restored, width, height, margin);

        let psnr = compute_psnr(&central_ref, &central_restored, 1.0);
        results.push((method, psnr));
    }

    // Print results for debugging
    for (method, psnr) in &results {
        println!("{:?}: {:.2} dB", method, psnr);
    }

    // For interpolating methods (not Nearest), quality generally increases:
    // Bilinear < Bicubic <= Lanczos
    //
    // Note: Nearest can appear to have high PSNR in roundtrip tests because
    // it doesn't blur, but it has terrible sub-pixel accuracy. We exclude it
    // from quality ordering comparisons.
    let bilinear_psnr = results
        .iter()
        .find(|(m, _)| *m == InterpolationMethod::Bilinear)
        .unwrap()
        .1;
    let bicubic_psnr = results
        .iter()
        .find(|(m, _)| *m == InterpolationMethod::Bicubic)
        .unwrap()
        .1;
    let lanczos3_psnr = results
        .iter()
        .find(|(m, _)| *m == InterpolationMethod::Lanczos3)
        .unwrap()
        .1;

    // Bicubic and Lanczos should be at least as good as bilinear
    assert!(
        bicubic_psnr >= bilinear_psnr - 2.0,
        "Bicubic ({:.1}) should be at least as good as Bilinear ({:.1})",
        bicubic_psnr,
        bilinear_psnr
    );
    assert!(
        lanczos3_psnr >= bilinear_psnr - 2.0,
        "Lanczos3 ({:.1}) should be at least as good as Bilinear ({:.1})",
        lanczos3_psnr,
        bilinear_psnr
    );
}

// ============================================================================
// Multi-channel image warping tests
// ============================================================================

#[test]
fn test_warp_to_reference_image_grayscale() {
    let (ref_pixels, width, height) = generate_test_field(88888);
    let ref_image = AstroImage::from_pixels(ImageDimensions::new(width, height, 1), ref_pixels);

    // Apply a translation
    let transform = TransformMatrix::translation(5.0, -3.0);

    // Warp the image
    let warped_image =
        warp_to_reference_image(&ref_image, &transform, InterpolationMethod::Lanczos3);

    // Verify dimensions and basic properties
    assert_eq!(warped_image.width(), width);
    assert_eq!(warped_image.height(), height);
    assert_eq!(warped_image.channels(), 1);
}

#[test]
fn test_warp_to_reference_image_rgb() {
    let (gray_pixels, width, height) = generate_test_field(99999);

    // Create RGB image by duplicating grayscale to all channels with slight offsets
    let mut rgb_pixels = Vec::with_capacity(width * height * 3);
    for (i, &val) in gray_pixels.iter().enumerate() {
        let y = i / width;
        let x = i % width;
        // Slightly different values per channel to verify independent processing
        rgb_pixels.push(val); // R
        rgb_pixels.push((val + 0.1).min(1.0)); // G
        rgb_pixels.push(if (x + y) % 2 == 0 { val } else { val * 0.8 }); // B
    }

    let rgb_image = AstroImage::from_pixels(ImageDimensions::new(width, height, 3), rgb_pixels);

    // Apply a transform
    let transform = TransformMatrix::euclidean(3.0, -2.0, 1.0_f64.to_radians());

    // Warp the RGB image
    let warped = warp_to_reference_image(&rgb_image, &transform, InterpolationMethod::Lanczos3);

    // Verify dimensions preserved
    assert_eq!(warped.width(), width);
    assert_eq!(warped.height(), height);
    assert_eq!(warped.channels(), 3);
    // Verify each channel was warped (non-zero values should exist)
    for c in 0..3 {
        let warped_channel = warped.channel(c);
        let non_zero_count = warped_channel.iter().filter(|&&v| v > 0.0).count();
        assert!(
            non_zero_count > 0,
            "Channel {} should have non-zero values after warping",
            c
        );
    }
}

#[test]
fn test_warp_to_reference_image_preserves_metadata() {
    use crate::astro_image::AstroImageMetadata;

    let (pixels, width, height) = generate_test_field(11111);
    let mut image = AstroImage::from_pixels(ImageDimensions::new(width, height, 1), pixels);

    // Set some metadata
    image.metadata = AstroImageMetadata {
        object: Some("M42".to_string()),
        exposure_time: Some(120.0),
        ..Default::default()
    };

    let transform = TransformMatrix::translation(5.0, 5.0);
    let warped = warp_to_reference_image(&image, &transform, InterpolationMethod::Bilinear);

    // Verify metadata is preserved
    assert_eq!(warped.metadata.object, Some("M42".to_string()));
    assert_eq!(warped.metadata.exposure_time, Some(120.0));
}

// ============================================================================
// Helper functions
// ============================================================================

/// Extract central region of two images for comparison (avoids border artifacts).
fn extract_central_region(
    a: &[f32],
    b: &[f32],
    width: usize,
    height: usize,
    margin: usize,
) -> (Vec<f32>, Vec<f32>) {
    let inner_width = width - 2 * margin;
    let inner_height = height - 2 * margin;

    let mut central_a = Vec::with_capacity(inner_width * inner_height);
    let mut central_b = Vec::with_capacity(inner_width * inner_height);

    for y in margin..(height - margin) {
        for x in margin..(width - margin) {
            let idx = y * width + x;
            central_a.push(a[idx]);
            central_b.push(b[idx]);
        }
    }

    (central_a, central_b)
}
