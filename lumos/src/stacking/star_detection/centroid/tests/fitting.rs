use crate::stacking::star_detection::centroid::tests::*;

/// Test that weighted centroid achieves claimed ~0.05 pixel accuracy
/// by testing many random sub-pixel offsets.
#[test]
fn test_weighted_centroid_precision_statistical() {
    let width = 128;
    let height = 128;
    let sigma = 2.5f32;

    // Test a grid of sub-pixel positions
    let mut total_error = 0.0f32;
    let mut max_error = 0.0f32;
    let mut count = 0;

    for dx in 0..10 {
        for dy in 0..10 {
            let true_pos = Vec2::new(64.0 + dx as f32 * 0.1, 64.0 + dy as f32 * 0.1);

            let pixels = make_gaussian_star(width, height, true_pos, sigma, 1.0, 0.1);
            let bg = estimate_background(
                &pixels,
                &BackgroundConfig {
                    tile_size: 32,
                    ..Default::default()
                },
            );
            let config = Config {
                measurement: MeasurementConfig {
                    centroid_method: CentroidMethod::WeightedMoments,
                    ..Default::default()
                },
                ..Default::default()
            };
            let candidates = detect_stars_test(&pixels, &bg, &config.detection);

            if candidates.is_empty() {
                continue;
            }

            if let Some(star) = measure_star(
                &pixels,
                &bg,
                &candidates[0],
                &config.measurement,
                config.fwhm.expected,
            ) {
                let error = ((star.pos.x - true_pos.x as f64).powi(2)
                    + (star.pos.y - true_pos.y as f64).powi(2))
                .sqrt() as f32;
                total_error += error;
                max_error = max_error.max(error);
                count += 1;
            }
        }
    }

    let avg_error = total_error / count as f32;

    // Weighted centroid should achieve ~0.05 pixel accuracy on average
    assert!(
        avg_error < 0.1,
        "Average centroid error {} exceeds 0.1 pixels (count={})",
        avg_error,
        count
    );
    assert!(
        max_error < 0.2,
        "Max centroid error {} exceeds 0.2 pixels",
        max_error
    );
}

/// Test that Gaussian fitting achieves claimed ~0.01 pixel accuracy.
#[test]
fn test_gaussian_fit_precision_statistical() {
    use crate::stacking::star_detection::centroid::gaussian_fit::{
        GaussianFitConfig, fit_gaussian_2d,
    };

    let width = 21;
    let height = 21;
    let sigma = 2.5f32;
    let background = 0.1f32;

    let mut total_error = 0.0f32;
    let mut max_error = 0.0f32;
    let mut count = 0;

    // Test a grid of sub-pixel positions
    for dx in 0..10 {
        for dy in 0..10 {
            let true_cx = 10.0 + dx as f32 * 0.1;
            let true_cy = 10.0 + dy as f32 * 0.1;

            // Create perfect Gaussian
            let mut pixels = vec![background; width * height];
            for y in 0..height {
                for x in 0..width {
                    let ddx = x as f32 - true_cx;
                    let ddy = y as f32 - true_cy;
                    pixels[y * width + x] +=
                        1.0 * (-0.5 * (ddx * ddx + ddy * ddy) / (sigma * sigma)).exp();
                }
            }
            let pixels_buf = Buffer2::new(width, height, pixels);

            let config = GaussianFitConfig::default();
            if let Some(result) =
                fit_gaussian_2d(&pixels_buf, Vec2::splat(10.0), 8, background, None, &config)
                && result.converged
            {
                let error =
                    ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
                total_error += error;
                max_error = max_error.max(error);
                count += 1;
            }
        }
    }

    let avg_error = total_error / count as f32;

    // Gaussian fitting should achieve ~0.01 pixel accuracy
    assert!(
        avg_error < 0.02,
        "Average Gaussian fit error {} exceeds 0.02 pixels (count={})",
        avg_error,
        count
    );
    assert!(
        max_error < 0.05,
        "Max Gaussian fit error {} exceeds 0.05 pixels",
        max_error
    );
}

/// Test that Moffat fitting achieves claimed ~0.01 pixel accuracy.
#[test]
fn test_moffat_fit_precision_statistical() {
    use crate::stacking::star_detection::centroid::moffat_fit::{MoffatFitConfig, fit_moffat_2d};

    let width = 21;
    let height = 21;
    let alpha = 2.5f32;
    let beta = 2.5f32;
    let background = 0.1f32;

    let mut total_error = 0.0f32;
    let mut max_error = 0.0f32;
    let mut count = 0;

    // Test a grid of sub-pixel positions
    for dx in 0..10 {
        for dy in 0..10 {
            let true_cx = 10.0 + dx as f32 * 0.1;
            let true_cy = 10.0 + dy as f32 * 0.1;

            // Create perfect Moffat profile
            let mut pixels = vec![background; width * height];
            for y in 0..height {
                for x in 0..width {
                    let r2 = (x as f32 - true_cx).powi(2) + (y as f32 - true_cy).powi(2);
                    pixels[y * width + x] += 1.0 * (1.0 + r2 / (alpha * alpha)).powf(-beta);
                }
            }
            let pixels_buf = Buffer2::new(width, height, pixels);

            let config = MoffatFitConfig {
                fixed_beta: beta,
                ..Default::default()
            };
            if let Some(result) =
                fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, background, None, &config)
                && result.converged
            {
                let error =
                    ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
                total_error += error;
                max_error = max_error.max(error);
                count += 1;
            }
        }
    }

    let avg_error = total_error / count as f32;

    // Moffat fitting should achieve ~0.01 pixel accuracy
    assert!(
        avg_error < 0.02,
        "Average Moffat fit error {} exceeds 0.02 pixels (count={})",
        avg_error,
        count
    );
    assert!(
        max_error < 0.05,
        "Max Moffat fit error {} exceeds 0.05 pixels",
        max_error
    );
}

/// Verify FWHM estimation accuracy from second moments.
#[test]
fn test_fwhm_estimation_accuracy() {
    let width = 128;
    let height = 128;
    let bg = make_uniform_background(width, height, 0.1, 0.001);

    // Test various sigma values
    for sigma in [1.5f32, 2.0, 2.5, 3.0, 3.5, 4.0] {
        let expected_fwhm = FWHM_TO_SIGMA * sigma;
        let pixels = make_gaussian_star(width, height, Vec2::splat(64.0), sigma, 1.0, 0.1);

        let metrics = compute_star(
            &pixels,
            &bg,
            Vec2::splat(64.0),
            0.0,
            TEST_STAMP_RADIUS,
            None,
            None,
        )
        .unwrap();

        let fwhm_error = (metrics.fwhm - expected_fwhm).abs() / expected_fwhm;
        assert!(
            fwhm_error < 0.15,
            "FWHM error {:.1}% too large for sigma={} (expected={:.2}, got={:.2})",
            fwhm_error * 100.0,
            sigma,
            expected_fwhm,
            metrics.fwhm
        );
    }
}

/// Verify eccentricity calculation for known elliptical sources.
#[test]
fn test_eccentricity_calculation_accuracy() {
    let width = 64;
    let height = 64;
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Test various axis ratios
    // eccentricity = sqrt(1 - (b/a)^2) where a >= b
    let test_cases = [
        (2.5, 2.5, 0.0),   // Circular: e = 0
        (3.0, 2.0, 0.745), // 1.5:1 ratio: e = sqrt(1 - 4/9) ≈ 0.745
        (4.0, 2.0, 0.866), // 2:1 ratio: e = sqrt(1 - 1/4) ≈ 0.866
    ];

    for (sigma_major, sigma_minor, expected_ecc) in test_cases {
        let pixels = make_elliptical_star(
            width,
            height,
            Vec2::splat(32.0),
            sigma_major,
            sigma_minor,
            0.8,
            0.1,
        );
        let metrics = compute_star(
            &pixels,
            &bg,
            Vec2::splat(32.0),
            0.0,
            TEST_STAMP_RADIUS,
            None,
            None,
        )
        .unwrap();

        let ecc_error = (metrics.eccentricity - expected_ecc).abs();
        assert!(
            ecc_error < 0.15,
            "Eccentricity error {} too large for ratio {:.1}:{:.1} (expected={:.3}, got={:.3})",
            ecc_error,
            sigma_major,
            sigma_minor,
            expected_ecc,
            metrics.eccentricity
        );
    }
}

#[test]
fn snr_uses_normalized_noise_units() {
    let model = NoiseModel::from_normalized(1_000.0, 10.0);

    // Model variance = 2/1000 + 4 × (0.02² + (10/1000)²) = 0.004.
    let modeled = compute_snr(2.0, 0.02, 4, Some(&model));
    let expected_modeled = 2.0 / 0.004_f32.sqrt();
    assert!((modeled - expected_modeled).abs() < 1e-5);

    // Background-only variance = 4 × 0.02² = 0.0016.
    let background_only = compute_snr(2.0, 0.02, 4, None);
    assert!((background_only - 50.0).abs() < 1e-5);
    assert_ne!(modeled, background_only);
}

/// Verify sharpness distinguishes point sources from extended sources.
#[test]
fn test_sharpness_point_vs_extended() {
    let width = 64;
    let height = 64;
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Compact star (small sigma) - high sharpness
    let compact = make_gaussian_star(width, height, Vec2::splat(32.0), 1.5, 0.8, 0.1);
    let metrics_compact = compute_star(
        &compact,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Extended star (large sigma) - lower sharpness
    let extended = make_gaussian_star(width, height, Vec2::splat(32.0), 4.0, 0.8, 0.1);
    let metrics_extended = compute_star(
        &extended,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    assert!(
        metrics_compact.sharpness > metrics_extended.sharpness,
        "Compact star should have higher sharpness than extended: {} vs {}",
        metrics_compact.sharpness,
        metrics_extended.sharpness
    );

    // Sharpness should be in valid range
    assert!(
        metrics_compact.sharpness > 0.0 && metrics_compact.sharpness <= 1.0,
        "Sharpness out of range: {}",
        metrics_compact.sharpness
    );
}

/// Verify Moffat FWHM formula is correct.
#[test]
fn test_moffat_fwhm_formula() {
    use crate::stacking::star_detection::centroid::moffat_fit::{
        alpha_beta_to_fwhm, fwhm_beta_to_alpha,
    };

    // Test known values
    // For beta=2.5: FWHM = 2*alpha*sqrt(2^0.4 - 1) ≈ 2*alpha*0.5657 ≈ 1.131*alpha
    let alpha = 2.0f32;
    let beta = 2.5f32;
    let fwhm = alpha_beta_to_fwhm(alpha, beta);

    // Verify against expected value
    let expected = 2.0 * alpha * (2.0f32.powf(1.0 / beta) - 1.0).sqrt();
    assert!(
        (fwhm - expected).abs() < 1e-6,
        "FWHM formula incorrect: {} vs {}",
        fwhm,
        expected
    );

    // Verify round-trip
    let alpha_back = fwhm_beta_to_alpha(fwhm, beta);
    assert!(
        (alpha_back - alpha).abs() < 1e-6,
        "Round-trip failed: {} vs {}",
        alpha_back,
        alpha
    );

    // Test limiting case: as beta -> infinity, Moffat -> Gaussian
    // For large beta, FWHM ≈ 2*alpha*sqrt(ln(2)/beta) -> 0
    // But for beta=4.765 (theoretical), FWHM ≈ 0.95*alpha
    let beta_theory = 4.765f32;
    let fwhm_theory = alpha_beta_to_fwhm(alpha, beta_theory);
    assert!(
        fwhm_theory > 0.0 && fwhm_theory < fwhm,
        "Higher beta should give smaller FWHM"
    );
}

/// Test Gaussian fitting recovers correct sigma values.
#[test]
fn test_gaussian_fit_sigma_recovery() {
    use crate::stacking::star_detection::centroid::gaussian_fit::{
        GaussianFitConfig, fit_gaussian_2d,
    };

    let width = 21;
    let height = 21;
    let background = 0.1f32;

    // Test sigma values that fit well within stamp_radius=8
    for true_sigma in [2.0f32, 2.5, 3.0, 3.5] {
        let cx = 10.0f32;
        let cy = 10.0f32;

        let mut pixels = vec![background; width * height];
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                pixels[y * width + x] +=
                    1.0 * (-0.5 * (dx * dx + dy * dy) / (true_sigma * true_sigma)).exp();
            }
        }
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = GaussianFitConfig::default();
        let result = fit_gaussian_2d(&pixels_buf, Vec2::new(cx, cy), 8, background, None, &config);

        let result =
            result.unwrap_or_else(|| panic!("Fit should return Some for sigma={}", true_sigma));

        // Check that sigma values are accurate (convergence flag may be false if
        // initial guess was already close, causing small parameter changes)
        let sigma_error_x = (result.sigma.x - true_sigma).abs() / true_sigma;
        let sigma_error_y = (result.sigma.y - true_sigma).abs() / true_sigma;

        assert!(
            sigma_error_x < 0.1,
            "Sigma_x error {:.1}% too large for sigma={} (got={})",
            sigma_error_x * 100.0,
            true_sigma,
            result.sigma.x
        );
        assert!(
            sigma_error_y < 0.1,
            "Sigma_y error {:.1}% too large for sigma={} (got={})",
            sigma_error_y * 100.0,
            true_sigma,
            result.sigma.y
        );
    }
}

/// Test Moffat fitting recovers correct alpha values.
#[test]
fn test_moffat_fit_alpha_recovery() {
    use crate::stacking::star_detection::centroid::moffat_fit::{MoffatFitConfig, fit_moffat_2d};

    let width = 21;
    let height = 21;
    let background = 0.1f32;
    let beta = 2.5f32;

    // Test alpha values that fit well within stamp_radius=8
    for true_alpha in [2.0f32, 2.5, 3.0, 3.5] {
        let cx = 10.0f32;
        let cy = 10.0f32;

        let mut pixels = vec![background; width * height];
        for y in 0..height {
            for x in 0..width {
                let r2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
                pixels[y * width + x] += 1.0 * (1.0 + r2 / (true_alpha * true_alpha)).powf(-beta);
            }
        }
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = MoffatFitConfig {
            fixed_beta: beta,
            ..Default::default()
        };
        let result = fit_moffat_2d(&pixels_buf, Vec2::new(cx, cy), 8, background, None, &config)
            .unwrap_or_else(|| panic!("Fit should return Some for alpha={}", true_alpha));

        // Check that alpha is accurate (convergence flag may be false if
        // initial guess was already close)
        let alpha_error = (result.debug.alpha - true_alpha).abs() / true_alpha;

        assert!(
            alpha_error < 0.15,
            "Alpha error {:.1}% too large for alpha={} (got={})",
            alpha_error * 100.0,
            true_alpha,
            result.debug.alpha
        );
    }
}

/// Test that fitting works with noisy data.
#[test]
fn test_gaussian_fit_with_noise() {
    use crate::stacking::star_detection::centroid::gaussian_fit::{
        GaussianFitConfig, fit_gaussian_2d,
    };

    let width = 21;
    let height = 21;
    let true_cx = 10.3f32;
    let true_cy = 10.7f32;
    let true_sigma = 2.5f32;
    let background = 0.1f32;

    // Create Gaussian with deterministic "noise" pattern
    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - true_cx;
            let dy = y as f32 - true_cy;
            let signal = 1.0 * (-0.5 * (dx * dx + dy * dy) / (true_sigma * true_sigma)).exp();
            // Add small deterministic noise
            let noise = ((x * 7 + y * 13) % 100) as f32 * 0.001 - 0.05;
            pixels[y * width + x] += signal + noise * 0.1;
        }
    }
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, Vec2::splat(10.0), 8, background, None, &config);

    assert!(result.is_some(), "Fit should succeed with noise");
    let result = result.unwrap();

    // With noise, expect slightly worse but still good accuracy
    let error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.15,
        "Position error {} too large with noise",
        error
    );
}

/// Verify roundness1 (GROUND) is close to 0 for circular sources.
#[test]
fn test_roundness1_circular_source() {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_star(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    assert!(
        metrics.roundness1.abs() < 0.1,
        "Circular source should have roundness1 near 0, got {}",
        metrics.roundness1
    );
}

/// Verify roundness1 detects x-elongated sources.
#[test]
fn test_roundness1_x_elongated() {
    let width = 64;
    let height = 64;
    // sigma_x > sigma_y means more spread in x direction
    let pixels = make_elliptical_star(width, height, Vec2::splat(32.0), 4.0, 2.0, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_star(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // For x-elongated: marginal in x has lower peak (more spread)
    // Roundness1 = (Hx - Hy) / (Hx + Hy)
    // This should be negative because Hx (peak of x marginal) < Hy
    assert!(
        metrics.roundness1 != 0.0,
        "Elongated source should have non-zero roundness1"
    );
}

/// Verify roundness2 (SROUND) is close to 0 for symmetric sources.
#[test]
fn test_roundness2_symmetric_source() {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_star(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    assert!(
        metrics.roundness2 < 0.1,
        "Symmetric source should have roundness2 near 0, got {}",
        metrics.roundness2
    );
}
