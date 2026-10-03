use super::*;

/// A lone star found by detection on an estimated sky and measured with the default config, as the
/// pipeline runs it before the FWHM is known: one candidate at the nearest pixel, a moments centroid
/// as far along as ten steps take it, the flux of the stamp's samples, and every metric as on
/// the true sky.
///
/// The default method is ten weighted-moments steps with a window of `σ_w = 0.8 · σ(seed FWHM 4.0)`
/// = 1.359 px, each step shrinking the error by `c = σ² / (σ² + σ_w²)` — see
/// `moments_contract_and_fits_ignore_the_seed`. The flux sums the residual over the
/// (2r+1)² stamp around the rounded centroid, r = `compute_stamp_radius(4.0)` = 7, and the
/// samples are point values of a separable profile: A · Σᵢ g(i − x₀) · Σⱼ g(j − y₀).
#[test]
fn a_detected_star_measures_as_on_its_true_sky() {
    const AMPLITUDE: f64 = 0.8;
    const SKY: f32 = 0.1;
    const SEED_FWHM: f32 = 4.0;
    // A noiseless frame thresholds at its noise floor, 1e-4 of the sky: the σ = 3 star stays above
    // 4 · 1e-5 out to r = 3 · √(2 ln(0.8 / 4e-5)) = 13.3 px, a footprint of π · 13.3² ≈ 560 px —
    // past the default 500.
    let config = Config {
        detection: DetectionConfig {
            max_area: 1000,
            ..Default::default()
        },
        ..Default::default()
    };
    let radius = compute_stamp_radius(SEED_FWHM);
    let window_sigma = f64::from(0.8 * fwhm_to_sigma(SEED_FWHM));
    for (x, y, sigma) in [(64.3, 64.7, 2.5f32), (64.0, 64.0, 3.0), (64.6, 63.8, 3.0)] {
        let truth = DVec2::new(x, y);
        let pixels = SyntheticStar::new(
            truth.as_vec2(),
            AMPLITUDE as f32,
            StarProfile::Gaussian { sigma },
        )
        .stamp(Size2us::new(128, 128), SKY);
        let truth = truth.as_vec2().as_dvec2();
        let background = background_map::estimate(
            &pixels,
            &BackgroundConfig {
                tile_size: 32,
                ..Default::default()
            },
        );
        let estimated = Measured::of(&pixels, &background);
        let exact = Measured::flat(&pixels, SKY, 0.01);

        let candidates = detect_stars_test(&estimated.residual, &estimated.sky, &config.detection);
        assert_eq!(candidates.len(), 1, "σ {sigma} at {truth}");
        let region = &candidates[0];
        let nearest = truth.round();
        assert_eq!(
            region.peak,
            Vec2us::new(nearest.x as usize, nearest.y as usize),
            "σ {sigma} at {truth}"
        );

        let star = estimated
            .measure(region, &config.measurement, SEED_FWHM)
            .expect("the star measures");
        let reference = exact
            .measure(region, &config.measurement, SEED_FWHM)
            .expect("the star measures on the true sky");

        let npix = (2 * radius + 1).pow(2);
        let sigma_sq = f64::from(sigma).powi(2);
        let contraction = sigma_sq / (sigma_sq + window_sigma.powi(2));
        let error = (star.pos - truth).length();
        // Plus the f64 rounding of the weighted sums, ε per term over the stamp at coordinates ≈ 64.
        let bound = 1.01 * (nearest - truth).length() * contraction.powi(10)
            + npix as f64 * f64::EPSILON * truth.max_element();
        assert!(error <= bound, "σ {sigma} at {truth}: {error} > {bound}");

        // The estimated sky is 0.1 to within one f32 ulp (7.5e-9) per pixel, against stars whose
        // stamp sums to ≥ 31: a relative change of ≤ 225 · 7.5e-9 / 31 = 5.4e-8 in every sum the
        // metrics are built from. 1e-6 holds it with room for the f32 rounding of each metric.
        let same = |name, a: f64, b: f64| {
            assert!(
                (a - b).abs() <= 1e-6 * b.abs().max(1.0),
                "σ {sigma} at {truth}: {name} {a} vs {b} on the true sky"
            );
        };
        same("x", star.pos.x, reference.pos.x);
        same("y", star.pos.y, reference.pos.y);
        for (name, a, b) in [
            ("flux", star.flux, reference.flux),
            ("fwhm", star.fwhm, reference.fwhm),
            ("eccentricity", star.eccentricity, reference.eccentricity),
            ("peak", star.peak, reference.peak),
            ("sharpness", star.sharpness, reference.sharpness),
            ("sround", star.roundness.sround, reference.roundness.sround),
            ("ground", star.roundness.ground, reference.roundness.ground),
        ] {
            same(name, f64::from(a), f64::from(b));
        }

        // Each sample takes four f32 roundings of at most ε · (A + sky) — the profile's exp and
        // scale, the add onto the sky and the subtraction from it — so the 225 of them move the
        // sum by at most 225 · 4ε · 0.9.
        let centre = star.pos.round();
        let samples = |c: f64, x0: f64| -> f64 {
            (-(radius as i32)..=radius as i32)
                .map(|d| (-(c + f64::from(d) - x0).powi(2) / (2.0 * sigma_sq)).exp())
                .sum()
        };
        let expected = AMPLITUDE * samples(centre.x, truth.x) * samples(centre.y, truth.y);
        let rounding = npix as f64 * 4.0 * f64::from(f32::EPSILON) * (AMPLITUDE + f64::from(SKY));
        assert!(
            (f64::from(reference.flux) - expected).abs() <= rounding,
            "σ {sigma} at {truth}: flux {} vs {expected}",
            reference.flux
        );
        // With no annulus the SNR divides by the map's noise over the stamp, 0.01 on the true
        // sky, times √npix.
        let snr = f64::from(reference.flux) / (0.01 * (npix as f64).sqrt());
        assert!(
            (f64::from(reference.snr) - snr).abs() <= 1e-6 * snr,
            "σ {sigma} at {truth}: SNR {} vs {snr}",
            reference.snr
        );
    }
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
    let bg_near = background_map::uniform(near, 0.1, 0.01);
    let bg_far = background_map::uniform(far, 0.1, 0.01);

    let near_pos = refine_centroid(
        &bg_near.residual_of(&near_pixels),
        true_pos,
        radius,
        TEST_EXPECTED_FWHM,
    )
    .expect("near refine should succeed");
    let far_pos = refine_centroid(
        &bg_far.residual_of(&far_pixels),
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
            position: DVec2::new(radius as f64 + 0.4, 32.0),
            size: Size2us::new(64, 64),
            expected: true,
        },
        Case {
            name: "fraction rounds out",
            position: DVec2::new(radius as f64 - 0.6, 32.0),
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
