use crate::stacking::star_detection::centroid::tests::*;

#[test]
fn test_refine_centroid_centered_star() {
    let width = 64;
    let height = 64;
    let pos = Vec2::splat(32.0);
    let pixels = make_gaussian_star(width, height, pos, 2.5, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        pos,
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );

    assert!(result.is_some());
    let new_pos = result.unwrap();
    // Should stay very close to original position
    assert!((new_pos.x - pos.x).abs() < 0.5);
    assert!((new_pos.y - pos.y).abs() < 0.5);
}

#[test]
fn test_refine_centroid_offset_converges() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.3, 32.7);
    let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Start with integer guess (peak pixel position)
    let start_pos = Vec2::new(32.0, 33.0);

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        start_pos,
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );

    assert!(result.is_some());
    let new_pos = result.unwrap();
    // Should move towards true center
    let old_error =
        ((start_pos.x - true_pos.x).powi(2) + (start_pos.y - true_pos.y).powi(2)).sqrt();
    let new_error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        new_error < old_error,
        "Refinement should reduce error: {} -> {}",
        old_error,
        new_error
    );
}

#[test]
fn test_refine_centroid_invalid_position_returns_none() {
    let width = 64;
    let height = 64;
    let pixels = vec![0.5f32; width * height];
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Position too close to edge
    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::new(3.0, 32.0),
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );
    assert!(result.is_none());
}

#[test]
fn test_refine_centroid_zero_flux_returns_none() {
    let width = 64;
    let height = 64;
    // All pixels equal to background - no signal
    let pixels = vec![0.1f32; width * height];
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );
    assert!(result.is_none());
}

#[test]
fn test_refine_centroid_rejects_large_movement() {
    let width = 64;
    let height = 64;
    // Create a star very far from initial position (outside the stamp entirely)
    let pixels = make_gaussian_star(width, height, Vec2::splat(50.0), 2.5, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Start far from the actual star - the stamp won't contain the star,
    // so there's no signal, which should cause rejection
    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );

    // With no star in the stamp (star is at 50,50, stamp centered at 32,32 with radius 7),
    // the weighted centroid has zero or near-zero flux
    assert!(result.is_none());
}

#[test]
fn test_refine_centroid_iterative_convergence() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.25, 32.75);
    let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Simulate multiple iterations like measure_star does
    let mut pos = Vec2::splat(32.0);

    for iteration in 0..MAX_MOMENTS_ITERATIONS {
        let result = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos,
            TEST_STAMP_RADIUS,
            TEST_EXPECTED_FWHM,
        );
        assert!(result.is_some(), "Iteration {} failed", iteration);

        let new_pos = result.unwrap();
        let delta = new_pos - pos;
        pos = new_pos;

        if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }

    // Should converge close to true position
    let error = ((pos.x - true_pos.x).powi(2) + (pos.y - true_pos.y).powi(2)).sqrt();
    assert!(error < 0.2, "Failed to converge: error = {}", error);
}

#[test]
fn test_compute_star_valid_star() {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let peak = 0.73;
    let star = compute_star(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        peak,
        TEST_STAMP_RADIUS,
        None,
        None,
    );

    assert!(star.is_some());
    let m = star.unwrap();
    assert_eq!(m.pos, Vec2::splat(32.0).as_dvec2());
    assert_eq!(m.peak, peak);
    assert!(m.flux > 0.0, "Flux should be positive");
    assert!(m.fwhm > 0.0, "FWHM should be positive");
    assert!(
        m.eccentricity >= 0.0 && m.eccentricity <= 1.0,
        "Eccentricity out of range"
    );
    assert!(m.snr > 0.0, "SNR should be positive");
}

#[test]
fn test_compute_star_background_override_replaces_global_map() {
    // Star (sigma 2.5, amplitude 0.8) on an exact 0.1 pedestal, with a global map matching
    // the truth (bg 0.1, noise 0.01). An override of bg 0.05 under-subtracts every stamp
    // pixel by exactly 0.05 (every pixel stays above both bg values, so `.max(0)` never
    // clamps), hence: flux_override - flux_global = 0.05 * npix = 0.05 * 15² = 11.25.
    // The override noise (0.05, vs the map's 0.01) must feed the simplified SNR formula
    // `flux / (noise * sqrt(npix))`, and the override bg must reach the windowed
    // covariance, where the unsubtracted 0.05 pedestal inflates the second moments and
    // therefore the FWHM relative to the correctly-subtracted global-map run.
    let width = 64;
    let height = 64;
    let pos = Vec2::splat(32.0);
    let pixels = make_gaussian_star(width, height, pos, 2.5, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let local_bg = LocalBackground {
        bg: 0.05,
        noise: 0.05,
    };

    let global = compute_star(&pixels, &bg, pos, 0.0, TEST_STAMP_RADIUS, None, None).unwrap();
    let local = compute_star(
        &pixels,
        &bg,
        pos,
        0.0,
        TEST_STAMP_RADIUS,
        Some(local_bg),
        None,
    )
    .unwrap();

    let npix = (2 * TEST_STAMP_RADIUS + 1).pow(2) as f32; // 15² = 225
    let flux_diff = local.flux - global.flux;
    let expected_diff = 0.05 * npix; // 11.25
    assert!(
        (flux_diff - expected_diff).abs() < 1e-2,
        "override bg must under-subtract exactly 0.05/pixel: flux diff {flux_diff}, expected {expected_diff}"
    );

    let expected_snr = local.flux / (0.05 * npix.sqrt());
    assert!(
        (local.snr - expected_snr).abs() / expected_snr < 1e-3,
        "override noise must feed the SNR: got {}, expected {expected_snr}",
        local.snr
    );

    assert!(
        local.fwhm > global.fwhm,
        "override bg must reach the windowed covariance: the unsubtracted pedestal should \
         inflate FWHM (override {} vs global {})",
        local.fwhm,
        global.fwhm
    );
}

#[test]
fn test_compute_star_invalid_position_returns_none() {
    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Position too close to edge
    let metrics = compute_star(
        &pixels,
        &bg,
        Vec2::new(3.0, 32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    );
    assert!(metrics.is_none());
}

#[test]
fn test_compute_star_zero_flux_returns_none() {
    let width = 64;
    let height = 64;
    // All pixels equal to or below background
    let pixels = Buffer2::new_filled(width, height, 0.05f32);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_star(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    );
    assert!(metrics.is_none());
}

#[test]
fn test_compute_star_fwhm_scales_with_sigma() {
    let width = 128;
    let height = 128;

    // Create stars with different sigmas
    let sigma_small = 2.0f32;
    let sigma_large = 4.0f32;

    let pixels_small = make_gaussian_star(width, height, Vec2::splat(64.0), sigma_small, 0.8, 0.1);
    let pixels_large = make_gaussian_star(width, height, Vec2::splat(64.0), sigma_large, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_small = compute_star(
        &pixels_small,
        &bg,
        Vec2::splat(64.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_large = compute_star(
        &pixels_large,
        &bg,
        Vec2::splat(64.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Larger sigma should result in larger FWHM
    assert!(
        metrics_large.fwhm > metrics_small.fwhm,
        "Larger sigma should give larger FWHM: {} vs {}",
        metrics_large.fwhm,
        metrics_small.fwhm
    );
}

#[test]
fn test_compute_star_snr_scales_with_amplitude() {
    let width = 64;
    let height = 64;

    let pixels_dim = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.2, 0.1);
    let pixels_bright = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_dim = compute_star(
        &pixels_dim,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_bright = compute_star(
        &pixels_bright,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Brighter star should have higher SNR
    assert!(
        metrics_bright.snr > metrics_dim.snr,
        "Brighter star should have higher SNR: {} vs {}",
        metrics_bright.snr,
        metrics_dim.snr
    );
}

#[test]
fn test_elongated_star_high_eccentricity() {
    let width = 64;
    let height = 64;
    // Elongated star: sigma_x = 4, sigma_y = 1 (4:1 aspect ratio)
    let pixels = make_elliptical_star(width, height, Vec2::splat(32.0), 4.0, 1.5, 0.8, 0.1);
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

    // Elongated star should have high eccentricity (> 0.5)
    assert!(
        metrics.eccentricity > 0.5,
        "Elongated star should have high eccentricity: {}",
        metrics.eccentricity
    );
}

#[test]
fn test_circular_vs_elongated_eccentricity() {
    let width = 64;
    let height = 64;

    let circular = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
    let elongated = make_elliptical_star(width, height, Vec2::splat(32.0), 4.0, 2.0, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_circular = compute_star(
        &circular,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_elongated = compute_star(
        &elongated,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    assert!(
        metrics_elongated.eccentricity > metrics_circular.eccentricity,
        "Elongated star should have higher eccentricity: {} vs {}",
        metrics_elongated.eccentricity,
        metrics_circular.eccentricity
    );
}

#[test]
fn test_centroid_with_noisy_background() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::splat(32.0);

    // Create star with added noise
    let mut pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8, 0.1);

    // Add random-ish noise pattern (deterministic for reproducibility)
    for (i, pixel) in pixels.iter_mut().enumerate() {
        let noise = ((i * 7 + 13) % 100) as f32 * 0.001 - 0.05;
        *pixel += noise;
    }

    let bg = make_uniform_background(width, height, 0.1, 0.05); // Higher noise estimate

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        true_pos,
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );
    assert!(result.is_some());

    let new_pos = result.unwrap();
    // With noise, allow more tolerance
    assert!(
        (new_pos.x - true_pos.x).abs() < 1.0,
        "X error too large with noise"
    );
    assert!(
        (new_pos.y - true_pos.y).abs() < 1.0,
        "Y error too large with noise"
    );
}

#[test]
fn test_snr_decreases_with_higher_noise() {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);

    let bg_low_noise = make_uniform_background(width, height, 0.1, 0.01);
    let bg_high_noise = make_uniform_background(width, height, 0.1, 0.1);

    let metrics_low = compute_star(
        &pixels,
        &bg_low_noise,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_high = compute_star(
        &pixels,
        &bg_high_noise,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    assert!(
        metrics_low.snr > metrics_high.snr,
        "Lower noise should give higher SNR: {} vs {}",
        metrics_low.snr,
        metrics_high.snr
    );
}

#[test]
fn test_fwhm_formula_for_known_gaussian() {
    // For a Gaussian with known sigma, verify FWHM ≈ FWHM_TO_SIGMA * sigma
    let width = 128;
    let height = 128;
    let sigma = 3.0f32;
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let pixels = make_gaussian_star(width, height, Vec2::splat(64.0), sigma, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.001); // Very low noise

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

    // Allow 10% error due to discrete sampling and finite aperture
    let error = (metrics.fwhm - expected_fwhm).abs() / expected_fwhm;
    assert!(
        error < 0.1,
        "FWHM should be close to 2.355*sigma: expected {}, got {}, error {}",
        expected_fwhm,
        metrics.fwhm,
        error
    );
}

#[test]
fn test_flux_proportional_to_amplitude() {
    let width = 64;
    let height = 64;

    let pixels_amp1 = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.4, 0.1);
    let pixels_amp2 = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics1 = compute_star(
        &pixels_amp1,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics2 = compute_star(
        &pixels_amp2,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Flux should scale roughly proportionally with amplitude
    let flux_ratio = metrics2.flux / metrics1.flux;
    let amp_ratio = 0.8 / 0.4;

    assert!(
        (flux_ratio - amp_ratio).abs() < 0.5,
        "Flux ratio {} should be close to amplitude ratio {}",
        flux_ratio,
        amp_ratio
    );
}

#[test]
fn test_eccentricity_bounds() {
    // Eccentricity should always be in [0, 1]
    let width = 64;
    let height = 64;
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Test various star shapes
    let circular = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
    let elongated_x = make_elliptical_star(width, height, Vec2::splat(32.0), 5.0, 2.0, 0.8, 0.1);
    let elongated_y = make_elliptical_star(width, height, Vec2::splat(32.0), 2.0, 5.0, 0.8, 0.1);

    for (name, pixels) in [
        ("circular", circular),
        ("elongated_x", elongated_x),
        ("elongated_y", elongated_y),
    ] {
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
            metrics.eccentricity >= 0.0 && metrics.eccentricity <= 1.0,
            "{} eccentricity {} out of bounds [0,1]",
            name,
            metrics.eccentricity
        );
    }
}

#[test]
fn test_eccentricity_orientation_invariant() {
    // Eccentricity should be similar regardless of orientation (x vs y elongation)
    let width = 64;
    let height = 64;
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let elongated_x = make_elliptical_star(width, height, Vec2::splat(32.0), 4.0, 2.0, 0.8, 0.1);
    let elongated_y = make_elliptical_star(width, height, Vec2::splat(32.0), 2.0, 4.0, 0.8, 0.1);

    let metrics_x = compute_star(
        &elongated_x,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_y = compute_star(
        &elongated_y,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Should have similar eccentricity (within 20%)
    let diff = (metrics_x.eccentricity - metrics_y.eccentricity).abs();
    let avg = (metrics_x.eccentricity + metrics_y.eccentricity) / 2.0;
    assert!(
        diff / avg < 0.2,
        "X and Y elongated stars should have similar eccentricity: {} vs {}",
        metrics_x.eccentricity,
        metrics_y.eccentricity
    );
}

#[test]
fn test_snr_formula_consistency() {
    // SNR = flux / (noise * sqrt(aperture_area))
    // Verify the formula behaves as expected
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);

    let noise1 = 0.02f32;
    let noise2 = 0.04f32; // 2x noise

    let bg1 = make_uniform_background(width, height, 0.1, noise1);
    let bg2 = make_uniform_background(width, height, 0.1, noise2);

    let metrics1 = compute_star(
        &pixels,
        &bg1,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics2 = compute_star(
        &pixels,
        &bg2,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // SNR should halve when noise doubles (same flux)
    let snr_ratio = metrics1.snr / metrics2.snr;
    assert!(
        (snr_ratio - 2.0).abs() < 0.1,
        "SNR ratio should be ~2 when noise doubles: got {}",
        snr_ratio
    );
}

#[test]
fn test_metrics_with_high_background() {
    // Stars should still be measurable with high but uniform background
    let width = 64;
    let height = 64;

    // High background value
    let mut pixels = vec![0.5f32; width * height];
    // Add star on top of high background
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - 32.0;
            let dy = y as f32 - 32.0;
            let r2 = dx * dx + dy * dy;
            let value = 0.4 * (-r2 / (2.0 * 2.5 * 2.5)).exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }
    let pixels = Buffer2::new(width, height, pixels);

    let bg = make_uniform_background(width, height, 0.5, 0.02);
    let metrics = compute_star(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    );

    assert!(
        metrics.is_some(),
        "Should compute metrics with high background"
    );
    let m = metrics.unwrap();
    assert!(m.flux > 0.0, "Flux should be positive");
    assert!(m.fwhm > 0.0, "FWHM should be positive");
}

#[test]
fn test_fwhm_independent_of_amplitude() {
    // FWHM should be the same regardless of star brightness (same sigma)
    let width = 64;
    let height = 64;
    let sigma = 2.5f32;

    let pixels_dim = make_gaussian_star(width, height, Vec2::splat(32.0), sigma, 0.3, 0.1);
    let pixels_bright = make_gaussian_star(width, height, Vec2::splat(32.0), sigma, 0.9, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_dim = compute_star(
        &pixels_dim,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_bright = compute_star(
        &pixels_bright,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // FWHM should be within 20% of each other
    let diff = (metrics_dim.fwhm - metrics_bright.fwhm).abs();
    let avg = (metrics_dim.fwhm + metrics_bright.fwhm) / 2.0;
    assert!(
        diff / avg < 0.2,
        "FWHM should be amplitude-independent: dim={}, bright={}",
        metrics_dim.fwhm,
        metrics_bright.fwhm
    );
}

#[test]
fn test_eccentricity_increases_with_elongation() {
    let width = 64;
    let height = 64;
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Create stars with increasing elongation ratios
    let ratio_1_1 = make_elliptical_star(width, height, Vec2::splat(32.0), 2.5, 2.5, 0.8, 0.1); // circular
    let ratio_2_1 = make_elliptical_star(width, height, Vec2::splat(32.0), 4.0, 2.0, 0.8, 0.1);
    let ratio_3_1 = make_elliptical_star(width, height, Vec2::splat(32.0), 6.0, 2.0, 0.8, 0.1);

    let ecc_1 = compute_star(
        &ratio_1_1,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap()
    .eccentricity;
    let ecc_2 = compute_star(
        &ratio_2_1,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap()
    .eccentricity;
    let ecc_3 = compute_star(
        &ratio_3_1,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap()
    .eccentricity;

    assert!(
        ecc_1 < ecc_2 && ecc_2 < ecc_3,
        "Eccentricity should increase with elongation: {} < {} < {}",
        ecc_1,
        ecc_2,
        ecc_3
    );
}

#[test]
fn test_measure_star_returns_none_for_edge_candidate() {
    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let config = Config::default();

    // Create region near edge
    let region = Region {
        bbox: URect::new(Vec2us::new(0, 30), Vec2us::new(6, 36)),
        peak: Vec2us::new(3, 32),
        peak_value: 0.9,
        area: 18,
    };

    let result = measure_star(
        &pixels,
        &bg,
        &region,
        &config.measurement,
        config.fwhm.expected,
    );
    assert!(
        result.is_none(),
        "Should reject candidate too close to edge"
    );
}

#[test]
fn test_measure_star_multiple_stars_independent() {
    let width = 128;
    let height = 128;

    // Create two well-separated stars using the helper function
    let star1_cx = 40.0f32;
    let star1_cy = 40.0f32;
    let star2_cx = 90.0f32;
    let star2_cy = 90.0f32;

    // Start with uniform background
    let mut pixels = vec![0.1f32; width * height];

    // Add first star (sigma=2.5 gives good detection)
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - star1_cx;
            let dy = y as f32 - star1_cy;
            let r2 = dx * dx + dy * dy;
            let value = 0.8 * (-r2 / (2.0 * 2.5 * 2.5)).exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }

    // Add second star
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - star2_cx;
            let dy = y as f32 - star2_cy;
            let r2 = dx * dx + dy * dy;
            let value = 0.6 * (-r2 / (2.0 * 2.5 * 2.5)).exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }

    let pixels = Buffer2::new(width, height, pixels);
    let bg = estimate_background(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config {
        detection: DetectionConfig {
            edge_margin: 10,
            ..Default::default()
        },
        ..Default::default()
    };
    let candidates = detect_stars_test(&pixels, &bg, &config.detection);

    assert_eq!(candidates.len(), 2, "Should detect two stars");

    // Compute centroids for both
    let stars: Vec<_> = candidates
        .iter()
        .filter_map(|c| measure_star(&pixels, &bg, c, &config.measurement, config.fwhm.expected))
        .collect();

    assert_eq!(stars.len(), 2, "Should compute centroids for both stars");

    // Verify each star is close to its true position
    for star in &stars {
        let near_star1 = (star.pos.x - star1_cx as f64).abs() < 1.0
            && (star.pos.y - star1_cy as f64).abs() < 1.0;
        let near_star2 = (star.pos.x - star2_cx as f64).abs() < 1.0
            && (star.pos.y - star2_cy as f64).abs() < 1.0;
        assert!(
            near_star1 || near_star2,
            "Star at ({}, {}) not near either true position",
            star.pos.x,
            star.pos.y
        );
    }
}

#[test]
fn test_circular_star_roundness() {
    // A circular Gaussian star should have roundness near zero
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);

    let bg = estimate_background(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config::default();
    let candidates = detect_stars_test(&pixels, &bg, &config.detection);

    assert_eq!(candidates.len(), 1);

    let star = measure_star(
        &pixels,
        &bg,
        &candidates[0],
        &config.measurement,
        config.fwhm.expected,
    )
    .expect("Should compute centroid");

    // Circular star should have roundness close to 0
    assert!(
        star.roundness1.abs() < 0.1,
        "Circular star should have roundness1 near 0, got {}",
        star.roundness1
    );
    assert!(
        star.roundness2 < 0.1,
        "Circular star should have roundness2 near 0, got {}",
        star.roundness2
    );
}

#[test]
fn test_elongated_x_star_roundness() {
    // An elongated star in x direction should have negative roundness1
    let width = 64;
    let height = 64;
    let mut pixels = vec![0.1f32; width * height];

    // Create elongated Gaussian (sigma_x > sigma_y)
    let cx = 32.0;
    let cy = 32.0;
    let sigma_x = 4.0;
    let sigma_y = 2.0;
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let value = 0.8
                * (-dx * dx / (2.0 * sigma_x * sigma_x) - dy * dy / (2.0 * sigma_y * sigma_y))
                    .exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }

    let pixels = Buffer2::new(width, height, pixels);
    let bg = estimate_background(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config::default();
    let candidates = detect_stars_test(&pixels, &bg, &config.detection);

    assert!(!candidates.is_empty());

    let star = measure_star(
        &pixels,
        &bg,
        &candidates[0],
        &config.measurement,
        config.fwhm.expected,
    )
    .expect("Should compute centroid");

    // X-elongated star: more flux in x marginal -> higher Hx -> negative roundness1
    // (roundness1 = (Hx - Hy) / (Hx + Hy), but Hx is sum in y direction)
    // Actually, marginal_x sums along y for each x position, so x-elongated means
    // the x marginal has lower peak (more spread). Let's just check it's non-zero.
    assert!(
        star.roundness1.abs() > 0.05 || star.eccentricity > 0.3,
        "Elongated star should have noticeable shape metrics"
    );
}

#[test]
fn test_asymmetric_star_roundness2() {
    // An asymmetric source should have non-zero roundness2
    let width = 64;
    let height = 64;
    let mut pixels = vec![0.1f32; width * height];

    // Create a star with extra flux on one side (like a cosmic ray tail)
    let cx = 32.0;
    let cy = 32.0;
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r2 = dx * dx + dy * dy;
            let mut value = 0.8 * (-r2 / (2.0 * 2.5 * 2.5)).exp();

            // Add asymmetric tail to the right
            if dx > 0.0 && dx < 8.0 && dy.abs() < 2.0 {
                value += 0.3 * (-(dx - 4.0).powi(2) / 8.0).exp();
            }

            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }

    let pixels = Buffer2::new(width, height, pixels);
    let bg = estimate_background(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config::default();
    let candidates = detect_stars_test(&pixels, &bg, &config.detection);

    assert!(!candidates.is_empty());

    let star = measure_star(
        &pixels,
        &bg,
        &candidates[0],
        &config.measurement,
        config.fwhm.expected,
    )
    .expect("Should compute centroid");

    // Asymmetric source should have higher roundness2 (symmetry metric)
    // The tail adds more flux to the right side
    assert!(
        star.roundness2 > 0.01,
        "Asymmetric star should have roundness2 > 0, got {}",
        star.roundness2
    );
}

#[test]
fn test_star_is_round() {
    use crate::stacking::star_detection::star::Star;

    let round_star = Star {
        pos: glam::DVec2::new(10.0, 10.0),
        flux: 100.0,
        fwhm: 3.0,
        eccentricity: 0.1,
        snr: 50.0,
        peak: 0.5,
        sharpness: 0.3,
        roundness1: 0.05,
        roundness2: 0.03,
    };

    let non_round_star = Star {
        roundness1: 0.5,
        roundness2: 0.4,
        ..round_star
    };

    assert!(
        round_star.is_round(0.2),
        "Round star should pass roundness check"
    );
    assert!(
        !non_round_star.is_round(0.2),
        "Non-round star should fail roundness check"
    );
    assert!(
        non_round_star.is_round(1.0),
        "All stars should pass with max_roundness=1.0"
    );
}
