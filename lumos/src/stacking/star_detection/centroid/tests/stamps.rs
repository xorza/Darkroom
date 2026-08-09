use crate::math::size2us::Size2us;
use crate::stacking::star_detection::centroid::tests::*;
use crate::stacking::star_detection::centroid::{StampGrid, compute_stamp_radius};

#[test]
fn test_estimate_sigma_from_moments_gaussian() {
    use crate::stacking::star_detection::centroid::estimate_sigma_from_moments;

    let width = 21;
    let height = 21;
    let cx = 10.0f64;
    let cy = 10.0f64;
    let true_sigma = 2.5f32;
    let background = 0.1f32;

    // Create Gaussian star
    let mut data_x = Vec::new();
    let mut data_y = Vec::new();
    let mut data_z = Vec::new();

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx as f32;
            let dy = y as f32 - cy as f32;
            let value =
                background + 1.0 * (-0.5 * (dx * dx + dy * dy) / (true_sigma * true_sigma)).exp();
            data_x.push(x as f64);
            data_y.push(y as f64);
            data_z.push(value as f64);
        }
    }

    let estimated_sigma = estimate_sigma_from_moments(
        &data_x,
        &data_y,
        &data_z,
        DVec2::new(cx, cy),
        background,
        10.0,
    );

    // Should be within 20% of true sigma
    let error = (estimated_sigma - true_sigma).abs() / true_sigma;
    assert!(
        error < 0.2,
        "Sigma estimate error {:.1}% too large (expected={}, got={})",
        error * 100.0,
        true_sigma,
        estimated_sigma
    );
}

#[test]
fn test_estimate_sigma_from_moments_various_sigmas() {
    use crate::stacking::star_detection::centroid::estimate_sigma_from_moments;

    let width = 21;
    let height = 21;
    let cx = 10.0f64;
    let cy = 10.0f64;
    let background = 0.1f32;

    for true_sigma in [1.5f32, 2.0, 2.5, 3.0, 4.0] {
        let mut data_x = Vec::new();
        let mut data_y = Vec::new();
        let mut data_z = Vec::new();

        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx as f32;
                let dy = y as f32 - cy as f32;
                let value = background
                    + 1.0 * (-0.5 * (dx * dx + dy * dy) / (true_sigma * true_sigma)).exp();
                data_x.push(x as f64);
                data_y.push(y as f64);
                data_z.push(value as f64);
            }
        }

        let estimated_sigma = estimate_sigma_from_moments(
            &data_x,
            &data_y,
            &data_z,
            DVec2::new(cx, cy),
            background,
            10.0,
        );

        let error = (estimated_sigma - true_sigma).abs() / true_sigma;
        assert!(
            error < 0.25,
            "Sigma={}: estimate error {:.1}% too large (got={})",
            true_sigma,
            error * 100.0,
            estimated_sigma
        );
    }
}

#[test]
fn test_refine_centroid_adaptive_sigma_small_fwhm() {
    let width = 64;
    let height = 64;
    let true_pos = DVec2::new(32.3, 32.7);
    let sigma = 1.5f32; // Small sigma
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let pixels = SyntheticStar::new(true_pos.as_vec2(), 0.8, StarProfile::Gaussian { sigma })
        .stamp(Size2us::new(width, height), 0.1);
    let bg = make_uniform_background(Size2us::new(width, height), 0.1, 0.01);

    // Use small expected FWHM
    let result = refine_centroid(
        &pixels,
        &bg,
        DVec2::splat(32.0),
        TEST_STAMP_RADIUS,
        expected_fwhm,
    );

    assert!(result.is_some());
    let new_pos = result.unwrap();

    // Should converge towards true position
    let error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.5,
        "Centroid error {} too large for small FWHM",
        error
    );
}

#[test]
fn test_refine_centroid_adaptive_sigma_large_fwhm() {
    let width = 64;
    let height = 64;
    let true_pos = DVec2::new(32.3, 32.7);
    let sigma = 4.0f32; // Large sigma
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let pixels = SyntheticStar::new(true_pos.as_vec2(), 0.8, StarProfile::Gaussian { sigma })
        .stamp(Size2us::new(width, height), 0.1);
    let bg = make_uniform_background(Size2us::new(width, height), 0.1, 0.01);

    // Use large expected FWHM
    let result = refine_centroid(
        &pixels,
        &bg,
        DVec2::splat(32.0),
        TEST_STAMP_RADIUS,
        expected_fwhm,
    );

    assert!(result.is_some());
    let new_pos = result.unwrap();

    // Should converge towards true position
    let error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.5,
        "Centroid error {} too large for large FWHM",
        error
    );
}

#[test]
fn test_extract_stamp_valid_center() {
    use crate::stacking::star_detection::centroid::extract_stamp;

    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    let result = extract_stamp(&pixels, DVec2::splat(32.0), 5);
    assert!(result.is_some(), "Should extract stamp at center");

    let stamp = result.unwrap();
    let expected_size = (2 * 5 + 1) * (2 * 5 + 1); // 11x11 = 121
    assert_eq!(stamp.z.len(), expected_size);
    // Coordinates live in the shared `StampGrid`; what the stamp itself pins is where its
    // top-left pixel sits, which is what the grid's `0..2r` is relative to.
    assert_eq!(stamp.origin, DVec2::new(27.0, 27.0));
    assert!(
        (stamp.peak - 0.5).abs() < f32::EPSILON,
        "Peak should be 0.5"
    );
}

#[test]
fn test_extract_stamp_edge_invalid() {
    use crate::stacking::star_detection::centroid::extract_stamp;

    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    // Too close to edges
    assert!(extract_stamp(&pixels, DVec2::new(3.0, 32.0), 5).is_none());
    assert!(extract_stamp(&pixels, DVec2::new(32.0, 3.0), 5).is_none());
    assert!(extract_stamp(&pixels, DVec2::new(61.0, 32.0), 5).is_none());
    assert!(extract_stamp(&pixels, DVec2::new(32.0, 61.0), 5).is_none());
}

#[test]
fn test_extract_stamp_peak_value() {
    use crate::stacking::star_detection::centroid::extract_stamp;

    let width = 64;
    let height = 64;
    let mut pixels = vec![0.1f32; width * height];
    // Add bright pixel at center
    pixels[32 * width + 32] = 0.9;
    let pixels = Buffer2::new(width, height, pixels);

    let result = extract_stamp(&pixels, DVec2::splat(32.0), 5);
    assert!(result.is_some());

    let stamp = result.unwrap();
    assert!(
        (stamp.peak - 0.9).abs() < f32::EPSILON,
        "Peak should be 0.9"
    );
}

#[test]
fn test_extract_stamp_coordinates() {
    use crate::stacking::star_detection::centroid::extract_stamp;

    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    let result = extract_stamp(&pixels, DVec2::splat(32.0), 2);
    assert!(result.is_some());

    let stamp = result.unwrap();
    // For radius=2, stamp is 5x5 centred at (32,32), so it spans image x,y 30..=34 — expressed
    // now as an origin at (30,30) plus the grid's own 0..4.
    assert_eq!(stamp.z.len(), 25);
    assert_eq!(stamp.origin, DVec2::new(30.0, 30.0));
}

#[test]
fn test_extract_stamp_fractional_position() {
    use crate::stacking::star_detection::centroid::extract_stamp;

    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    // Fractional position 32.3, 32.7 rounds to 32, 33
    let result = extract_stamp(&pixels, DVec2::new(32.3, 32.7), 2);
    assert!(result.is_some());

    let stamp = result.unwrap();
    // Centre rounds to (32, 33), so the stamp's top-left pixel is (30, 31).
    assert_eq!(stamp.origin, DVec2::new(30.0, 31.0));
}

#[test]
fn test_local_annulus_background_uniform() {
    use crate::stacking::star_detection::config::measurement_config::LocalBackgroundMethod;

    let width = 128;
    let height = 128;
    let background_value = 0.2f32;

    // Create uniform background with a star
    let mut pixels = vec![background_value; width * height];
    // Add star at center
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - 64.0;
            let dy = y as f32 - 64.0;
            let r2 = dx * dx + dy * dy;
            let value = 0.8 * (-r2 / (2.0 * 2.5 * 2.5)).exp();
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
        measurement: MeasurementConfig {
            local_background: LocalBackgroundMethod::LocalAnnulus,
            ..Default::default()
        },
        ..Default::default()
    };
    let candidates = detect_stars_test(&pixels, &bg, &config.detection);

    assert!(!candidates.is_empty(), "Should detect star");

    let star = measure_star(
        &pixels,
        &bg,
        &candidates[0],
        &config.measurement,
        config.fwhm.expected,
        &StampGrid::new(compute_stamp_radius(config.fwhm.expected)),
    );
    assert!(star.is_some(), "Should compute centroid with LocalAnnulus");

    let star = star.unwrap();
    // SNR should be computed correctly
    assert!(star.snr > 0.0, "SNR should be positive");
    assert!(star.flux > 0.0, "Flux should be positive");
}

#[test]
fn test_local_annulus_vs_global_map() {
    use crate::stacking::star_detection::config::measurement_config::LocalBackgroundMethod;

    let width = 128;
    let height = 128;

    // Create star on uniform background
    let pixels = SyntheticStar::new(Vec2::splat(64.0), 0.8, StarProfile::Gaussian { sigma: 2.5 })
        .stamp(Size2us::new(width, height), 0.1);
    let bg = estimate_background(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    // Detect with GlobalMap
    let config_global = Config {
        measurement: MeasurementConfig {
            local_background: LocalBackgroundMethod::GlobalMap,
            ..Default::default()
        },
        ..Default::default()
    };
    let candidates = detect_stars_test(&pixels, &bg, &config_global.detection);
    let star_global = measure_star(
        &pixels,
        &bg,
        &candidates[0],
        &config_global.measurement,
        config_global.fwhm.expected,
        &StampGrid::new(compute_stamp_radius(config_global.fwhm.expected)),
    )
    .expect("global centroid");

    // Detect with LocalAnnulus
    let config_annulus = Config {
        measurement: MeasurementConfig {
            local_background: LocalBackgroundMethod::LocalAnnulus,
            ..Default::default()
        },
        ..Default::default()
    };
    let star_annulus = measure_star(
        &pixels,
        &bg,
        &candidates[0],
        &config_annulus.measurement,
        config_annulus.fwhm.expected,
        &StampGrid::new(compute_stamp_radius(config_annulus.fwhm.expected)),
    )
    .expect("annulus centroid");

    // Both should give similar position (within 0.5 pixels)
    let pos_diff = ((star_global.pos.x - star_annulus.pos.x).powi(2)
        + (star_global.pos.y - star_annulus.pos.y).powi(2))
    .sqrt();
    assert!(
        pos_diff < 0.5,
        "GlobalMap and LocalAnnulus should give similar positions: diff={}",
        pos_diff
    );

    // Both should have positive flux and SNR
    assert!(star_global.flux > 0.0 && star_annulus.flux > 0.0);
    assert!(star_global.snr > 0.0 && star_annulus.snr > 0.0);
}

#[test]
fn test_local_annulus_near_edge_fallback() {
    use crate::stacking::star_detection::config::measurement_config::LocalBackgroundMethod;

    let width = 64;
    let height = 64;

    // Create star near edge where annulus might be partially outside
    let pos = DVec2::new(20.0, 32.0);
    let pixels = SyntheticStar::new(pos.as_vec2(), 0.8, StarProfile::Gaussian { sigma: 2.0 })
        .stamp(Size2us::new(width, height), 0.1);
    let bg = estimate_background(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );

    let config = Config {
        measurement: MeasurementConfig {
            local_background: LocalBackgroundMethod::LocalAnnulus,
            ..Default::default()
        },
        detection: DetectionConfig {
            edge_margin: 15,
            ..Default::default()
        },
        ..Default::default()
    };
    let candidates = detect_stars_test(&pixels, &bg, &config.detection);

    if !candidates.is_empty() {
        // Should still work (falls back to global if annulus doesn't have enough pixels)
        let star = measure_star(
            &pixels,
            &bg,
            &candidates[0],
            &config.measurement,
            config.fwhm.expected,
            &StampGrid::new(compute_stamp_radius(config.fwhm.expected)),
        );
        if let Some(s) = star {
            assert!(s.flux > 0.0, "Flux should be positive");
        }
    }
}

/// The seed must respect the ceiling it is handed, because the optimizer clamps to that same
/// bound on its first iteration — seeding above it just spends an iteration being pulled back.
#[test]
fn sigma_seed_honours_its_ceiling() {
    // A flat 21x21 patch one unit above the sky, so the second moment is the grid's own:
    // E[dx²] = E[dy²] = 2·(1²+..+10²)/21 = 770/21 = 36.667, giving
    // sigma = sqrt((36.667 + 36.667)/2) = 6.0553.
    let side = 21;
    let background = 0.1f32;
    let mut data_x = Vec::new();
    let mut data_y = Vec::new();
    let mut data_z = Vec::new();
    for y in 0..side {
        for x in 0..side {
            data_x.push(x as f64);
            data_y.push(y as f64);
            data_z.push(background as f64 + 1.0);
        }
    }
    let centre = DVec2::new(10.0, 10.0);

    let wide = estimate_sigma_from_moments(&data_x, &data_y, &data_z, centre, background, 15.0);
    assert!(
        (wide - 6.0553).abs() < 1e-3,
        "grid's own moment is 6.0553, got {wide}"
    );

    // The tightest ceiling the detector ever uses is MIN_STAMP_RADIUS; the same data has to seed
    // inside it rather than at the old fixed 10.0.
    let narrow = estimate_sigma_from_moments(&data_x, &data_y, &data_z, centre, background, 4.0);
    assert_eq!(narrow, 4.0);
    assert_ne!(narrow, wide, "the ceiling has to change the answer");
}
