use super::*;
use crate::stacking::star_detection::centroid::compute_stamp_radius;
use crate::stacking::star_detection::centroid::stamp::StampGrid;
use crate::stacking::star_detection::centroid::stamp::{StampFit, sigma_from_moments};

/// σ seeds now come out of `StampFit::prepare`'s single pass, so they are pinned through it.
/// The ceiling is the stamp radius, which is 10 for these 21×21 fields.
#[test]
fn sigma_seed_recovers_a_known_gaussian() {
    let side = 21;
    let background = 0.1f32;
    let true_sigma = 2.5f32;

    let pixels = SyntheticStar::new(
        Vec2::splat(10.0),
        1.0,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(side, side), background);

    let fit = StampFit::prepare::<6>(
        &pixels,
        DVec2::splat(10.0),
        &StampGrid::new(10),
        background,
        None,
    )
    .expect("21x21 stamp at its centre");

    // Second moments of a truncated Gaussian under-report σ; 20% is the tolerance the seed needs.
    let error = (fit.sigma_est - true_sigma).abs() / true_sigma;
    assert!(
        error < 0.2,
        "sigma seed error {:.1}% too large (expected={true_sigma}, got={})",
        error * 100.0,
        fit.sigma_est
    );
}

#[test]
fn sigma_seed_tracks_the_true_width() {
    let side = 21;
    let background = 0.1f32;
    let mut seeds = Vec::new();

    for true_sigma in [1.5f32, 2.0, 2.5, 3.0, 4.0] {
        let pixels = SyntheticStar::new(
            Vec2::splat(10.0),
            1.0,
            StarProfile::Gaussian { sigma: true_sigma },
        )
        .stamp(Size2us::new(side, side), background);

        let fit = StampFit::prepare::<6>(
            &pixels,
            DVec2::splat(10.0),
            &StampGrid::new(10),
            background,
            None,
        )
        .expect("21x21 stamp at its centre");

        let error = (fit.sigma_est - true_sigma).abs() / true_sigma;
        assert!(
            error < 0.25,
            "sigma={true_sigma}: seed error {:.1}% too large (got={})",
            error * 100.0,
            fit.sigma_est
        );
        seeds.push(fit.sigma_est);
    }

    // A wider star must seed wider — the estimate has to respond to the input, not just land
    // inside a tolerance band.
    assert!(
        seeds.windows(2).all(|w| w[1] > w[0]),
        "seeds must increase with true sigma, got {seeds:?}"
    );
}

#[test]
fn refine_centroid_adaptive_sigma_small_fwhm() {
    let width = 64;
    let height = 64;
    let true_pos = DVec2::new(32.3, 32.7);
    let sigma = 1.5f32; // Small sigma
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let pixels = SyntheticStar::new(true_pos.as_vec2(), 0.8, StarProfile::Gaussian { sigma })
        .stamp(Size2us::new(width, height), 0.1);
    let bg = background_map::uniform(Size2us::new(width, height), 0.1, 0.01);

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
fn refine_centroid_adaptive_sigma_large_fwhm() {
    let width = 64;
    let height = 64;
    let true_pos = DVec2::new(32.3, 32.7);
    let sigma = 4.0f32; // Large sigma
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let pixels = SyntheticStar::new(true_pos.as_vec2(), 0.8, StarProfile::Gaussian { sigma })
        .stamp(Size2us::new(width, height), 0.1);
    let bg = background_map::uniform(Size2us::new(width, height), 0.1, 0.01);

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

/// Extraction is no longer its own function — `StampFit::prepare` walks the stamp once and fills
/// everything — so its behaviour is pinned through the constructor that does it.
fn extract(pixels: &Buffer2<f32>, pos: DVec2, radius: usize) -> Option<StampFit> {
    StampFit::prepare::<6>(pixels, pos, &StampGrid::new(radius), 0.0, None)
}

#[test]
fn extract_stamp_valid_center() {
    let pixels = Buffer2::new_filled(64, 64, 0.5f32);

    let fit = extract(&pixels, DVec2::splat(32.0), 5).expect("stamp at centre");
    assert_eq!(fit.stamp.z.len(), 11 * 11);
    // Coordinates live in the shared `StampGrid`; what the stamp itself pins is where its
    // top-left pixel sits, which is what the grid's `0..2r` is relative to.
    assert_eq!(fit.stamp.origin, DVec2::new(27.0, 27.0));
    assert_eq!(fit.stamp.peak, 0.5);
    // A flat stamp weights every pixel equally, so `local_pos` lands on the stamp's own centre.
    assert_eq!(fit.local_pos, DVec2::splat(5.0));
    assert!(
        fit.weights.is_none(),
        "unweighted unless a noise model is set"
    );
}

#[test]
fn extract_stamp_edge_invalid() {
    let pixels = Buffer2::new_filled(64, 64, 0.5f32);

    // Too close to edges
    assert!(extract(&pixels, DVec2::new(3.0, 32.0), 5).is_none());
    assert!(extract(&pixels, DVec2::new(32.0, 3.0), 5).is_none());
    assert!(extract(&pixels, DVec2::new(61.0, 32.0), 5).is_none());
    assert!(extract(&pixels, DVec2::new(32.0, 61.0), 5).is_none());
}

#[test]
fn extract_stamp_peak_value() {
    let mut pixels = Buffer2::new_filled(64, 64, 0.1f32);
    // Add bright pixel at center
    pixels[(32, 32)] = 0.9;

    let fit = extract(&pixels, DVec2::splat(32.0), 5).expect("stamp at centre");
    assert_eq!(fit.stamp.peak, 0.9);
}

#[test]
fn extract_stamp_coordinates() {
    let pixels = Buffer2::new_filled(64, 64, 0.5f32);

    // For radius=2, stamp is 5x5 centred at (32,32), so it spans image x,y 30..=34 — expressed
    // now as an origin at (30,30) plus the grid's own 0..4.
    let fit = extract(&pixels, DVec2::splat(32.0), 2).expect("stamp at centre");
    assert_eq!(fit.stamp.z.len(), 25);
    assert_eq!(fit.stamp.origin, DVec2::new(30.0, 30.0));
}

#[test]
fn extract_stamp_fractional_position() {
    let pixels = Buffer2::new_filled(64, 64, 0.5f32);

    // Fractional position 32.3, 32.7 rounds to 32, 33, so the top-left pixel is (30, 31) and the
    // centre sits at (2.3, 1.7) within the stamp.
    let fit = extract(&pixels, DVec2::new(32.3, 32.7), 2).expect("stamp at centre");
    assert_eq!(fit.stamp.origin, DVec2::new(30.0, 31.0));
    assert!((fit.local_pos - DVec2::new(2.3, 1.7)).length() < 1e-12);
}

#[test]
fn stamp_too_small_for_the_parameter_count_is_rejected() {
    let pixels = Buffer2::new_filled(64, 64, 0.5f32);
    let grid = StampGrid::new(1);
    // A radius-1 stamp is 9 pixels: enough to constrain Moffat's 5, not Gaussian's 6 with the
    // strict inequality a least-squares fit needs... but 9 > 6, so both pass. Radius 0 is 1 pixel.
    assert!(StampFit::prepare::<6>(&pixels, DVec2::splat(32.0), &grid, 0.0, None).is_some());
    let point = StampGrid::new(0);
    assert!(StampFit::prepare::<6>(&pixels, DVec2::splat(32.0), &point, 0.0, None).is_none());
}

#[test]
fn local_annulus_background_uniform() {
    use crate::stacking::star_detection::config::measurement_config::LocalBackgroundMethod;

    let width = 128;
    let height = 128;
    let background_value = 0.2f32;

    // Create uniform background with a star
    let mut pixels = Buffer2::new_filled(width, height, background_value);
    // Add star at center
    SyntheticStar::new(Vec2::splat(64.0), 0.8, StarProfile::Gaussian { sigma: 2.5 })
        .add_to(&mut pixels);

    let bg = background_map::estimate(
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
fn local_annulus_vs_global_map() {
    use crate::stacking::star_detection::config::measurement_config::LocalBackgroundMethod;

    let width = 128;
    let height = 128;

    // Create star on uniform background
    let pixels = SyntheticStar::new(Vec2::splat(64.0), 0.8, StarProfile::Gaussian { sigma: 2.5 })
        .stamp(Size2us::new(width, height), 0.1);
    let bg = background_map::estimate(
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
fn local_annulus_near_edge_fallback() {
    use crate::stacking::star_detection::config::measurement_config::LocalBackgroundMethod;

    let width = 64;
    let height = 64;

    // Create star near edge where annulus might be partially outside
    let pos = DVec2::new(20.0, 32.0);
    let pixels = SyntheticStar::new(pos.as_vec2(), 0.8, StarProfile::Gaussian { sigma: 2.0 })
        .stamp(Size2us::new(width, height), 0.1);
    let bg = background_map::estimate(
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
    // The sums a flat 21x21 patch one unit above the sky produces about its centre: every pixel
    // weighs 1, so sum_w = 441, and E[dx²] = E[dy²] = 2·(1²+..+10²)/21 = 770/21, so
    // sum_r2 = 441 · 2 · 770/21 = 32340. That gives sigma = sqrt(32340/441/2) = sqrt(36.667)
    // = 6.0553.
    let sum_w = 441.0;
    let sum_r2 = 32340.0;

    let wide = sigma_from_moments(sum_r2, sum_w, 15.0);
    assert!(
        (wide - 6.0553).abs() < 1e-3,
        "grid's own moment is 6.0553, got {wide}"
    );

    // The tightest ceiling the detector ever uses is MIN_STAMP_RADIUS; the same data has to seed
    // inside it rather than at the old fixed 10.0.
    let narrow = sigma_from_moments(sum_r2, sum_w, 4.0);
    assert_eq!(narrow, 4.0);
    assert_ne!(narrow, wide, "the ceiling has to change the answer");

    // No signal above the sky leaves the moment undefined, so the seed falls back rather than
    // dividing by zero.
    assert_eq!(sigma_from_moments(0.0, 0.0, 15.0), 2.0);
}
