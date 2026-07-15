//! Warping tests using synthetic star field images.
//!
//! These tests verify that warping an image with a known transform
//! and then aligning it back produces images that match.
//!
//! Tests cover:
//! - All TransformType variants (Translation, Euclidean, Similarity, Affine, Homography)
//! - All InterpolationMethod variants (Nearest, Bilinear, Bicubic, Lanczos2/3/4)

use crate::stacking::registration::config::InterpolationMethod;
use crate::stacking::registration::interpolation::{WarpParams, warp_image};
use crate::stacking::registration::resample::warp;
use crate::stacking::registration::synthetic_tests::helpers;
use crate::stacking::registration::transform::{Transform, TransformType, WarpTransform};
use crate::stacking::star_detection::detector::StarDetector;
use crate::testing::synthetic::fixtures::star_field;
use crate::{AstroImage, ImageDimensions};
use glam::DVec2;
use imaginarium::Buffer2;

/// Helper to warp and return a new buffer (for test convenience).
/// Visually applies the transform to the image content (stars move by T).
/// Passes T⁻¹ to warp_image since it uses output→input coordinate mapping.
fn do_warp(
    input: &Buffer2<f32>,
    transform: &Transform,
    method: InterpolationMethod,
) -> Buffer2<f32> {
    let inverse = transform.inverse();
    let mut output = Buffer2::new(input.width(), input.height(), vec![0.0; input.len()]);
    warp_image(
        input,
        &mut output,
        &WarpTransform::new(inverse),
        &WarpParams::new(method),
    );
    output
}

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

/// All interpolation methods to test.
fn all_interpolation_methods() -> Vec<InterpolationMethod> {
    vec![
        InterpolationMethod::Nearest,
        InterpolationMethod::Bilinear,
        InterpolationMethod::Bicubic,
        InterpolationMethod::Lanczos2 { deringing: 0.3 },
        InterpolationMethod::Lanczos3 { deringing: 0.3 },
        InterpolationMethod::Lanczos4 { deringing: 0.3 },
    ]
}

/// Per-method PSNR and NCC thresholds for roundtrip warp tests.
type MethodThresholds = &'static [(InterpolationMethod, f64, f64)];

/// Common helper for roundtrip warp tests.
///
/// Creates a star field, warps forward then inverse, and checks that the
/// central region matches the original within per-method thresholds.
fn assert_roundtrip(
    seed: u64,
    forward: Transform,
    label: &str,
    margin: usize,
    thresholds: MethodThresholds,
) {
    let ref_buf = star_field(256, 256, 30, seed).image.channel(0).clone();
    let width = ref_buf.width();
    let height = ref_buf.height();
    let inverse = forward.inverse();

    for &(method, min_psnr, min_ncc) in thresholds {
        let warped = do_warp(&ref_buf, &forward, method);
        let restored = do_warp(&warped, &inverse, method);

        let (central_ref, central_restored) =
            extract_central_region(ref_buf.pixels(), restored.pixels(), width, height, margin);

        let psnr = compute_psnr(&central_ref, &central_restored, 1.0);
        let ncc = compute_ncc(&central_ref, &central_restored);

        assert!(
            psnr > min_psnr,
            "{label} {:?}: PSNR {psnr} < {min_psnr} dB",
            method,
        );
        assert!(ncc > min_ncc, "{label} {:?}: NCC {ncc} < {min_ncc}", method,);
    }
}

/// Test that warping with identity transform preserves the image.
#[test]
fn test_warp_identity_all_methods() {
    let ref_buf = star_field(256, 256, 30, 12345).image.channel(0).clone();
    let identity = Transform::identity();

    for method in all_interpolation_methods() {
        let warped = do_warp(&ref_buf, &identity, method);

        let psnr = compute_psnr(ref_buf.pixels(), warped.pixels(), 1.0);
        let ncc = compute_ncc(ref_buf.pixels(), warped.pixels());

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

#[test]
fn test_warp_translation_roundtrip() {
    assert_roundtrip(
        11111,
        Transform::translation(DVec2::new(10.5, -7.3)),
        "Translation",
        20,
        &[
            (InterpolationMethod::Nearest, 15.0, 0.25),
            (InterpolationMethod::Bilinear, 22.0, 0.70),
            (InterpolationMethod::Lanczos3 { deringing: 0.3 }, 25.0, 0.80),
        ],
    );
}

#[test]
fn test_warp_euclidean_roundtrip() {
    assert_roundtrip(
        22222,
        Transform::euclidean(DVec2::new(5.0, -3.0), 2.0_f64.to_radians()),
        "Euclidean",
        30,
        &[
            (InterpolationMethod::Nearest, 12.0, 0.25),
            (InterpolationMethod::Bilinear, 25.0, 0.85),
            (InterpolationMethod::Lanczos3 { deringing: 0.3 }, 30.0, 0.90),
        ],
    );
}

#[test]
fn test_warp_similarity_roundtrip() {
    assert_roundtrip(
        33333,
        Transform::similarity(DVec2::new(8.0, -5.0), 1.5_f64.to_radians(), 1.02),
        "Similarity",
        40,
        &[
            (InterpolationMethod::Nearest, 12.0, 0.85),
            (InterpolationMethod::Bilinear, 22.0, 0.85),
            (InterpolationMethod::Lanczos3 { deringing: 0.3 }, 28.0, 0.85),
        ],
    );
}

#[test]
fn test_warp_affine_roundtrip() {
    // Affine with slight differential scaling
    let angle_rad = 0.5_f64.to_radians();
    let (cos_a, sin_a) = (angle_rad.cos(), angle_rad.sin());
    let forward = Transform::affine([
        1.01 * cos_a,
        -0.99 * sin_a,
        6.0,
        1.01 * sin_a,
        0.99 * cos_a,
        -4.0,
    ]);
    assert_eq!(forward.transform_type, TransformType::Affine);

    assert_roundtrip(
        44444,
        forward,
        "Affine",
        40,
        &[
            (InterpolationMethod::Nearest, 12.0, 0.85),
            (InterpolationMethod::Bilinear, 22.0, 0.85),
            (InterpolationMethod::Lanczos3 { deringing: 0.3 }, 26.0, 0.85),
        ],
    );
}

#[test]
fn test_warp_homography_roundtrip() {
    // Mild perspective distortion
    let forward = Transform::homography([1.0, 0.0, 5.0, 0.0, 1.0, -3.0, 0.00005, 0.00003]);
    assert_eq!(forward.transform_type, TransformType::Homography);

    assert_roundtrip(
        55555,
        forward,
        "Homography",
        50,
        &[
            (InterpolationMethod::Nearest, 10.0, 0.80),
            (InterpolationMethod::Bilinear, 20.0, 0.80),
            (InterpolationMethod::Lanczos3 { deringing: 0.3 }, 24.0, 0.80),
        ],
    );
}

#[test]
fn test_warp_with_detected_transform() {
    use crate::AstroImage;

    use crate::stacking::registration::{Config as RegConfig, register};
    use crate::stacking::star_detection::config::Config as StarConfig;

    let width = 256;
    let height = 256;
    let ref_pixels = star_field(width, height, 40, 66666)
        .image
        .channel(0)
        .clone();

    // Apply a known transform
    let dx = 12.0;
    let dy = -8.0;
    let angle_rad = 0.8_f64.to_radians();

    let true_transform = Transform::euclidean(DVec2::new(dx, dy), angle_rad);

    // Create target by warping reference
    let target_pixels = do_warp(
        &ref_pixels,
        &true_transform,
        InterpolationMethod::Lanczos3 { deringing: 0.3 },
    );

    // Detect stars in both images
    let mut detection_config = StarConfig::default();
    detection_config.fwhm.expected = 0.0;
    detection_config.filter.min_snr = 5.0;
    detection_config.detection.sigma_threshold = 3.0;
    let mut det = StarDetector::from_config(detection_config).unwrap();

    let ref_image = AstroImage::from_pixels(
        ImageDimensions::new((width, height), 1),
        ref_pixels.to_vec(),
    );
    let target_image = AstroImage::from_pixels(
        ImageDimensions::new((width, height), 1),
        target_pixels.to_vec(),
    );

    let ref_result = det.detect(&ref_image);
    let target_result = det.detect(&target_image);

    // Register to find transform using detected stars directly
    let reg_config = RegConfig {
        transform_type: TransformType::Euclidean,
        matching: helpers::matching_config(6, 4),
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    // Use warp to align target back to reference frame
    let target_astro = AstroImage::from_pixels(
        ImageDimensions::new((width, height), 1),
        target_pixels.into_vec(),
    );
    let warp_config = WarpParams {
        method: InterpolationMethod::Lanczos3 { deringing: 0.3 },
        ..Default::default()
    };
    let warped_astro = warp(&target_astro, &result.warp_transform(), &warp_config).image;

    // Compare aligned image to reference
    let margin = 40;
    let (central_ref, central_aligned) = extract_central_region(
        ref_pixels.pixels(),
        warped_astro.channel(0),
        width,
        height,
        margin,
    );

    let psnr = compute_psnr(&central_ref, &central_aligned, 1.0);
    let ncc = compute_ncc(&central_ref, &central_aligned);

    assert!(psnr > 25.0, "End-to-end alignment PSNR {} < 25 dB", psnr);
    assert!(ncc > 0.90, "End-to-end alignment NCC {} < 0.90", ncc);
}

#[test]
fn test_interpolation_quality_ordering() {
    let ref_buf = star_field(256, 256, 30, 77777).image.channel(0).clone();
    let width = ref_buf.width();
    let height = ref_buf.height();

    // Apply a transform that requires interpolation
    let forward = Transform::similarity(DVec2::new(3.7, -2.3), 1.0_f64.to_radians(), 1.01);
    let inverse = forward.inverse();

    let mut results: Vec<(InterpolationMethod, f64)> = Vec::new();

    for method in all_interpolation_methods() {
        let warped = do_warp(&ref_buf, &forward, method);
        let restored = do_warp(&warped, &inverse, method);

        let margin = 50;
        let (central_ref, central_restored) =
            extract_central_region(ref_buf.pixels(), restored.pixels(), width, height, margin);

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
        .find(|(m, _)| *m == InterpolationMethod::Lanczos3 { deringing: 0.3 })
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

#[test]
fn test_warp_grayscale_translation() {
    let ref_buf = star_field(256, 256, 30, 88888).image.channel(0).clone();
    let width = ref_buf.width();
    let height = ref_buf.height();
    let ref_pixels = ref_buf.into_vec();
    let ref_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), ref_pixels.clone());

    // Apply a translation of (5, -3) pixels
    let transform = Transform::translation(DVec2::new(5.0, -3.0));

    let warp_config = WarpParams {
        method: InterpolationMethod::Lanczos3 { deringing: 0.3 },
        ..Default::default()
    };
    let warped = warp(&ref_image, &WarpTransform::new(transform), &warp_config).image;

    // Verify dimensions preserved
    assert_eq!(warped.width(), width);
    assert_eq!(warped.height(), height);
    assert_eq!(warped.channels(), 1);

    // Verify pixels actually moved: the warped image should differ from input
    // in the central region (not just "it doesn't panic")
    let warped_pixels = warped.channel(0);
    let mut diff_count = 0usize;
    let margin = 20;
    for y in margin..height - margin {
        for x in margin..width - margin {
            let idx = y * width + x;
            if (ref_pixels[idx] - warped_pixels[idx]).abs() > 1e-4 {
                diff_count += 1;
            }
        }
    }
    let total_central = (height - 2 * margin) * (width - 2 * margin);
    // With a 5-pixel translation on star field, some pixels near stars should differ.
    // Star field is mostly background (zero), so only pixels near stars change.
    // 30 stars with ~5px radius gives ~2300 affected pixels minimum.
    assert!(
        diff_count > 1000,
        "Expected some differing pixels after translation, got {}/{}",
        diff_count,
        total_central
    );
}

#[test]
fn test_warp_rgb() {
    let gray_buf = star_field(256, 256, 30, 99999).image.channel(0).clone();
    let width = gray_buf.width();
    let height = gray_buf.height();

    // Create RGB image by duplicating grayscale to all channels with slight offsets
    let mut rgb_pixels = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            let val = gray_buf[(x, y)];
            // Slightly different values per channel to verify independent processing
            rgb_pixels.push(val); // R
            rgb_pixels.push((val + 0.1).min(1.0)); // G
            rgb_pixels.push(if (x + y) % 2 == 0 { val } else { val * 0.8 }); // B
        }
    }

    let rgb_image = AstroImage::from_pixels(ImageDimensions::new((width, height), 3), rgb_pixels);

    // Apply a transform
    let transform = Transform::euclidean(DVec2::new(3.0, -2.0), 1.0_f64.to_radians());

    // Warp the RGB image
    let warp_config = WarpParams {
        method: InterpolationMethod::Lanczos3 { deringing: 0.3 },
        ..Default::default()
    };
    let warped = warp(&rgb_image, &WarpTransform::new(transform), &warp_config).image;

    // Verify dimensions preserved
    assert_eq!(warped.width(), width);
    assert_eq!(warped.height(), height);
    assert_eq!(warped.channels(), 3);

    // Verify each channel was warped independently:
    // Channels have different input values, so warped channels should differ from each other
    let ch0 = warped.channel(0);
    let ch1 = warped.channel(1);

    // Count pixels where channels differ (G channel had +0.1 offset)
    let margin = 10;
    let mut differ_count = 0usize;
    for y in margin..height - margin {
        for x in margin..width - margin {
            let idx = y * width + x;
            if (ch0[idx] - ch1[idx]).abs() > 0.01 {
                differ_count += 1;
            }
        }
    }
    let total_inner = (height - 2 * margin) * (width - 2 * margin);
    assert!(
        differ_count > total_inner / 2,
        "Channels should differ after warping (independent processing), only {}/{} differ",
        differ_count,
        total_inner
    );
}

#[test]
fn test_warp_preserves_output_metadata() {
    use crate::io::astro_image::AstroImageMetadata;

    let pixels = star_field(256, 256, 30, 11111).image.channel(0).clone();
    let width = pixels.width();
    let height = pixels.height();
    let mut image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), pixels.into_vec());
    image.metadata = AstroImageMetadata {
        object: Some("M42".to_string()),
        exposure_time: Some(120.0),
        ..Default::default()
    };

    let transform = Transform::translation(DVec2::new(5.0, 5.0));
    let warp_config = WarpParams {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };
    let warped = warp(&image, &WarpTransform::new(transform), &warp_config).image;

    // Verify the input's metadata is carried into the warped output (warp only
    // produces new pixel data).
    assert_eq!(warped.metadata.object, Some("M42".to_string()));
    assert_eq!(warped.metadata.exposure_time, Some(120.0));
}

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

/// Test that warp with SIP correction produces different (corrected) output
/// compared to warp without SIP.
#[test]
fn test_warp_with_sip_correction() {
    use crate::stacking::registration::distortion::sip::{SipConfig, SipPolynomial};

    let width = 256;
    let height = 256;
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;

    // Generate a grid of matched point pairs with barrel distortion.
    // The "linear" transform is identity. The SIP polynomial should capture
    // the nonlinear (barrel) distortion.
    let transform = Transform::translation(DVec2::new(5.0, -3.0));

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();
    let distortion_k = 2e-6; // barrel distortion coefficient

    for gy in 0..16 {
        for gx in 0..16 {
            let rx = 16.0 + gx as f64 * 14.0;
            let ry = 16.0 + gy as f64 * 14.0;
            let ref_pos = DVec2::new(rx, ry);

            // Apply barrel distortion: r' = r + k*r^3
            let dx = rx - cx;
            let dy = ry - cy;
            let r2 = dx * dx + dy * dy;
            let distorted = DVec2::new(rx + distortion_k * dx * r2, ry + distortion_k * dy * r2);

            // Target = transform(distorted_ref)
            let target_pos = transform.apply(distorted);

            ref_points.push(ref_pos);
            target_points.push(target_pos);
        }
    }

    // Fit SIP from these matched points
    let sip_config = SipConfig {
        order: 3,
        reference_point: Some(DVec2::new(cx, cy)),
        ..Default::default()
    };
    let sip =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &sip_config)
            .unwrap()
            .polynomial;

    // Verify SIP correction is non-trivial
    let max_correction = sip.max_correction(width, height, 10.0);
    assert!(
        max_correction > 0.1,
        "SIP correction should be significant, got {}",
        max_correction
    );

    // Create a test image with a gradient pattern
    let ref_buf = star_field(width, height, 30, 54321)
        .image
        .channel(0)
        .clone();

    // Warp the same image with and without SIP
    let mut output_no_sip = Buffer2::new(width, height, vec![0.0; width * height]);
    let mut output_with_sip = Buffer2::new(width, height, vec![0.0; width * height]);

    warp_image(
        &ref_buf,
        &mut output_no_sip,
        &WarpTransform::new(transform),
        &WarpParams::new(InterpolationMethod::Lanczos3 { deringing: 0.3 }),
    );
    warp_image(
        &ref_buf,
        &mut output_with_sip,
        &WarpTransform::with_sip(transform, sip),
        &WarpParams::new(InterpolationMethod::Lanczos3 { deringing: 0.3 }),
    );

    // The two outputs should differ — SIP applies nonlinear correction
    let mut max_diff: f32 = 0.0;
    let mut diff_count = 0usize;
    let margin = 20;

    for y in margin..height - margin {
        for x in margin..width - margin {
            let d = (output_no_sip[(x, y)] - output_with_sip[(x, y)]).abs();
            if d > 1e-6 {
                diff_count += 1;
            }
            max_diff = max_diff.max(d);
        }
    }

    assert!(
        diff_count > 100,
        "SIP should produce different pixel values, only {} pixels differ",
        diff_count
    );
    assert!(
        max_diff > 0.001,
        "Max pixel difference too small: {}",
        max_diff
    );
}

/// Test that warp with SIP correction through the public `warp()` API works.
#[test]
fn test_warp_api_with_sip() {
    use crate::stacking::registration::distortion::sip::{SipConfig, SipPolynomial};

    let width = 128;
    let height = 128;
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;

    let transform = Transform::identity();
    let distortion_k = 3e-6;

    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();

    for gy in 0..10 {
        for gx in 0..10 {
            let rx = 10.0 + gx as f64 * 11.0;
            let ry = 10.0 + gy as f64 * 11.0;
            let ref_pos = DVec2::new(rx, ry);

            let dx = rx - cx;
            let dy = ry - cy;
            let r2 = dx * dx + dy * dy;
            let distorted = DVec2::new(rx + distortion_k * dx * r2, ry + distortion_k * dy * r2);
            let target_pos = transform.apply(distorted);

            ref_points.push(ref_pos);
            target_points.push(target_pos);
        }
    }

    let sip_config = SipConfig {
        order: 3,
        reference_point: Some(DVec2::new(cx, cy)),
        ..Default::default()
    };
    let sip =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &sip_config)
            .unwrap()
            .polynomial;

    // Create a grayscale image
    let pixels = star_field(width, height, 20, 12321)
        .image
        .channel(0)
        .clone();
    let image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), pixels.into_vec());

    let warp_config = WarpParams {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };

    // Warp without SIP
    let warped_no_sip = warp(&image, &WarpTransform::new(transform), &warp_config).image;

    // Warp with SIP
    let warped_with_sip = warp(
        &image,
        &WarpTransform::with_sip(transform, sip),
        &warp_config,
    )
    .image;

    // They should differ
    let ch_no_sip = warped_no_sip.channel(0);
    let ch_with_sip = warped_with_sip.channel(0);
    let diff_count = ch_no_sip
        .iter()
        .zip(ch_with_sip.iter())
        .filter(|(a, b)| (*a - *b).abs() > 1e-6)
        .count();

    assert!(
        diff_count > 50,
        "SIP should produce different warp output, only {} pixels differ",
        diff_count
    );
}

/// `warp` emits a per-pixel coverage map (the in-bounds kernel-weight fraction)
/// and renormalizes partially-covered bilinear border pixels back to the
/// in-bounds average instead of darkening them toward the zero border.
#[test]
fn warp_emits_coverage_and_renormalizes_bilinear_border() {
    // Constant image so any darkening is unambiguous: a covered output pixel
    // must read back exactly V.
    const V: f32 = 0.5;
    let (w, h) = (16usize, 8usize);
    let image = AstroImage::from_pixels(ImageDimensions::new((w, h), 1), vec![V; w * h]);

    // output(x,y) samples source (x + 2.5, y): columns 0..=12 are fully in
    // bounds, column 13 is half-covered (its right bilinear tap is off the
    // edge), columns 14..=15 fall entirely outside.
    let transform = Transform::translation(DVec2::new(2.5, 0.0));
    let config = WarpParams {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };
    assert_eq!(config.border_value, 0.0, "test assumes a zero border");

    let result = warp(&image, &WarpTransform::new(transform), &config);
    let cov = result.coverage.pixels();
    let val = result.image.channel(0).pixels();
    let at = |x: usize, y: usize| y * w + x;

    // x-only translation → every row shares the column pattern; check one.
    let y = 4;
    for x in 0..=12 {
        assert!(
            (cov[at(x, y)] - 1.0).abs() < 1e-5,
            "col {x} should be fully covered, got {}",
            cov[at(x, y)]
        );
    }
    assert!(
        (cov[at(13, y)] - 0.5).abs() < 1e-5,
        "col 13 should be half-covered, got {}",
        cov[at(13, y)]
    );
    assert_eq!(cov[at(14, y)], 0.0, "col 14 is outside the source");
    assert_eq!(cov[at(15, y)], 0.0, "col 15 is outside the source");

    // Renormalization: every covered pixel — including the half-covered
    // column 13 — reads back V, not a darkened 0.5·V; fully-outside columns
    // stay at the zero border.
    for x in 0..=13 {
        assert!(
            (val[at(x, y)] - V).abs() < 1e-5,
            "col {x} should renormalize to V={V}, got {}",
            val[at(x, y)]
        );
    }
    assert_eq!(val[at(14, y)], 0.0);
    assert_eq!(val[at(15, y)], 0.0);

    // Coverage stays in [0, 1] everywhere; every pixel is either border-zero or V.
    for (&c, &v) in cov.iter().zip(val.iter()) {
        assert!((0.0..=1.0).contains(&c), "coverage {c} out of range");
        assert!(v == 0.0 || (v - V).abs() < 1e-5, "unexpected value {v}");
    }
}

/// The default negative-lobe kernel (Lanczos3) emits coverage — 1.0 interior, 0.0 fully
/// outside, fractional across an `a`-wide border band — AND renormalizes the value by the
/// in-bounds weight: a partially-covered edge pixel of a flat field reads V (recovered),
/// not V·coverage (darkened). Coverage stays as the downstream stacking weight.
#[test]
fn warp_renormalizes_lanczos_edges_and_emits_coverage() {
    const V: f32 = 0.5;
    let (w, h) = (32usize, 8usize);
    let image = AstroImage::from_pixels(ImageDimensions::new((w, h), 1), vec![V; w * h]);

    // src = (x + 3.5, y): a Lanczos3 (6-tap) kernel reaches off the right edge
    // for x ≳ 26 and lands entirely outside by x = 31.
    let transform = Transform::translation(DVec2::new(3.5, 0.0));
    let config = WarpParams {
        method: InterpolationMethod::Lanczos3 { deringing: 0.3 },
        ..Default::default()
    };

    let result = warp(&image, &WarpTransform::new(transform), &config);
    let cov = result.coverage.pixels();
    let val = result.image.channel(0).pixels();
    let at = |x: usize, y: usize| y * w + x;
    let y = 4;

    // Interior: every kernel tap is in bounds, so Σ_in/Σ_all == 1 exactly.
    assert!(
        (cov[at(10, y)] - 1.0).abs() < 1e-5,
        "interior coverage should be 1.0, got {}",
        cov[at(10, y)]
    );
    // Far past the edge: every tap is outside.
    assert_eq!(cov[at(31, y)], 0.0, "column 31 is fully extrapolated");
    // A fractional border band exists between the two.
    let partial = (24..32)
        .filter(|&x| cov[at(x, y)] > 0.0 && cov[at(x, y)] < 1.0)
        .count();
    assert!(
        partial >= 2,
        "expected a fractional coverage band, got {partial} columns"
    );

    // Lanczos is renormalized by the in-bounds weight: a flat field reads V everywhere it
    // has any coverage — interior AND partially-covered edge columns — instead of being
    // darkened to V·coverage as it was before in-sampler weight tracking.
    assert!(
        (val[at(10, y)] - V).abs() < 1e-4,
        "interior value should be V, got {}",
        val[at(10, y)]
    );
    let edge_x = (24..31)
        .find(|&x| cov[at(x, y)] > 0.05 && cov[at(x, y)] < 0.95)
        .expect("a partially-covered edge column");
    assert!(
        (val[at(edge_x, y)] - V).abs() < 1e-4,
        "renormalized Lanczos edge value should recover V, got {} at col {edge_x} (cov {})",
        val[at(edge_x, y)],
        cov[at(edge_x, y)]
    );

    for &c in cov {
        assert!((0.0..=1.0).contains(&c), "coverage {c} out of range");
    }
}
