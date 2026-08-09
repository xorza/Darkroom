use crate::math::size2us::Size2us;
use crate::stacking::star_detection::centroid::tests::*;
use crate::stacking::star_detection::centroid::{StampGrid, compute_stamp_radius};

#[test]
fn test_centroid_accuracy() {
    // Use larger image to minimize background estimation effects
    let width = 128;
    let height = 128;
    let true_pos = DVec2::new(64.3, 64.7);
    let pixels = SyntheticStar::new(
        true_pos.as_vec2(),
        0.8,
        StarProfile::Gaussian { sigma: 2.5 },
    )
    .stamp(Size2us::new(width, height), 0.1);

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
        &StampGrid::new(compute_stamp_radius(config.fwhm.expected)),
    )
    .expect("Should compute centroid");

    let error_x = (star.pos.x - true_pos.x).abs();
    let error_y = (star.pos.y - true_pos.y).abs();

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

/// The same pixels measured near the origin and far out along x must give the same sub-pixel
/// result, because the position carrier is f64.
///
/// The two stamps are byte-identical — one buffer is the other blitted `SHIFT` columns to the
/// right — so the only thing that differs is the magnitude of the coordinates the centroid
/// arithmetic runs on. An f32 carrier quantizes to 4.88e-4 px at x ≈ 6000, which is both coarser
/// than the agreement asserted here and coarser than `CENTROID_CONVERGENCE_THRESHOLD`, so the
/// moments loop's own stopping test would degrade to "the value stopped changing at all".
#[test]
fn subpixel_result_is_independent_of_distance_from_the_origin() {
    const SHIFT: usize = 5968;
    let near = Size2us::new(64, 64);
    let true_pos = DVec2::new(32.3, 32.7);

    let near_pixels = SyntheticStar::new(
        true_pos.as_vec2(),
        0.8,
        StarProfile::Gaussian { sigma: 2.5 },
    )
    .stamp(near, 0.1);

    // Blit, don't re-render: re-rendering at x = 6000.3 would round the centre in the fixture
    // itself and measure that instead of the coordinate arithmetic.
    let far = Size2us::new(SHIFT + 64, 64);
    let mut far_data = vec![0.1f32; far.pixel_count()];
    for y in 0..near.height {
        let dst = far.width * y + SHIFT;
        far_data[dst..dst + near.width].copy_from_slice(near_pixels.row(y));
    }
    let far_pixels = Buffer2::new(far.width, far.height, far_data);

    let radius = compute_stamp_radius(TEST_EXPECTED_FWHM);
    let bg_near = make_uniform_background(near, 0.1, 0.01);
    let bg_far = make_uniform_background(far, 0.1, 0.01);

    let near_pos = refine_centroid(&near_pixels, &bg_near, true_pos, radius, TEST_EXPECTED_FWHM)
        .expect("near refine should succeed");
    let far_pos = refine_centroid(
        &far_pixels,
        &bg_far,
        true_pos + DVec2::new(SHIFT as f64, 0.0),
        radius,
        TEST_EXPECTED_FWHM,
    )
    .expect("far refine should succeed");

    let drift = (far_pos.x - SHIFT as f64 - near_pos.x).abs();
    assert!(
        drift < 1e-9,
        "same pixels drifted {drift} px between x≈32 and x≈{}: near={}, far={}",
        SHIFT + 32,
        near_pos.x,
        far_pos.x - SHIFT as f64
    );
    // Not bit-identical: the per-column Gaussian weights are computed from `px - pos_x`, whose
    // rounding differs between x ≈ 32 and x ≈ 6000, and those weights feed the y accumulator too.
    // A few f64 ulp, six orders of magnitude below the f32 quantization this replaces.
    let y_drift = (far_pos.y - near_pos.y).abs();
    assert!(
        y_drift < 1e-9,
        "y drifted {y_drift} px under an x-only shift"
    );
}

#[test]
fn test_fwhm_estimation() {
    // Use larger image for better background estimation
    let width = 128;
    let height = 128;
    let sigma = 3.0f32;
    let expected_fwhm = FWHM_TO_SIGMA * sigma;
    let pixels = SyntheticStar::new(Vec2::splat(64.0), 0.8, StarProfile::Gaussian { sigma })
        .stamp(Size2us::new(width, height), 0.1);

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
        &StampGrid::new(compute_stamp_radius(config.fwhm.expected)),
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
    let pixels = SyntheticStar::new(Vec2::splat(32.0), 0.8, StarProfile::Gaussian { sigma: 2.5 })
        .stamp(Size2us::new(width, height), 0.1);

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
        &StampGrid::new(compute_stamp_radius(config.fwhm.expected)),
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
    let pixels = SyntheticStar::new(Vec2::splat(32.0), 0.8, StarProfile::Gaussian { sigma: 2.5 })
        .stamp(Size2us::new(width, height), 0.1);

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
        &StampGrid::new(compute_stamp_radius(config.fwhm.expected)),
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
        position: DVec2,
        size: Size2us,
        expected: bool,
    }

    let radius = TEST_STAMP_RADIUS;
    let min_size = 2 * TEST_STAMP_RADIUS + 1;
    let cases = [
        Case {
            name: "center",
            position: DVec2::splat(32.0),
            size: Size2us::new(64, 64),
            expected: true,
        },
        Case {
            name: "minimum valid",
            position: DVec2::splat(radius as f64),
            size: Size2us::new(64, 64),
            expected: true,
        },
        Case {
            name: "maximum valid",
            position: DVec2::splat((64 - radius - 1) as f64),
            size: Size2us::new(64, 64),
            expected: true,
        },
        Case {
            name: "left edge",
            position: DVec2::new((radius - 1) as f64, 32.0),
            size: Size2us::new(64, 64),
            expected: false,
        },
        Case {
            name: "top edge",
            position: DVec2::new(32.0, (radius - 1) as f64),
            size: Size2us::new(64, 64),
            expected: false,
        },
        Case {
            name: "right edge",
            position: DVec2::new((64 - radius) as f64, 32.0),
            size: Size2us::new(64, 64),
            expected: false,
        },
        Case {
            name: "bottom edge",
            position: DVec2::new(32.0, (64 - radius) as f64),
            size: Size2us::new(64, 64),
            expected: false,
        },
        Case {
            name: "negative x",
            position: DVec2::new(-1.0, 32.0),
            size: Size2us::new(64, 64),
            expected: false,
        },
        Case {
            name: "negative y",
            position: DVec2::new(32.0, -1.0),
            size: Size2us::new(64, 64),
            expected: false,
        },
        Case {
            name: "fraction rounds in",
            position: DVec2::new(7.4, 32.0),
            size: Size2us::new(64, 64),
            expected: true,
        },
        Case {
            name: "fraction rounds out",
            position: DVec2::new(6.4, 32.0),
            size: Size2us::new(64, 64),
            expected: false,
        },
        Case {
            name: "minimum image size",
            position: DVec2::splat(radius as f64),
            size: Size2us::new(min_size, min_size),
            expected: true,
        },
        Case {
            name: "image too small",
            position: DVec2::splat(radius as f64),
            size: Size2us::new(min_size - 1, min_size - 1),
            expected: false,
        },
    ];

    for case in cases {
        assert_eq!(
            is_valid_stamp_position(case.position, case.size, radius),
            case.expected,
            "{}: {case:?}",
            case.name
        );
    }
}
