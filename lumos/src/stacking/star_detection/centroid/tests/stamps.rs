use crate::math::size2us::Size2us;
use crate::stacking::star_detection::centroid::tests::*;

#[test]
fn test_estimate_sigma_from_moments_gaussian() {
    use crate::stacking::star_detection::centroid::estimate_sigma_from_moments;

    let width = 21;
    let height = 21;
    let cx = 10.0f32;
    let cy = 10.0f32;
    let true_sigma = 2.5f32;
    let background = 0.1f32;

    // Create Gaussian star
    let mut data_x = Vec::new();
    let mut data_y = Vec::new();
    let mut data_z = Vec::new();

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let value =
                background + 1.0 * (-0.5 * (dx * dx + dy * dy) / (true_sigma * true_sigma)).exp();
            data_x.push(x as f32);
            data_y.push(y as f32);
            data_z.push(value);
        }
    }

    let estimated_sigma =
        estimate_sigma_from_moments(&data_x, &data_y, &data_z, Vec2::new(cx, cy), background);

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
    let cx = 10.0f32;
    let cy = 10.0f32;
    let background = 0.1f32;

    for true_sigma in [1.5f32, 2.0, 2.5, 3.0, 4.0] {
        let mut data_x = Vec::new();
        let mut data_y = Vec::new();
        let mut data_z = Vec::new();

        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let value = background
                    + 1.0 * (-0.5 * (dx * dx + dy * dy) / (true_sigma * true_sigma)).exp();
                data_x.push(x as f32);
                data_y.push(y as f32);
                data_z.push(value);
            }
        }

        let estimated_sigma =
            estimate_sigma_from_moments(&data_x, &data_y, &data_z, Vec2::new(cx, cy), background);

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
    let true_pos = Vec2::new(32.3, 32.7);
    let sigma = 1.5f32; // Small sigma
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let pixels = SyntheticStar::new(true_pos, 0.8, StarProfile::Gaussian { sigma })
        .stamp(Size2us::new(width, height), 0.1);
    let bg = make_uniform_background(Size2us::new(width, height), 0.1, 0.01);

    // Use small expected FWHM
    let result = refine_centroid(
        &pixels,
        Size2us::new(width, height),
        &bg,
        Vec2::splat(32.0),
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
    let true_pos = Vec2::new(32.3, 32.7);
    let sigma = 4.0f32; // Large sigma
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let pixels = SyntheticStar::new(true_pos, 0.8, StarProfile::Gaussian { sigma })
        .stamp(Size2us::new(width, height), 0.1);
    let bg = make_uniform_background(Size2us::new(width, height), 0.1, 0.01);

    // Use large expected FWHM
    let result = refine_centroid(
        &pixels,
        Size2us::new(width, height),
        &bg,
        Vec2::splat(32.0),
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

    let result = extract_stamp(&pixels, Vec2::splat(32.0), 5);
    assert!(result.is_some(), "Should extract stamp at center");

    let stamp = result.unwrap();
    let expected_size = (2 * 5 + 1) * (2 * 5 + 1); // 11x11 = 121
    assert_eq!(stamp.x.len(), expected_size);
    assert_eq!(stamp.y.len(), expected_size);
    assert_eq!(stamp.z.len(), expected_size);
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
    assert!(extract_stamp(&pixels, Vec2::new(3.0, 32.0), 5).is_none());
    assert!(extract_stamp(&pixels, Vec2::new(32.0, 3.0), 5).is_none());
    assert!(extract_stamp(&pixels, Vec2::new(61.0, 32.0), 5).is_none());
    assert!(extract_stamp(&pixels, Vec2::new(32.0, 61.0), 5).is_none());
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

    let result = extract_stamp(&pixels, Vec2::splat(32.0), 5);
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

    let result = extract_stamp(&pixels, Vec2::splat(32.0), 2);
    assert!(result.is_some());

    let stamp = result.unwrap();
    // For radius=2, stamp is 5x5, centered at (32,32)
    // x coords should be 30,31,32,33,34 (repeated for each row)
    // y coords should be 30,30,30,30,30, 31,31,31,31,31, etc.
    assert_eq!(stamp.x.len(), 25);

    // Check that coordinates are correct
    let min_x = stamp.x.iter().fold(f32::MAX, |a, &b| a.min(b));
    let max_x = stamp.x.iter().fold(f32::MIN, |a, &b| a.max(b));
    let min_y = stamp.y.iter().fold(f32::MAX, |a, &b| a.min(b));
    let max_y = stamp.y.iter().fold(f32::MIN, |a, &b| a.max(b));

    assert_eq!(min_x, 30.0);
    assert_eq!(max_x, 34.0);
    assert_eq!(min_y, 30.0);
    assert_eq!(max_y, 34.0);
}

#[test]
fn test_extract_stamp_fractional_position() {
    use crate::stacking::star_detection::centroid::extract_stamp;

    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    // Fractional position 32.3, 32.7 rounds to 32, 33
    let result = extract_stamp(&pixels, Vec2::new(32.3, 32.7), 2);
    assert!(result.is_some());

    let stamp = result.unwrap();
    // Center should be at rounded position (32, 33)
    let min_x = stamp.x.iter().fold(f32::MAX, |a, &b| a.min(b));
    let max_x = stamp.x.iter().fold(f32::MIN, |a, &b| a.max(b));
    let min_y = stamp.y.iter().fold(f32::MAX, |a, &b| a.min(b));
    let max_y = stamp.y.iter().fold(f32::MIN, |a, &b| a.max(b));

    assert_eq!(min_x, 30.0); // 32 - 2 = 30
    assert_eq!(max_x, 34.0); // 32 + 2 = 34
    assert_eq!(min_y, 31.0); // 33 - 2 = 31
    assert_eq!(max_y, 35.0); // 33 + 2 = 35
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
    let pos = Vec2::new(20.0, 32.0);
    let pixels = SyntheticStar::new(pos, 0.8, StarProfile::Gaussian { sigma: 2.0 })
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
        );
        if let Some(s) = star {
            assert!(s.flux > 0.0, "Flux should be positive");
        }
    }
}
