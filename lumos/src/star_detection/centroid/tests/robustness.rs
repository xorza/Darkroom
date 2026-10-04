use super::*;
use crate::star_detection::detector::stages::filter::Rejection;

/// Create two overlapping stars on a 0.1 sky.
fn make_blended_stars(
    size: Size2us,
    pos1: Vec2,
    pos2: Vec2,
    sigma: f32,
    amp1: f32,
    amp2: f32,
) -> Buffer2<f32> {
    let mut pixels =
        SyntheticStar::new(pos1, amp1, StarProfile::Gaussian { sigma }).stamp(size, 0.1);
    SyntheticStar::new(pos2, amp2, StarProfile::Gaussian { sigma }).add_exact(&mut pixels);
    pixels
}

/// A companion pulls the converged windowed centre toward itself by exactly what the window gives
/// it. The centre stops where the windowed mean offset vanishes. Under a Gaussian window of `σ_w`
/// about `p`, a Gaussian star of σ and amplitude A at `c` weighs `W = A·e^(−(c − p)²/2(σ² + σ_w²))`
/// and its windowed light centres a share `σ_w²/(σ² + σ_w²)` of the way from `p` to `c`, so the
/// offset vanishes where `p = W₂·d / (W₁ + W₂)`, a fixed point solved here by bisection. The pixels
/// hold each Gaussian convolved with the unit box, a Gaussian of `σ² + 1/12` up to the box's
/// kurtosis, which the derivation leaves out: it moves the centre by under 3e-5 px (measured
/// 5.5e-6 at d 8, 2.8e-5 at d 5). Both windowed products lie well inside the stamp, so the sampled
/// sums match the integrals, and the centre lands within that and twice its tolerance of 1e-5 px.
///
/// The same companion shows in the shape: a round star alone has eccentricity 0 to rounding, and
/// beside the companion clearly more (measured 0.557 and 0.763).
#[test]
fn the_windowed_centre_is_pulled_toward_a_companion_as_derived() {
    let size = Size2us::new(64, 64);
    let fwhm = sigma_to_fwhm(2.5);
    let window = MeasureGrid::new(fwhm).window_sigma;
    let spread = 2.0 * (2.5f64 * 2.5 + 1.0 / 12.0 + window * window);
    let alone = Measured::flat(
        &SyntheticStar::new(Vec2::splat(32.0), 0.8, StarProfile::Gaussian { sigma: 2.5 })
            .stamp(size, 0.1),
        0.1,
        0.01,
    );
    let round = alone
        .compute(DVec2::splat(32.0), TEST_STAMP_RADIUS)
        .unwrap();
    assert!(round.eccentricity <= 3e-4, "alone: {}", round.eccentricity);

    for (distance, amplitude) in [(8.0f32, 0.3f32), (5.0, 0.5)] {
        let pixels = make_blended_stars(
            size,
            Vec2::splat(32.0),
            Vec2::new(32.0 + distance, 32.0),
            2.5,
            0.8,
            amplitude,
        );
        let measured = Measured::flat(&pixels, 0.1, 0.01);
        let centre = measured
            .windowed(DVec2::splat(32.0), fwhm)
            .expect("the centroid converges")
            .pos;

        let d = f64::from(distance);
        let ratio = f64::from(amplitude) / 0.8;
        let excess = |p: f64| {
            let primary = (-p * p / spread).exp();
            let companion = ratio * (-(d - p) * (d - p) / spread).exp();
            p * (primary + companion) - companion * d
        };
        let (mut low, mut high) = (0.0, d / 2.0);
        for _ in 0..100 {
            let mid = f64::midpoint(low, high);
            if excess(mid) < 0.0 {
                low = mid;
            } else {
                high = mid;
            }
        }
        let pull = f64::midpoint(low, high);
        assert!(
            (centre.x - 32.0 - pull).abs() <= 3e-5 + 2e-5,
            "d {distance}: pulled {} against {pull}",
            centre.x - 32.0
        );
        assert!(
            (centre.y - 32.0).abs() <= 1e-12,
            "d {distance}: {}",
            centre.y
        );

        let blended = measured
            .compute(DVec2::splat(32.0), TEST_STAMP_RADIUS)
            .unwrap();
        assert!(
            blended.eccentricity > 0.5,
            "d {distance}: {}",
            blended.eccentricity
        );
    }
}

/// A one-star Gaussian fit to a stamp with a 0.2 companion 7 px off sits on its own optimum of the
/// two-star light, pulled 0.3817 px toward the companion — no closed form for a model that does not
/// hold, so pinned to 1e-4 of the measured value, and along x only by symmetry.
#[test]
fn gaussian_fit_with_contamination() {
    let mut pixels =
        SyntheticStar::new(Vec2::splat(15.0), 0.8, StarProfile::Gaussian { sigma: 2.5 })
            .stamp(Size2us::new(31, 31), 0.1);
    SyntheticStar::new(
        Vec2::new(22.0, 15.0),
        0.2,
        StarProfile::Gaussian { sigma: 2.5 },
    )
    .add_exact(&mut pixels);
    let fit = GaussianFit::new(&pixels, DVec2::splat(15.0), &StampGrid::new(8), 0.1, None)
        .expect("the fit lands");
    assert!((fit.pos.x - 15.0 - 0.38168).abs() <= 1e-4, "{}", fit.pos);
    assert!((fit.pos.y - 15.0).abs() <= 1e-9, "{}", fit.pos);
}

/// The windowed centroid where a Gaussian's sampled moments are not exact: a star of σ 0.7 is
/// undersampled, its windowed light a Gaussian of `σ_e² = σ²σ_w²/(σ² + σ_w²)` = 0.33 at the window
/// of 1, which pixel-centre samples centre off the truth by a term of order `e^(−2π²σ_e²)`; and one
/// of σ 8 is cut by the largest stamp at under 2σ, which an off-centre stamp cuts unevenly. From
/// 0.76 px off both still converge, 0.0065 and 0.0355 px off (measured), held here to 1.5× that.
/// Their FWHMs read 2.149 against a true 1.648 and 18.02 against 18.84 (measured), pinned to 1e-3
/// relative.
#[test]
fn the_windowed_centroid_on_undersampled_and_truncated_stars() {
    for (sigma, side, position_bound, fwhm) in [
        (0.7f32, 64, 0.0098, 2.148_717),
        (8.0, 128, 0.054, 18.020_363),
    ] {
        let start = DVec2::splat((side / 2) as f64);
        let truth = (start + DVec2::new(0.3, 0.7)).as_vec2();
        let pixels = SyntheticStar::new(truth, 0.9, StarProfile::Gaussian { sigma })
            .stamp(Size2us::new(side, side), 0.1);
        let measured = Measured::flat(&pixels, 0.1, 0.01);
        let centre = measured
            .windowed(start, sigma_to_fwhm(sigma))
            .expect("the centroid converges")
            .pos;
        assert!(
            (centre - truth.as_dvec2()).length() <= position_bound,
            "σ {sigma}: {centre}"
        );
        let radius = if sigma < 1.0 { 5 } else { MAX_STAMP_RADIUS };
        let star = measured.compute(start, radius).unwrap();
        assert!(
            (star.fwhm - fwhm).abs() <= 1e-3 * fwhm,
            "σ {sigma}: FWHM {}",
            star.fwhm
        );
    }
}

/// Create a rotated elliptical Gaussian.
fn make_rotated_elliptical_star(
    size: Size2us,
    pos: Vec2,
    sigma_major: f32,
    sigma_minor: f32,
    angle_rad: f32,
    amplitude: f32,
) -> Buffer2<f32> {
    let mut pixels = Buffer2::new_filled(size.width, size.height, 0.1f32);

    SyntheticStar::new(
        pos,
        amplitude,
        StarProfile::Elliptical {
            sigma_x: sigma_major,
            sigma_y: sigma_minor,
            angle: angle_rad,
        },
    )
    .add_to(&mut pixels);

    pixels
}

/// A 4 × 2 ellipse at every angle.
///
/// Centred on a pixel, the light is point-symmetric about the start, so the windowed centroid
/// stays put (to 1e-12). Axis-aligned, the moments read [`MomentReading`]: eccentricity 0.866015
/// and FWHM 6.660577 against the PSF's 0.866025 and 6.660437, to 1e-6. Turned, the window's
/// response to the box's kurtosis, which lies along the pixel axes, shares out across the
/// ellipse's axes otherwise: within the size of that response, 1e-5 of either reading (measured
/// 3.1e-6 and 3.2e-6 across the angles). Started 0.76 px off, the Newton
/// gain `σ_w²·(σ_w²·I − C)⁻¹` is a matrix, which turns with the ellipse, so the centroid lands on it
/// within its tolerance at every angle.
#[test]
fn rotated_ellipse_moments_at_every_angle() {
    let size = Size2us::new(64, 64);
    let centre = DVec2::splat(32.0);
    for degrees in [0.0f32, 30.0, 45.0, 60.0, 90.0, 120.0, 135.0, 150.0] {
        let angle = degrees.to_radians();
        let centred = Measured::flat(
            &make_rotated_elliptical_star(size, centre.as_vec2(), 4.0, 2.0, angle, 0.8),
            0.1,
            0.01,
        );
        let still = centred.windowed(centre, 6.0).unwrap().pos;
        assert!((still - centre).length() <= 1e-12, "{degrees}°: {still}");
        let star = centred.compute(centre, 15).unwrap();
        let reading = MomentReading::of(4.0, 2.0);
        let bound = if degrees % 90.0 == 0.0 { 1e-6 } else { 1e-5 };
        assert!(
            (star.eccentricity - reading.eccentricity).abs() <= bound,
            "{degrees}°: {}",
            star.eccentricity
        );
        assert!(
            (star.fwhm - reading.fwhm).abs() <= bound * reading.fwhm,
            "{degrees}°: {}",
            star.fwhm
        );

        let truth = DVec2::new(32.3, 32.7).as_vec2();
        let offset = Measured::flat(
            &make_rotated_elliptical_star(size, truth, 4.0, 2.0, angle, 0.8),
            0.1,
            0.01,
        );
        let after = offset.windowed(centre, 6.0).unwrap().pos;
        assert!(
            (after - truth.as_dvec2()).length() <= 1e-4,
            "{degrees}° from off-centre: {after}"
        );
    }
}

/// The Gaussian fit recovers a rotated ellipse's full covariance, not only its axis projections.
#[test]
fn gaussian_fit_rotated_ellipse() {
    use crate::star_detection::centroid::gaussian_fit::GaussianFit;

    let width = 31;
    let height = 31;
    let true_cx = 15.0f64;
    let true_cy = 15.0f64;
    let background = 0.1f32;

    // Create 45° rotated ellipse
    let pixels = make_rotated_elliptical_star(
        Size2us::new(width, height),
        Vec2::new(true_cx as f32, true_cy as f32),
        3.5,
        2.0,
        FRAC_PI_4,
        0.8,
    );
    let result = GaussianFit::new(
        &pixels,
        DVec2::new(true_cx, true_cy),
        &StampGrid::new(8),
        background,
        None,
    );

    assert!(result.is_some(), "Should fit rotated ellipse");
    let result = result.unwrap();

    // Position should still be accurate
    let error = (result.pos.x - true_cx).hypot(result.pos.y - true_cy);
    assert!(
        error < 0.1,
        "Position error {error} too large for rotated ellipse fit"
    );

    // A 45° ellipse with principal σ 3.5 and 2.0 has covariance R·diag(σ₁², σ₂²)·Rᵀ:
    // xx = yy = (3.5² + 2²)/2 = 8.125 and xy = (3.5² − 2²)/2 = 4.125, the off-diagonal term an
    // axis-aligned model cannot represent. Noiseless samples of the model itself, so the fit is
    // exact up to the f32 rounding of the pixels.
    let c = result.covariance;
    for (got, want) in [(c.xx, 8.125), (c.yy, 8.125), (c.xy, 4.125)] {
        assert!((got - want).abs() < EXACT_FIT_PX2, "{got} vs {want}");
    }
    // √(1 − 2²/3.5²) = √(33/49) = 0.820652; σ_eq = √(3.5·2) = √7, FWHM = 2√(2 ln 2)·√7 = 6.230268.
    // Both read the covariance above, held to 1e-6 px², and come out as f32: 1e-5 is that error
    // carried through plus a few f32 ulps of the result.
    assert!((result.covariance.eccentricity() - 0.820_652).abs() < 1e-5);
    assert!((result.covariance.fwhm() - 6.230_268).abs() < 1e-5);
}

/// Test recovery from initial guess 2 pixels away from true position.
/// Under `GaussianFit` a 45° elongated star reports the eccentricity of its true shape —
/// √(1 − 2²/3.5²) = 0.8207 for σ 3.5 × 2.0 — and the filter rejects it as eccentric at the
/// `high_resolution` preset's 0.5. The axis-aligned fit read it as round, σx = σy = √8.125, and
/// passed it.
#[test]
fn gaussian_fit_rejects_a_diagonal_elongated_star() {
    let size = Size2us::new(64, 64);
    let pixels =
        make_rotated_elliptical_star(size, Vec2::new(32.0, 32.0), 3.5, 2.0, FRAC_PI_4, 0.8);
    let bg = background_map::uniform(size, 0.1, 0.01);
    let config = MeasurementConfig {
        centroid_method: CentroidMethod::GaussianFit,
        ..Default::default()
    };
    let region = Region {
        bbox: URect::new(Vec2us::new(24, 24), Vec2us::new(41, 41)),
        peak: Vec2us::new(32, 32),
        area: 200,
    };
    let expected_fwhm = 6.23;
    let star = measure_star(
        MeasurePlanes {
            residual: &bg.residual_of(&pixels),
            sky: &bg.sky_noise(),
            saturation: &unsaturated(&pixels),
            no_data: None,
        },
        &region,
        &config,
        &MeasureGrid::new(expected_fwhm),
    )
    .expect("the star measures");
    // FWHM = 2√(2 ln 2)·√(3.5·2) = 6.230268. The covariance is fitted to noiseless samples of its
    // own model, so both hold to a few f32 ulps; 1e-5 bounds that.
    assert!(
        (star.eccentricity - 0.820_652).abs() < 1e-5,
        "{}",
        star.eccentricity
    );
    assert!((star.fwhm - 6.230_268).abs() < 1e-5, "{}", star.fwhm);
    assert_eq!(
        Rejection::of(&star, &Config::high_resolution().filter),
        Some(Rejection::Eccentric)
    );
}
