use crate::stacking::star_detection::centroid::tests::*;

#[test]
fn test_centroid_accuracy() {
    // Use larger image to minimize background estimation effects
    let width = 128;
    let height = 128;
    let true_pos = Vec2::new(64.3, 64.7);
    let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8, 0.1);

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

    let error_x = (star.pos.x - true_pos.x as f64).abs();
    let error_y = (star.pos.y - true_pos.y as f64).abs();

    // Sub-pixel accuracy within 0.2 pixels is good for weighted centroid
    assert!(
        error_x < 0.2,
        "X centroid error {} too large (true={}, computed={})",
        error_x,
        true_pos.x,
        star.pos.x
    );
    assert!(
        error_y < 0.2,
        "Y centroid error {} too large (true={}, computed={})",
        error_y,
        true_pos.y,
        star.pos.y
    );
}

#[test]
fn test_fwhm_estimation() {
    // Use larger image for better background estimation
    let width = 128;
    let height = 128;
    let sigma = 3.0f32;
    let expected_fwhm = FWHM_TO_SIGMA * sigma;
    let pixels = make_gaussian_star(width, height, Vec2::splat(64.0), sigma, 0.8, 0.1);

    let bg = estimate_background(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    // Use higher max_area because dilation (radius=2) expands the star region
    let config = Config {
        detection: DetectionConfig {
            max_area: 1000,
            ..Default::default()
        },
        ..Default::default()
    };
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

    // FWHM estimation from weighted second moments has systematic bias due to
    // finite aperture and background noise - 40% tolerance is reasonable
    let fwhm_error = (star.fwhm - expected_fwhm).abs() / expected_fwhm;
    assert!(
        fwhm_error < 0.4,
        "FWHM error {} too large (expected={}, computed={})",
        fwhm_error,
        expected_fwhm,
        star.fwhm
    );
}

#[test]
fn test_circular_star_eccentricity() {
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

    let star = measure_star(
        &pixels,
        &bg,
        &candidates[0],
        &config.measurement,
        config.fwhm.expected,
    )
    .expect("Should compute centroid");

    assert!(
        star.eccentricity < 0.3,
        "Circular star has high eccentricity: {}",
        star.eccentricity
    );
}

#[test]
fn test_snr_and_flux_values() {
    // A bright star (amplitude 0.8, sigma 2.5) on background 0.0 should have
    // substantial SNR (>> 10) and measurable flux
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

    let star = measure_star(
        &pixels,
        &bg,
        &candidates[0],
        &config.measurement,
        config.fwhm.expected,
    )
    .expect("Should compute centroid");

    // Bright star with amplitude 0.8 on zero background should have high SNR
    assert!(
        star.snr > 50.0,
        "Bright star SNR {} should be > 50",
        star.snr
    );
    // Flux should be substantial for amplitude=0.8 Gaussian
    assert!(
        star.flux > 1.0,
        "Bright star flux {} should be > 1.0",
        star.flux
    );
    // Peak should be close to star amplitude
    assert!(
        star.peak > 0.5,
        "Peak {} should be close to amplitude 0.8",
        star.peak
    );
}

#[test]
fn valid_stamp_position_covers_boundaries_and_rounding() {
    #[derive(Debug)]
    struct Case {
        name: &'static str,
        position: Vec2,
        width: usize,
        height: usize,
        expected: bool,
    }

    let radius = TEST_STAMP_RADIUS;
    let min_size = 2 * TEST_STAMP_RADIUS + 1;
    let cases = [
        Case {
            name: "center",
            position: Vec2::splat(32.0),
            width: 64,
            height: 64,
            expected: true,
        },
        Case {
            name: "minimum valid",
            position: Vec2::splat(radius as f32),
            width: 64,
            height: 64,
            expected: true,
        },
        Case {
            name: "maximum valid",
            position: Vec2::splat((64 - radius - 1) as f32),
            width: 64,
            height: 64,
            expected: true,
        },
        Case {
            name: "left edge",
            position: Vec2::new((radius - 1) as f32, 32.0),
            width: 64,
            height: 64,
            expected: false,
        },
        Case {
            name: "top edge",
            position: Vec2::new(32.0, (radius - 1) as f32),
            width: 64,
            height: 64,
            expected: false,
        },
        Case {
            name: "right edge",
            position: Vec2::new((64 - radius) as f32, 32.0),
            width: 64,
            height: 64,
            expected: false,
        },
        Case {
            name: "bottom edge",
            position: Vec2::new(32.0, (64 - radius) as f32),
            width: 64,
            height: 64,
            expected: false,
        },
        Case {
            name: "negative x",
            position: Vec2::new(-1.0, 32.0),
            width: 64,
            height: 64,
            expected: false,
        },
        Case {
            name: "negative y",
            position: Vec2::new(32.0, -1.0),
            width: 64,
            height: 64,
            expected: false,
        },
        Case {
            name: "fraction rounds in",
            position: Vec2::new(7.4, 32.0),
            width: 64,
            height: 64,
            expected: true,
        },
        Case {
            name: "fraction rounds out",
            position: Vec2::new(6.4, 32.0),
            width: 64,
            height: 64,
            expected: false,
        },
        Case {
            name: "minimum image size",
            position: Vec2::splat(radius as f32),
            width: min_size,
            height: min_size,
            expected: true,
        },
        Case {
            name: "image too small",
            position: Vec2::splat(radius as f32),
            width: min_size - 1,
            height: min_size - 1,
            expected: false,
        },
    ];

    for case in cases {
        assert_eq!(
            is_valid_stamp_position(case.position, case.width, case.height, radius),
            case.expected,
            "{}: {case:?}",
            case.name
        );
    }
}
