#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use super::*;
use std::f32::consts::PI;

/// The windowed centroid against what it must return. A star centred on the start stays put: its
/// light is point-symmetric about the window. A lone pixel `k` columns off moves the centre onto it
/// in one step and stays: at k = 5, within half the stamp radius, 11/2; at k = 6, beyond it, the
/// centroid is refused. No positive light at all, or a stamp off the frame, gives nothing.
#[test]
fn windowed_centroid_limits() {
    let size = Size2us::new(64, 64);
    let centre = DVec2::splat(32.0);
    let star = Measured::flat(
        &SyntheticStar::new(centre.as_vec2(), 0.8, StarProfile::Gaussian { sigma: 2.5 })
            .stamp(size, 0.1),
        0.1,
        0.01,
    );
    let grid = MeasureGrid::new(TEST_EXPECTED_FWHM);
    let measure = |residual: &Buffer2<f32>, from: DVec2| {
        WindowedCentroid::measure(
            residual,
            from,
            &grid,
            WindowedInputs {
                offset: 0.0,
                sky_sigma: 0.01,
                noise_model: None,
            },
        )
        .map(|centre| centre.pos)
    };
    let centred = measure(&star.residual, centre).unwrap();
    assert!((centred - centre).length() <= 1e-12, "{centred}");
    assert_eq!(
        measure(&star.residual, DVec2::new(3.0, 32.0)),
        None,
        "off the frame"
    );

    let lone = |k: usize| {
        let mut residual = Buffer2::new_filled(64, 64, 0.0f32);
        residual[(32 + k, 32)] = 1.0;
        residual
    };
    assert_eq!(measure(&lone(5), centre), Some(DVec2::new(37.0, 32.0)));
    assert_eq!(
        measure(&lone(6), centre),
        None,
        "moved past half the radius"
    );

    assert_eq!(
        measure(&Buffer2::new_filled(64, 64, 0.0), centre),
        None,
        "no light"
    );
    assert_eq!(
        measure(&Buffer2::new_filled(64, 64, -0.5), centre),
        None,
        "only negative light"
    );
}

#[test]
fn compute_star_of_a_clean_star() {
    let size = Size2us::new(64, 64);
    let centre = DVec2::splat(32.0);
    let npix = (2 * TEST_STAMP_RADIUS + 1) as f32;
    let reference = |amplitude: f32| {
        let pixels = SyntheticStar::new(
            centre.as_vec2(),
            amplitude,
            StarProfile::Gaussian { sigma: 2.5 },
        )
        .stamp(size, 0.1);
        Measured::flat(&pixels, 0.1, 0.01)
            .compute(centre, TEST_STAMP_RADIUS)
            .expect("a star")
    };

    let base = reference(0.8);
    let flux = 2.0 * PI * 6.25 * 0.8;
    assert!(
        (base.flux - flux).abs() <= 2.5e-5 * flux,
        "flux {}",
        base.flux
    );
    assert!(
        (base.snr - base.flux / (0.01 * npix)).abs() <= 1e-6 * base.snr,
        "SNR {}",
        base.snr
    );
    assert_eq!(base.pos, centre);

    for scale in [0.5f32, 2.0] {
        let scaled = reference(0.8 * scale);
        assert!(
            (scaled.flux / base.flux - scale).abs() <= 1e-6 * scale,
            "flux at ×{scale}: {}",
            scaled.flux
        );
        assert!(
            (scaled.snr / base.snr - scale).abs() <= 1e-6 * scale,
            "SNR at ×{scale}: {}",
            scaled.snr
        );
        assert!(
            (scaled.fwhm - base.fwhm).abs() <= 1e-6 * base.fwhm,
            "FWHM at ×{scale}"
        );
        assert!(
            (scaled.eccentricity - base.eccentricity).abs() <= 3e-4,
            "eccentricity at ×{scale}"
        );
    }
}

/// Doubling the sky noise halves the SNR exactly: the flux is the same, and the noise the SNR
/// reads, the mean of a uniform map over the stamp's outer ring, doubles bit for bit.
#[test]
fn snr_halves_when_the_noise_doubles() {
    let size = Size2us::new(64, 64);
    let pixels = SyntheticStar::new(Vec2::splat(32.0), 0.8, StarProfile::Gaussian { sigma: 2.5 })
        .stamp(size, 0.1);
    let snr = |noise: f32| {
        Measured::flat(&pixels, 0.1, noise)
            .compute(DVec2::splat(32.0), TEST_STAMP_RADIUS)
            .unwrap()
            .snr
    };
    assert_eq!(snr(0.02), 2.0 * snr(0.04));
}

/// The same star on a sky of 10 measures as on a sky of 0.1: the sky is gone from the residual,
/// up to the f32 rounding of `v + 10 − 10` — 2⁻²⁰ per pixel, 529 · 1e-6 = 5e-4 of a flux of 15.7.
#[test]
fn a_bright_sky_changes_nothing() {
    let size = Size2us::new(64, 64);
    let measure = |sky: f32| {
        let pixels =
            SyntheticStar::new(Vec2::splat(32.0), 0.8, StarProfile::Gaussian { sigma: 2.5 })
                .stamp(size, sky);
        Measured::flat(&pixels, sky, 0.01)
            .compute(DVec2::splat(32.0), TEST_STAMP_RADIUS)
            .unwrap()
    };
    let (dark, bright) = (measure(0.1), measure(10.0));
    assert!(
        (bright.flux - dark.flux).abs() <= 5e-4,
        "{} vs {}",
        bright.flux,
        dark.flux
    );
    assert!((bright.fwhm - dark.fwhm).abs() <= 1e-4 * dark.fwhm);
}

/// The windowed centroid under white noise σₙ reports the σ the noise gives it. The centre is a
/// ratio of windowed sums moved by the Newton gain `F = (σ_w² + σ_s²)/σ_w²` of a Gaussian star, so
/// to first order its error is `F·σₙ·√Σ(w·dx)² / Σ w·I` per axis, with `w` the window and `I` the
/// noiseless star — computed here from the same stamp. The reported σ reads the noisy stamp, whose
/// windowed light and spread err by a few percent at this noise, so it agrees within 10%; the
/// error itself lies within 5 of it.
#[test]
fn the_windowed_centroid_reports_its_propagated_noise() {
    let size = Size2us::new(64, 64);
    let centre = DVec2::splat(32.0);
    let clean = SyntheticStar::new(centre.as_vec2(), 0.8, StarProfile::Gaussian { sigma: 2.5 })
        .stamp(size, 0.1);
    let noise = 0.05f32;
    let mut noisy = clean.clone();
    patterns::add_gaussian_noise(&mut noisy, noise, 21);

    let grid = MeasureGrid::new(TEST_EXPECTED_FWHM);
    let window2 = grid.window_sigma * grid.window_sigma;
    let gain = (window2 + 6.25) / window2;
    let r = TEST_STAMP_RADIUS as isize;
    let (mut spread, mut light) = (0.0f64, 0.0f64);
    for dy in -r..=r {
        for dx in -r..=r {
            let w = (-((dx * dx + dy * dy) as f64) / (2.0 * window2)).exp();
            let value = f64::from(clean[((32 + dx) as usize, (32 + dy) as usize)] - 0.1);
            spread += (w * dx as f64).powi(2);
            light += w * value;
        }
    }
    let expected = gain * f64::from(noise) * spread.sqrt() / light;

    let measured = WindowedCentroid::measure(
        &Measured::flat(&noisy, 0.1, noise).residual,
        centre,
        &grid,
        WindowedInputs {
            offset: 0.0,
            sky_sigma: noise,
            noise_model: None,
        },
    )
    .unwrap();
    assert!(
        (measured.sigma / expected - 1.0).abs() <= 0.1,
        "{} against {expected}",
        measured.sigma
    );
    let error = (measured.pos - centre).abs();
    assert!(
        error.x <= 5.0 * measured.sigma && error.y <= 5.0 * measured.sigma,
        "{error} against {}",
        measured.sigma
    );
}

/// Two stars far apart, each measured from its own detection, land on their own centres: each is
/// point-symmetric about its peak pixel and the other is outside its stamp, so exactly.
#[test]
fn measure_star_multiple_stars_independent() {
    let mut pixels = Buffer2::new_filled(128, 128, 0.1f32);
    for (centre, amplitude) in [(Vec2::splat(40.0), 0.8), (Vec2::splat(90.0), 0.6)] {
        SyntheticStar::new(centre, amplitude, StarProfile::Gaussian { sigma: 2.5 })
            .add_to(&mut pixels);
    }
    let measured = Measured::flat(&pixels, 0.1, 0.01);
    let candidates = detect_stars_test(
        &measured.residual,
        &measured.sky,
        &DetectionConfig::default(),
    );
    let mut positions: Vec<DVec2> = candidates
        .iter()
        .map(|candidate| {
            measured
                .measure(candidate, &MeasurementConfig::default(), TEST_EXPECTED_FWHM)
                .expect("a star")
                .pos
        })
        .collect();
    positions.sort_by(|a, b| a.x.total_cmp(&b.x));
    assert_eq!(positions.len(), 2);
    for (position, truth) in positions
        .iter()
        .zip([DVec2::splat(40.0), DVec2::splat(90.0)])
    {
        assert!(
            (*position - truth).length() <= 1e-9,
            "{position} vs {truth}"
        );
    }

    // A candidate whose stamp would leave the frame is not measured.
    let edge = Region {
        bbox: URect::new(Vec2us::new(0, 30), Vec2us::new(6, 36)),
        peak: Vec2us::new(3, 32),
        area: 18,
    };
    assert!(
        measured
            .measure(&edge, &MeasurementConfig::default(), TEST_EXPECTED_FWHM)
            .is_none()
    );
}

/// A tail on one side breaks the symmetry SROUND measures: a 0.3 companion stretched along x,
/// 4 px right of a round star, against the round star alone (SROUND 0 exactly; see `fitting`).
#[test]
fn a_one_sided_tail_raises_sround() {
    let mut pixels =
        SyntheticStar::new(Vec2::splat(32.0), 0.8, StarProfile::Gaussian { sigma: 2.5 })
            .stamp(Size2us::new(64, 64), 0.1);
    SyntheticStar::new(
        Vec2::new(36.0, 32.0),
        0.3,
        StarProfile::Elliptical {
            sigma_x: 2.0,
            sigma_y: 1.0,
            angle: 0.0,
        },
    )
    .add_to(&mut pixels);
    let star = Measured::flat(&pixels, 0.1, 0.01)
        .compute(DVec2::splat(32.0), TEST_STAMP_RADIUS)
        .unwrap();
    eprintln!("SROUND {}", star.roundness.sround);
    assert!(
        star.roundness.sround > 0.01,
        "SROUND {}",
        star.roundness.sround
    );
}

#[test]
fn compute_star_local_offset_removes_what_the_global_map_left() {
    // Star (sigma 2.5, amplitude 0.8) on an exact 0.1 pedestal, with a global map that
    // under-estimates it at 0.05 (noise 0.01): the residual still carries 0.05 at every pixel.
    // A local offset of 0.05 removes it from every one of the 15² = 225 stamp pixels, so
    // flux_global − flux_local = 0.05 · 225 = 11.25 (f32 sums of 225 terms near 1: within 1e-3).
    // The local noise (0.05, vs the map's 0.01) must feed the simplified SNR formula
    // `flux / (noise * sqrt(npix))`, and the offset must reach the windowed covariance, where
    // the pedestal left in the global run inflates the second moments and so the FWHM.
    let width = 64;
    let height = 64;
    let pos = DVec2::splat(32.0);
    let pixels = SyntheticStar::new(pos.as_vec2(), 0.8, StarProfile::Gaussian { sigma: 2.5 })
        .stamp(Size2us::new(width, height), 0.1);
    let bg = background_map::uniform(Size2us::new(width, height), 0.05, 0.01);
    let residual = bg.residual_of(&pixels);
    let sky = bg.sky_noise();
    let local_bg = LocalBackground {
        offset: 0.05,
        noise: 0.05,
    };

    let global = compute_star(&residual, &sky, pos, 0.0, TEST_STAMP_RADIUS, None, None).unwrap();
    let local = compute_star(
        &residual,
        &sky,
        pos,
        0.0,
        TEST_STAMP_RADIUS,
        Some(local_bg),
        None,
    )
    .unwrap();

    let npix = (2 * TEST_STAMP_RADIUS + 1).pow(2) as f32;
    let flux_diff = global.flux - local.flux;
    assert!(
        (flux_diff - 0.05 * npix).abs() < 1e-3,
        "the local offset must remove exactly 0.05/pixel: flux diff {flux_diff}"
    );

    let expected_snr = local.flux / (0.05 * npix.sqrt());
    assert!(
        (local.snr - expected_snr).abs() / expected_snr < 1e-6,
        "local noise must feed the SNR: got {}, expected {expected_snr}",
        local.snr
    );

    assert!(
        global.fwhm > local.fwhm,
        "the offset must reach the windowed covariance: the pedestal left in the global run \
         should inflate its FWHM (global {} vs local {})",
        global.fwhm,
        local.fwhm
    );
}

#[test]
fn compute_star_invalid_position_returns_none() {
    let pixels = Buffer2::new_filled(64, 64, 0.5f32);
    let measured = Measured::flat(&pixels, 0.1, 0.01);
    assert!(
        measured
            .compute(DVec2::new(3.0, 32.0), TEST_STAMP_RADIUS)
            .is_none()
    );
    // No net light above the sky: not a star.
    let flat = Measured::flat(&Buffer2::new_filled(64, 64, 0.1f32), 0.1, 0.01);
    assert!(
        flat.compute(DVec2::splat(32.0), TEST_STAMP_RADIUS)
            .is_none()
    );
}

/// Metrics must be measured against a sky annulus centred on the position the fit actually
/// reported. The annulus samples by rounded centre, so when the fit crosses a pixel boundary the
/// estimate taken back at the moments position describes a different ring — on a sky gradient
/// that lands straight in flux and SNR.
#[test]
fn annulus_sky_is_centred_on_the_fitted_position() {
    let size = Size2us::new(64, 64);
    // 0.02 of sky per column, so shifting the annulus one pixel moves its sigma-clipped median by
    // ~0.02 — several percent of this star's flux once summed over the stamp.
    let sky = |x: usize| 0.1 + 0.02 * x as f32;
    let mut pixels =
        SyntheticStar::new(Vec2::splat(32.0), 1.0, StarProfile::Gaussian { sigma: 2.5 })
            .stamp(size, 0.0);
    let mut sky_plane = Buffer2::new_filled(size.width, size.height, 0.0f32);
    for y in 0..size.height {
        for x in 0..size.width {
            pixels[(x, y)] += sky(x);
            sky_plane[(x, y)] = sky(x);
        }
    }
    let measured = Measured::of(
        &pixels,
        &BackgroundEstimate {
            background: sky_plane,
            noise: Buffer2::new_filled(size.width, size.height, 0.01),
            noise_floor: 1e-6,
        },
    );
    let config = MeasurementConfig {
        centroid_method: CentroidMethod::GaussianFit,
        local_background: LocalBackgroundMethod::LocalAnnulus,
        ..Default::default()
    };
    let radius = MeasureGrid::stamp_radius(4.0);
    // Seeded two pixels off the star, so the fit has to cross a pixel boundary to reach it.
    let region = Region {
        bbox: URect::new(Vec2us::new(28, 26), Vec2us::new(40, 38)),
        peak: Vec2us::new(34, 32),
        area: 40,
    };
    let star = measured
        .measure(&region, &config, 4.0)
        .expect("star should measure");

    // Same metrics pass, but with the sky annulus explicitly centred where the fit ended up.
    let sky_at_fit =
        compute_annulus_background(&measured.residual, star.pos, MeasureGrid::new(4.0).annulus)
            .expect("annulus has samples");
    let expected = compute_star(
        &measured.residual,
        &measured.sky,
        star.pos,
        measured.residual[(region.peak.x, region.peak.y)],
        radius,
        Some(sky_at_fit),
        config.noise_model.as_ref(),
    )
    .expect("reference measurement");

    assert_eq!(
        star.flux, expected.flux,
        "flux came from a sky annulus centred somewhere other than the reported position"
    );
    assert_eq!(star.snr, expected.snr, "snr disagrees for the same reason");
}

/// Sky noise is zero-mean, so it must not add flux. A ±σ checkerboard on a known sky is the exact
/// case: the stamp holds one more `+σ` than `−σ` pixel, so its signed flux is exactly σ and its
/// SNR exactly `σ / (σ·√npix) = 1 / (2r + 1)` — every value is dyadic or a perfect square, so the
/// f32 arithmetic is exact. Clipping each pixel at zero instead keeps the `(npix + 1) / 2`
/// positive pixels and reports `(npix + 1) / (2·√npix)`, about `r` — above the default
/// `min_snr = 10` from r = 10 on, for a stamp holding no star at all.
#[test]
fn sky_noise_adds_no_flux_or_snr() {
    const SIGMA: f32 = 0.0625;
    const SKY: f32 = 0.25;
    let size = Size2us::new(48, 48);
    let pixels = Buffer2::new(
        size.width,
        size.height,
        (0..size.pixel_count())
            .map(|i| {
                let (x, y) = (i % size.width, i / size.width);
                if (x + y) % 2 == 0 {
                    SKY + SIGMA
                } else {
                    SKY - SIGMA
                }
            })
            .collect(),
    );
    let bg = background_map::uniform(size, SKY, SIGMA);

    for radius in [7, 13, 15] {
        let star = compute_star(
            &bg.residual_of(&pixels),
            &bg.sky_noise(),
            DVec2::splat(24.0),
            0.0,
            radius,
            None,
            None,
        )
        .expect("one net +σ pixel is positive flux");
        assert_eq!(
            star.flux, SIGMA,
            "r = {radius}: the signed sum of the stamp"
        );
        assert_eq!(star.snr, 1.0 / (2 * radius + 1) as f32, "r = {radius}");
    }
}

#[test]
fn empty_sky_stamps_measure_no_signal() {
    // Pure zero-mean noise, σ = 0.01, the sky already removed. Each stamp's signed flux is a sum
    // of npix independent N(0, σ²) samples, so SNR = flux / (σ·√npix) is N(0, 1): about half the
    // stamps have no net signal and are not stars, and the rest stay low — over the ≤ 1156
    // disjoint stamps here, P(any Z > 4.5) < 1156 · 3.4e-6 ≈ 0.004. Clipping each pixel at 0
    // instead adds σ/√(2π) ≈ 0.399σ per pixel, an SNR of 0.399·√npix: 6.0 at r = 7, 10.8 at 13
    // and 12.4 at 15, every stamp a "star".
    let side = 496;
    let size = Size2us::new(side, side);
    let sigma = 0.01f32;
    let mut residual = Buffer2::new_filled(side, side, 0.0f32);
    patterns::add_gaussian_noise(residual.pixels_mut(), sigma, 7);
    let sky = background_map::uniform(size, 0.0, sigma).sky_noise();

    for radius in [7usize, 13, 15] {
        let stride = 2 * radius + 1;
        let centres: Vec<usize> = (radius + 1..side - radius - 1).step_by(stride).collect();
        let mut measured = 0usize;
        for &cy in &centres {
            for &cx in &centres {
                let pos = DVec2::new(cx as f64, cy as f64);
                if let Some(star) = compute_star(&residual, &sky, pos, 0.0, radius, None, None) {
                    measured += 1;
                    assert!(
                        star.snr < 4.5,
                        "r = {radius}: an empty stamp at {pos} measured SNR {}",
                        star.snr
                    );
                }
            }
        }
        // Binomial(N, ½) with N ≥ 225: the share measured is within 0.5 ± 0.15, over 4.5σ.
        let share = measured as f64 / (centres.len() * centres.len()) as f64;
        assert!(
            (share - 0.5).abs() < 0.15,
            "r = {radius}: {share} of empty stamps carried net signal"
        );
    }
}
