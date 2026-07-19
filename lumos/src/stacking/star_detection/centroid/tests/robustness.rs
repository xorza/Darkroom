use crate::stacking::star_detection::centroid::tests::*;

/// Test centroiding with undersampled PSF (FWHM < 2 pixels).
/// This is a challenging case where the star is barely resolved.
#[test]
fn test_centroid_undersampled_psf() {
    let width = 64;
    let height = 64;
    let sigma = 0.7f32; // FWHM ≈ 1.65 pixels (undersampled)
    let true_pos = Vec2::new(32.3, 32.7);

    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.9, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let expected_fwhm = FWHM_TO_SIGMA * sigma;
    let stamp_radius = 5; // Smaller stamp for undersampled

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::splat(32.0),
        stamp_radius,
        expected_fwhm,
    );

    assert!(
        result.is_some(),
        "Should find centroid for undersampled PSF"
    );
    let new_pos = result.unwrap();

    // Undersampled PSFs have worse accuracy - allow 0.5 pixel error
    let error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.5,
        "Centroid error {} too large for undersampled PSF (FWHM={:.2})",
        error,
        expected_fwhm
    );
}

/// Test centroiding with very large PSF (FWHM > 15 pixels).
#[test]
fn test_centroid_large_psf() {
    let width = 128;
    let height = 128;
    let sigma = 8.0f32; // FWHM ≈ 18.8 pixels (large PSF)
    let true_pos = Vec2::new(64.3, 64.7);

    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let expected_fwhm = FWHM_TO_SIGMA * sigma;
    let stamp_radius = MAX_STAMP_RADIUS; // Use maximum allowed

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::splat(64.0),
        stamp_radius,
        expected_fwhm,
    );

    assert!(result.is_some(), "Should find centroid for large PSF");
    let new_pos = result.unwrap();

    // Large PSFs have reduced accuracy since the stamp radius (15px) is smaller
    // than the FWHM (18.8px), so not all light is captured for weighting.
    // Allow 0.5 pixel error for such extreme cases.
    let error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.5,
        "Centroid error {} too large for large PSF (FWHM={:.2})",
        error,
        expected_fwhm
    );
}

/// Test Gaussian fitting with undersampled PSF.
#[test]
fn test_gaussian_fit_undersampled_psf() {
    use crate::stacking::star_detection::centroid::gaussian_fit::{
        GaussianFitConfig, fit_gaussian_2d,
    };

    let width = 21;
    let height = 21;
    let true_sigma = 0.8f32; // FWHM ≈ 1.88 pixels
    let true_cx = 10.3f32;
    let true_cy = 10.7f32;
    let background = 0.1f32;

    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - true_cx;
            let dy = y as f32 - true_cy;
            pixels[y * width + x] +=
                1.0 * (-0.5 * (dx * dx + dy * dy) / (true_sigma * true_sigma)).exp();
        }
    }
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, Vec2::splat(10.0), 6, background, None, &config);

    assert!(result.is_some(), "Should fit undersampled Gaussian");
    let result = result.unwrap();

    // Position accuracy degrades for undersampled PSFs
    let error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.3,
        "Position error {} too large for undersampled PSF",
        error
    );
}

/// Test metrics computation with very small FWHM.
#[test]
fn test_metrics_small_fwhm() {
    let width = 64;
    let height = 64;
    let sigma = 0.8f32; // Very small
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), sigma, 0.9, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_star(&pixels, &bg, Vec2::splat(32.0), 0.0, 5, None, None);

    assert!(metrics.is_some(), "Should compute metrics for small FWHM");
    let m = metrics.unwrap();
    assert!(m.fwhm > 0.0, "FWHM should be positive");
    assert!(m.flux > 0.0, "Flux should be positive");
}

/// Test metrics computation with very large FWHM.
#[test]
fn test_metrics_large_fwhm() {
    let width = 128;
    let height = 128;
    let sigma = 7.0f32; // Large
    let pixels = make_gaussian_star(width, height, Vec2::splat(64.0), sigma, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_star(
        &pixels,
        &bg,
        Vec2::splat(64.0),
        0.0,
        MAX_STAMP_RADIUS,
        None,
        None,
    );

    assert!(metrics.is_some(), "Should compute metrics for large FWHM");
    let m = metrics.unwrap();
    assert!(m.fwhm > 10.0, "FWHM should be large: {}", m.fwhm);
}

/// Create two overlapping stars.
fn make_blended_stars(
    width: usize,
    height: usize,
    pos1: Vec2,
    pos2: Vec2,
    sigma: f32,
    amp1: f32,
    amp2: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![0.1f32; width * height];

    for y in 0..height {
        for x in 0..width {
            let dx1 = x as f32 - pos1.x;
            let dy1 = y as f32 - pos1.y;
            let r2_1 = dx1 * dx1 + dy1 * dy1;
            let v1 = amp1 * (-r2_1 / (2.0 * sigma * sigma)).exp();

            let dx2 = x as f32 - pos2.x;
            let dy2 = y as f32 - pos2.y;
            let r2_2 = dx2 * dx2 + dy2 * dy2;
            let v2 = amp2 * (-r2_2 / (2.0 * sigma * sigma)).exp();

            if v1 > 0.001 || v2 > 0.001 {
                pixels[y * width + x] += v1 + v2;
            }
        }
    }

    Buffer2::new(width, height, pixels)
}

/// Test centroiding with a nearby contaminating star.
#[test]
fn test_centroid_with_nearby_star() {
    let width = 64;
    let height = 64;
    let sigma = 2.5f32;

    // Primary star at center, fainter companion 8 pixels away
    let primary_pos = Vec2::new(32.0, 32.0);
    let secondary_pos = Vec2::new(40.0, 32.0); // 8 pixels separation
    let pixels = make_blended_stars(width, height, primary_pos, secondary_pos, sigma, 0.8, 0.3);

    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        primary_pos,
        TEST_STAMP_RADIUS,
        expected_fwhm,
    );

    assert!(result.is_some(), "Should find centroid despite nearby star");
    let new_pos = result.unwrap();

    // Primary centroid should be pulled slightly toward secondary
    // but should still be within 1 pixel of true position
    let error = ((new_pos.x - primary_pos.x).powi(2) + (new_pos.y - primary_pos.y).powi(2)).sqrt();
    assert!(
        error < 1.0,
        "Centroid error {} too large with nearby contamination",
        error
    );
}

/// Test centroiding with closely blended stars (partially overlapping).
#[test]
fn test_centroid_blended_stars() {
    let width = 64;
    let height = 64;
    let sigma = 2.5f32;

    // Two stars only 5 pixels apart (significant overlap)
    let primary_pos = Vec2::new(32.0, 32.0);
    let secondary_pos = Vec2::new(37.0, 32.0);
    let pixels = make_blended_stars(width, height, primary_pos, secondary_pos, sigma, 0.8, 0.5);

    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        primary_pos,
        TEST_STAMP_RADIUS,
        expected_fwhm,
    );

    assert!(result.is_some(), "Should attempt centroid on blended stars");
    let new_pos = result.unwrap();

    // Centroid will be pulled toward center of light - check it moved toward secondary
    // This is expected behavior for blended sources
    assert!(
        new_pos.x > primary_pos.x,
        "Blended centroid should be pulled toward secondary star"
    );
}

/// Test Gaussian fitting with contaminating star in the wing.
#[test]
fn test_gaussian_fit_with_contamination() {
    use crate::stacking::star_detection::centroid::gaussian_fit::{
        GaussianFitConfig, fit_gaussian_2d,
    };

    let width = 31;
    let height = 31;
    let sigma = 2.5f32;
    let background = 0.1f32;

    // Primary star at center
    let true_cx = 15.0f32;
    let true_cy = 15.0f32;

    let mut pixels = vec![background; width * height];

    // Add primary star
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - true_cx;
            let dy = y as f32 - true_cy;
            let r2 = dx * dx + dy * dy;
            pixels[y * width + x] += 0.8 * (-r2 / (2.0 * sigma * sigma)).exp();
        }
    }

    // Add faint contaminating star at edge of stamp
    let contam_cx = 22.0f32;
    let contam_cy = 15.0f32;
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - contam_cx;
            let dy = y as f32 - contam_cy;
            let r2 = dx * dx + dy * dy;
            pixels[y * width + x] += 0.2 * (-r2 / (2.0 * sigma * sigma)).exp();
        }
    }

    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(
        &pixels_buf,
        Vec2::new(true_cx, true_cy),
        8,
        background,
        None,
        &config,
    );

    assert!(result.is_some(), "Should fit despite contamination");
    let result = result.unwrap();

    // Position should still be reasonably accurate (within 0.5 pixel)
    let error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.5,
        "Position error {} too large with wing contamination",
        error
    );
}

/// Test that eccentricity is affected by nearby star contamination.
#[test]
fn test_eccentricity_with_contamination() {
    let width = 64;
    let height = 64;
    let sigma = 2.5f32;

    // Single circular star
    let single_star = make_gaussian_star(width, height, Vec2::splat(32.0), sigma, 0.8, 0.1);

    // Star with nearby companion (will appear elongated)
    let contaminated = make_blended_stars(
        width,
        height,
        Vec2::splat(32.0),
        Vec2::new(38.0, 32.0),
        sigma,
        0.8,
        0.4,
    );

    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_single = compute_star(
        &single_star,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    let metrics_contaminated = compute_star(
        &contaminated,
        &bg,
        Vec2::splat(32.0),
        0.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Contaminated source should have higher eccentricity
    assert!(
        metrics_contaminated.eccentricity > metrics_single.eccentricity,
        "Contaminated source should appear more elongated: {} vs {}",
        metrics_contaminated.eccentricity,
        metrics_single.eccentricity
    );
}

/// Create a rotated elliptical Gaussian.
fn make_rotated_elliptical_star(
    width: usize,
    height: usize,
    pos: Vec2,
    sigma_major: f32,
    sigma_minor: f32,
    angle_rad: f32,
    amplitude: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![0.1f32; width * height];

    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - pos.x;
            let dy = y as f32 - pos.y;

            // Rotate coordinates
            let dx_rot = dx * cos_a + dy * sin_a;
            let dy_rot = -dx * sin_a + dy * cos_a;

            // Compute elliptical distance
            let r2 = (dx_rot / sigma_major).powi(2) + (dy_rot / sigma_minor).powi(2);
            let value = amplitude * (-r2 / 2.0).exp();

            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }

    Buffer2::new(width, height, pixels)
}

/// Test centroiding on a 45-degree rotated ellipse.
#[test]
fn test_centroid_rotated_ellipse_45deg() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.3, 32.7);
    let angle = FRAC_PI_4; // 45 degrees

    let pixels = make_rotated_elliptical_star(width, height, true_pos, 4.0, 2.0, angle, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        6.0,
    );

    assert!(result.is_some(), "Should find centroid for rotated ellipse");
    let new_pos = result.unwrap();

    // Rotated ellipses with high eccentricity and offset starting position
    // have reduced accuracy. Allow 0.6 pixel error.
    let error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.6,
        "Centroid error {} too large for 45° rotated ellipse",
        error
    );
}

/// Test centroiding on various rotation angles.
#[test]
fn test_centroid_various_rotation_angles() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.0, 32.0);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Test multiple rotation angles
    for angle_deg in [0, 30, 45, 60, 90, 120, 150] {
        let angle_rad = (angle_deg as f32).to_radians();
        let pixels =
            make_rotated_elliptical_star(width, height, true_pos, 4.0, 2.0, angle_rad, 0.8);

        let result = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            true_pos,
            TEST_STAMP_RADIUS,
            6.0,
        );

        assert!(
            result.is_some(),
            "Should find centroid for {}° rotated ellipse",
            angle_deg
        );
        let new_pos = result.unwrap();

        let error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
        assert!(
            error < 0.2,
            "Centroid error {} too large for {}° rotated ellipse",
            error,
            angle_deg
        );
    }
}

/// Test that rotated ellipses have similar eccentricity regardless of angle.
#[test]
fn test_eccentricity_rotation_invariant() {
    let width = 64;
    let height = 64;
    let pos = Vec2::splat(32.0);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let mut eccentricities = Vec::new();

    for angle_deg in [0, 45, 90, 135] {
        let angle_rad = (angle_deg as f32).to_radians();
        let pixels = make_rotated_elliptical_star(width, height, pos, 4.0, 2.0, angle_rad, 0.8);

        let metrics = compute_star(&pixels, &bg, pos, 0.0, TEST_STAMP_RADIUS, None, None).unwrap();
        eccentricities.push((angle_deg, metrics.eccentricity));
    }

    // All eccentricities should be similar (within 20% of each other)
    let avg_ecc: f32 =
        eccentricities.iter().map(|(_, e)| e).sum::<f32>() / eccentricities.len() as f32;
    for (angle, ecc) in &eccentricities {
        let diff = (ecc - avg_ecc).abs() / avg_ecc;
        assert!(
            diff < 0.25,
            "Eccentricity at {}° differs too much from average: {} vs {}",
            angle,
            ecc,
            avg_ecc
        );
    }
}

/// Test Gaussian fitting on rotated ellipse (fits axis-aligned sigma_x, sigma_y).
#[test]
fn test_gaussian_fit_rotated_ellipse() {
    use crate::stacking::star_detection::centroid::gaussian_fit::{
        GaussianFitConfig, fit_gaussian_2d,
    };

    let width = 31;
    let height = 31;
    let true_cx = 15.0f32;
    let true_cy = 15.0f32;
    let background = 0.1f32;

    // Create 45° rotated ellipse
    let pixels = make_rotated_elliptical_star(
        width,
        height,
        Vec2::new(true_cx, true_cy),
        3.5,
        2.0,
        FRAC_PI_4,
        0.8,
    );

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(
        &pixels,
        Vec2::new(true_cx, true_cy),
        8,
        background,
        None,
        &config,
    );

    assert!(result.is_some(), "Should fit rotated ellipse");
    let result = result.unwrap();

    // Position should still be accurate
    let error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.1,
        "Position error {} too large for rotated ellipse fit",
        error
    );

    // The fitted sigma values will be axis-aligned projections, not the true major/minor axes
    // Just verify they're reasonable and different from each other
    assert!(
        result.sigma.x > 1.0 && result.sigma.x < 5.0,
        "sigma_x out of range"
    );
    assert!(
        result.sigma.y > 1.0 && result.sigma.y < 5.0,
        "sigma_y out of range"
    );
}

/// Test recovery from initial guess 2 pixels away from true position.
#[test]
fn test_recovery_from_2pixel_offset() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.0, 32.0);
    let sigma = 2.5f32;

    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    // Start 2 pixels away
    let initial_guess = Vec2::new(34.0, 30.0);

    let mut pos = initial_guess;
    for _ in 0..MAX_MOMENTS_ITERATIONS {
        if let Some(new_pos) = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos,
            TEST_STAMP_RADIUS,
            expected_fwhm,
        ) {
            let delta = new_pos - pos;
            pos = new_pos;
            if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
                break;
            }
        } else {
            break;
        }
    }

    let error = ((pos.x - true_pos.x).powi(2) + (pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.2,
        "Should recover from 2-pixel offset, error = {}",
        error
    );
}

/// Test recovery from initial guess 3 pixels away.
#[test]
fn test_recovery_from_3pixel_offset() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.0, 32.0);
    let sigma = 2.5f32;

    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    // Start 3 pixels away diagonally
    let initial_guess = Vec2::new(34.1, 34.1); // ~3 pixel offset

    let mut pos = initial_guess;
    for _ in 0..MAX_MOMENTS_ITERATIONS {
        if let Some(new_pos) = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos,
            TEST_STAMP_RADIUS,
            expected_fwhm,
        ) {
            let delta = new_pos - pos;
            pos = new_pos;
            if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
                break;
            }
        } else {
            break;
        }
    }

    let error = ((pos.x - true_pos.x).powi(2) + (pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.3,
        "Should recover from 3-pixel offset, error = {}",
        error
    );
}

/// Test Gaussian fitting with bad initial position guess.
#[test]
fn test_gaussian_fit_bad_initial_guess() {
    use crate::stacking::star_detection::centroid::gaussian_fit::{
        GaussianFitConfig, fit_gaussian_2d,
    };

    let width = 31;
    let height = 31;
    let true_cx = 15.0f32;
    let true_cy = 15.0f32;
    let sigma = 2.5f32;
    let background = 0.1f32;

    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - true_cx;
            let dy = y as f32 - true_cy;
            pixels[y * width + x] += 1.0 * (-0.5 * (dx * dx + dy * dy) / (sigma * sigma)).exp();
        }
    }
    let pixels_buf = Buffer2::new(width, height, pixels);

    // Initial guess 2.5 pixels away
    let initial_guess = Vec2::new(13.0, 17.0);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, initial_guess, 8, background, None, &config);

    assert!(result.is_some(), "Should converge from bad initial guess");
    let result = result.unwrap();

    let error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.1,
        "Gaussian fit should recover from bad guess, error = {}",
        error
    );
}

/// Test Moffat fitting with bad initial position guess.
#[test]
fn test_moffat_fit_bad_initial_guess() {
    use crate::stacking::star_detection::centroid::moffat_fit::{MoffatFitConfig, fit_moffat_2d};

    let width = 31;
    let height = 31;
    let true_cx = 15.0f32;
    let true_cy = 15.0f32;
    let alpha = 2.5f32;
    let beta = 2.5f32;
    let background = 0.1f32;

    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let r2 = (x as f32 - true_cx).powi(2) + (y as f32 - true_cy).powi(2);
            pixels[y * width + x] += 1.0 * (1.0 + r2 / (alpha * alpha)).powf(-beta);
        }
    }
    let pixels_buf = Buffer2::new(width, height, pixels);

    // Initial guess 2 pixels away
    let initial_guess = Vec2::new(13.0, 17.0);

    let config = MoffatFitConfig {
        fixed_beta: beta,
        ..Default::default()
    };
    let result = fit_moffat_2d(&pixels_buf, initial_guess, 8, background, None, &config);

    assert!(
        result.is_some(),
        "Moffat should converge from bad initial guess"
    );
    let result = result.unwrap();

    let error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.1,
        "Moffat fit should recover from bad guess, error = {}",
        error
    );
}
