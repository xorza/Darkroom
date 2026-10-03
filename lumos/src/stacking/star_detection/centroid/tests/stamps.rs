use super::*;
use crate::stacking::star_detection::centroid::stamp::{StampFit, sigma_from_moments};

/// The σ seed is the stamp's second moment about the centre, `σ² = Σw·r² / Σw / 2`: on a centred
/// Gaussian truncated to the stamp's ±10 px it is S₂ / S with S = Σᵢ g(i), S₂ = Σᵢ i²·g(i) — the
/// truncation under-reports σ, the more the wider the star — and a wider star seeds wider.
///
/// Each f32 sample is off by at most ε of its value ≤ 1.1 (the profile, then the add onto the
/// sky); over 441 samples with |r²/2 − σ²| ≤ 100 that moves σ² by ≤ 441 · ε · 100 / Σw, and σ by
/// half that over σ.
#[test]
fn sigma_seed_is_the_truncated_second_moment() {
    const SKY: f32 = 0.1;
    let radius = 10;
    for sigma in [1.5f32, 2.0, 2.5, 3.0, 4.0] {
        let pixels = SyntheticStar::new(Vec2::splat(10.0), 1.0, StarProfile::Gaussian { sigma })
            .stamp(Size2us::new(21, 21), SKY);
        let fit = StampFit::prepare::<6>(
            &pixels,
            DVec2::splat(10.0),
            &StampGrid::new(radius),
            SKY,
            None,
        )
        .expect("21x21 stamp at its centre");

        let two_sigma_sq = 2.0 * f64::from(sigma).powi(2);
        let g = |i: i32| (-f64::from(i * i) / two_sigma_sq).exp();
        let arms = -(radius as i32)..=radius as i32;
        let sum: f64 = arms.clone().map(g).sum();
        let second: f64 = arms.map(|i| f64::from(i * i) * g(i)).sum();
        let expected = (second / sum).sqrt();
        let rounding = 441.0 * f64::from(f32::EPSILON) * 100.0 / sum.powi(2) / (2.0 * expected);
        let seed = f64::from(fit.sigma_est);
        assert!(
            (seed - expected).abs() <= rounding,
            "σ {sigma}: seed {seed}, expected {expected} ± {rounding}"
        );
        assert!(
            expected < f64::from(sigma),
            "σ {sigma}: truncation under-reports"
        );
    }
}

/// One moments step contracts the error by `c = σ² / (σ² + σ_w²)`, with the window
/// `σ_w = 0.8 · σ(expected FWHM)` held to [1, r / 2]: the window follows the FWHM it is told, at each clamp too.
/// The truth is 0.76 px from the start, (32, 32) to (32.3, 32.7).
///
/// The weighted star is a Gaussian of `σ_p² = σ²σ_w² / (σ² + σ_w²)`, and every stamp here keeps
/// ≥ 4.3 `σ_p` of it on each side: what the stamp cuts moves the ratio by ≤ 1.3e-4 (measured, the r / 2
/// case), under the 2e-4 asserted.
#[test]
fn refine_centroid_window_follows_the_expected_fwhm() {
    struct Case {
        name: &'static str,
        sigma: f32,
        expected_fwhm: f32,
        radius: usize,
        window_sigma: f64,
    }
    let cases = [
        Case {
            name: "matched narrow",
            sigma: 1.5,
            expected_fwhm: sigma_to_fwhm(1.5),
            radius: TEST_STAMP_RADIUS,
            window_sigma: 1.2,
        },
        Case {
            name: "matched wide",
            sigma: 4.0,
            expected_fwhm: sigma_to_fwhm(4.0),
            radius: TEST_STAMP_RADIUS,
            window_sigma: 3.2,
        },
        Case {
            name: "wide star, narrow window",
            sigma: 4.0,
            expected_fwhm: sigma_to_fwhm(1.5),
            radius: TEST_STAMP_RADIUS,
            window_sigma: 1.2,
        },
        Case {
            name: "held to 1 px",
            sigma: 1.5,
            expected_fwhm: 1.0,
            radius: TEST_STAMP_RADIUS,
            window_sigma: 1.0,
        },
        Case {
            name: "held to r / 2",
            sigma: 1.5,
            expected_fwhm: sigma_to_fwhm(4.0),
            radius: 6,
            window_sigma: 3.0,
        },
    ];
    let truth = Vec2::new(32.3, 32.7).as_dvec2();
    let start = DVec2::splat(32.0);
    for case in cases {
        let pixels = SyntheticStar::new(
            truth.as_vec2(),
            0.8,
            StarProfile::Gaussian { sigma: case.sigma },
        )
        .stamp(Size2us::new(64, 64), 0.1);
        let measured = Measured::flat(&pixels, 0.1, 0.01);
        let step = refine_centroid(&measured.residual, start, case.radius, case.expected_fwhm)
            .expect("one step lands");
        let sigma_sq = f64::from(case.sigma).powi(2);
        let contraction = sigma_sq / (sigma_sq + case.window_sigma.powi(2));
        let ratio = (step - truth).length() / (start - truth).length();
        assert!(
            (ratio - contraction).abs() <= 2e-4 * contraction,
            "{}: step ratio {ratio}, expected {contraction}",
            case.name
        );
    }
}

/// `StampFit::prepare` walks the stamp once and fills every field, so its extraction is pinned
/// through the constructor.
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

    assert!(extract(&pixels, DVec2::new(3.0, 32.0), 5).is_none());
    assert!(extract(&pixels, DVec2::new(32.0, 3.0), 5).is_none());
    assert!(extract(&pixels, DVec2::new(61.0, 32.0), 5).is_none());
    assert!(extract(&pixels, DVec2::new(32.0, 61.0), 5).is_none());
}

#[test]
fn extract_stamp_peak_value() {
    let mut pixels = Buffer2::new_filled(64, 64, 0.1f32);
    pixels[(32, 32)] = 0.9;

    let fit = extract(&pixels, DVec2::splat(32.0), 5).expect("stamp at centre");
    assert_eq!(fit.stamp.peak, 0.9);
}

#[test]
fn extract_stamp_coordinates() {
    let pixels = Buffer2::new_filled(64, 64, 0.5f32);

    // A radius-2 stamp about (32, 32) spans x, y 30..=34: its origin at (30, 30) plus the grid's
    // own 0..=4.
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
    // A least-squares fit needs strictly more samples than parameters: a radius-1 stamp holds 9 > 6,
    // a radius-0 stamp 1.
    assert!(StampFit::prepare::<6>(&pixels, DVec2::splat(32.0), &grid, 0.0, None).is_some());
    let point = StampGrid::new(0);
    assert!(StampFit::prepare::<6>(&pixels, DVec2::splat(32.0), &point, 0.0, None).is_none());
}

/// A sky the map left in the residual — a flat pedestal Δ — stays in the flux under `GlobalMap` and
/// comes out under `LocalAnnulus`, while the moments position, which neither mode touches, is the
/// same bit for bit.
///
/// The star is σ = 1.5 on a matched stamp, r = 7, and its annulus runs from 7 to 11 px about the
/// rounded centre, which the star sits 0.5 px from: there it adds at most A · e^(−6.5² / 2σ²) =
/// 6.7e-5 to Δ, so the annulus median is Δ to within that and the annulus flux the true flux to
/// within npix times it. The pedestal adds exactly npix · Δ to the
/// map's flux, to the f32 rounding of each sample (≤ ε of A + Δ) and of the sum.
#[test]
fn local_annulus_removes_a_sky_the_map_left() {
    const AMPLITUDE: f32 = 0.8;
    const PEDESTAL: f32 = 0.02;
    let sigma = 1.5f32;
    let fwhm = sigma_to_fwhm(sigma);
    let radius = compute_stamp_radius(fwhm);
    assert_eq!(radius, 7);
    let pixels = SyntheticStar::new(
        Vec2::new(64.3, 63.6),
        AMPLITUDE,
        StarProfile::Gaussian { sigma },
    )
    .stamp(Size2us::new(128, 128), 0.1);
    let truth = Measured::flat(&pixels, 0.1, 0.01);
    let offset = Measured::flat(&pixels, 0.1 - PEDESTAL, 0.01);
    let region = truth.region_at(DVec2::new(64.3, 63.6));
    let measure = |measured: &Measured, local_background| {
        let config = MeasurementConfig {
            local_background,
            ..Default::default()
        };
        measured
            .measure(&region, &config, fwhm)
            .expect("the star measures")
    };
    let exact = measure(&truth, LocalBackgroundMethod::GlobalMap);
    let global = measure(&offset, LocalBackgroundMethod::GlobalMap);
    let annulus = measure(&offset, LocalBackgroundMethod::LocalAnnulus);

    assert_eq!(global.pos, annulus.pos);
    let npix = ((2 * radius + 1) * (2 * radius + 1)) as f64;
    let flux = f64::from(exact.flux);
    let rounding = npix * f64::from(f32::EPSILON) * f64::from(AMPLITUDE + PEDESTAL) * 2.0;
    let added = f64::from(global.flux) - flux;
    assert!(
        (added - npix * f64::from(PEDESTAL)).abs() <= rounding,
        "the map keeps {added}, expected {}",
        npix * f64::from(PEDESTAL)
    );
    let tail = f64::from(AMPLITUDE) * (-6.5f64.powi(2) / (2.0 * f64::from(sigma).powi(2))).exp();
    let removed = f64::from(annulus.flux) - flux;
    assert!(
        removed.abs() <= npix * tail + rounding,
        "the annulus leaves {removed} of the pedestal, bound {}",
        npix * tail + rounding
    );
}

/// The annulus is `None` below 10 in-frame pixels. A ring of r² ∈ [1, 4] holds 12: (±1, 0),
/// (0, ±1), (±1, ±1), (±2, 0), (0, ±2). About (1, 1), (−2, 0) and (0, −2) fall off the frame, and
/// (2, 0) past a frame 3 wide: 9 are left. A frame 4 wide keeps (2, 0): 10, and the flat
/// residual's median and σ come back exactly.
#[test]
fn local_annulus_needs_ten_pixels_in_the_frame() {
    let annulus = |width| {
        let residual = Buffer2::new_filled(width, 5, 0.25f32);
        compute_annulus_background(&residual, DVec2::splat(1.0), 1, 2)
    };
    assert!(annulus(3).is_none());
    let sky = annulus(4).expect("ten pixels are enough");
    assert_eq!(sky.offset, 0.25);
    assert_eq!(sky.noise, 0.0);
}

/// A stamp inside the frame keeps the whole square ring at distance r in the annulus — every
/// (±r, dy) and (dx, ±r) with r² ≤ dx² + dy² ≤ 2r² ≤ (1.5r)² — so at least 8r ≥ 32 pixels, far
/// above the 10 the annulus needs: `measure_star` never falls back to the map for want of them.
#[test]
fn local_annulus_fills_at_the_tightest_stamp() {
    for radius in MIN_STAMP_RADIUS..=MAX_STAMP_RADIUS {
        let side = 2 * radius + 1;
        let residual = Buffer2::new_filled(side, side, 0.25f32);
        let corner = DVec2::splat(radius as f64);
        assert!(is_valid_stamp_position(
            corner,
            Size2us::new(side, side),
            radius
        ));
        let sky =
            compute_annulus_background(&residual, corner, radius, annulus_outer_radius(radius));
        assert!(sky.is_some(), "radius {radius}");
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
