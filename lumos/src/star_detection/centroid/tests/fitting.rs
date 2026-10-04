use super::*;

/// Every centroid method over a 10 × 10 grid of sub-pixel offsets in 0.1 px steps, each fit seeded
/// at the pixel nearest the star.
///
/// The profile fits are handed samples of their own model, so they must land on the truth to the
/// f32 rounding of the samples (≤ 1e-6 px; see `gaussian_fit`'s `RecoveryCase`), and every one must
/// converge. `measure_star`'s moments start at the seed and contract by 1/1.64 a step at the
/// matched window (see `convergence`), so ten steps leave the start's error times 1.64⁻¹⁰ =
/// 7.1e-3, with 1% for the sampling.
#[test]
fn every_method_on_a_sub_pixel_grid() {
    let size = Size2us::new(64, 64);
    let fwhm = sigma_to_fwhm(2.5);
    let moments = MeasurementConfig {
        centroid_method: CentroidMethod::WeightedMoments,
        ..Default::default()
    };
    for dx in 0..10 {
        for dy in 0..10 {
            // The star sits where its f32 centre rounds 32 + 0.1·k to.
            let truth = DVec2::new(32.0 + f64::from(dx) * 0.1, 32.0 + f64::from(dy) * 0.1)
                .as_vec2()
                .as_dvec2();
            let seed = truth.round();

            let gaussian =
                SyntheticStar::new(truth.as_vec2(), 1.0, StarProfile::Gaussian { sigma: 2.5 })
                    .stamp(size, 0.1);
            let fit = GaussianFit::new(&gaussian, seed, &StampGrid::new(8), 0.1, None)
                .expect("the Gaussian fit lands");
            assert!(
                (fit.pos - truth).length() <= 1e-6,
                "Gaussian at {truth}: {}",
                fit.pos
            );

            let moffat = SyntheticStar::new(
                truth.as_vec2(),
                1.0,
                StarProfile::Moffat {
                    alpha: 2.5,
                    beta: 2.5,
                },
            )
            .stamp(size, 0.1);
            let beta = 2.5;
            let fit = MoffatFit::new(&moffat, seed, &StampGrid::new(8), 0.1, None, beta)
                .expect("the Moffat fit lands");
            assert!(
                (fit.pos - truth).length() <= 1e-6,
                "Moffat at {truth}: {}",
                fit.pos
            );

            let measured = Measured::flat(&gaussian, 0.1, 0.01);
            let star = measured
                .measure(&measured.region_at(seed), &moments, fwhm)
                .expect("the moments measure");
            let bound = 1.01 * (seed - truth).length() * (1.0 / 1.64f64).powi(10);
            assert!(
                (star.pos - truth).length() <= bound,
                "moments at {truth}: {} against {bound}",
                star.pos
            );
        }
    }
}

/// The moment metrics of noiseless Gaussian stars at the stamp `measure_star` would give them.
///
/// The windowed covariance deconvolves its window exactly for a Gaussian, so FWHM and eccentricity
/// come out to the f32 rounding of the samples: 2e-6 of the FWHM, measured ≤ 1.1e-6. A round star's
/// eccentricity, √(1 − λ₂/λ₁), turns a rounding δ in the ratio into √δ: ≤ 3e-4. A round star reads
/// 0 on both roundness metrics; an elongated one reads photutils' `roundness2` and `roundness1` of
/// the same f32 samples' 7 × 7 DAOFIND cutout for the PSF of [`TEST_EXPECTED_FWHM`], from its
/// marginal fit and its quadrant slices reproduced in numpy, to 1e-6: a ulp of f32 `exp` in a sample and the f32 result.
/// Sharpness is the peak over the 3 × 3 core, 1/(1 + 2·e^(−1/2σ²))² for a round star on a pixel
/// centre.
#[test]
fn moment_metrics_of_gaussian_stars() {
    let size = Size2us::new(128, 128);
    let at = DVec2::splat(64.0);
    for (sigma_x, sigma_y, ground, sround) in [
        (1.5f32, 1.5f32, 0.0, 0.0),
        (2.0, 2.0, 0.0, 0.0),
        (2.5, 2.5, 0.0, 0.0),
        (3.0, 3.0, 0.0, 0.0),
        (3.5, 3.5, 0.0, 0.0),
        (4.0, 4.0, 0.0, 0.0),
        (3.0, 2.0, -0.662_517_3, -0.084_948_63),
        (4.0, 2.0, -1.089_321_7, -0.114_288_59),
        (2.0, 4.0, 1.089_321_7, 0.114_288_59),
    ] {
        let profile = StarProfile::Elliptical {
            sigma_x,
            sigma_y,
            angle: 0.0,
        };
        let pixels = SyntheticStar::new(at.as_vec2(), 0.8, profile).stamp(size, 0.1);
        let radius = MeasureGrid::stamp_radius(sigma_to_fwhm(sigma_x.max(sigma_y)));
        let star = Measured::flat(&pixels, 0.1, 0.01)
            .compute(at, radius)
            .expect("a star");

        let label = format!("σ {sigma_x} × {sigma_y}");
        let fwhm = sigma_to_fwhm((sigma_x * sigma_y).sqrt());
        assert!(
            (star.fwhm - fwhm).abs() <= 2e-6 * fwhm,
            "{label}: FWHM {}",
            star.fwhm
        );
        let (minor, major) = (sigma_x.min(sigma_y), sigma_x.max(sigma_y));
        let eccentricity = (1.0 - (minor / major).powi(2)).sqrt();
        assert!(
            (star.eccentricity - eccentricity).abs() <= 3e-4,
            "{label}: eccentricity {}",
            star.eccentricity
        );
        assert!(
            (star.roundness.ground - ground).abs() <= 1e-6
                && (star.roundness.sround - sround).abs() <= 1e-6,
            "{label}: {:?}",
            star.roundness
        );
        if sigma_x == sigma_y {
            let core = 1.0 + 2.0 * (-1.0 / (2.0 * sigma_x * sigma_x)).exp();
            let sharpness = 1.0 / (core * core);
            assert!(
                (star.sharpness - sharpness).abs() <= 1e-6,
                "{label}: sharpness {}",
                star.sharpness
            );
        }
    }
}

/// Both profile fits accept or reject a centre through one predicate, so its bounds are pinned
/// once here rather than per model.
#[test]
fn fit_plausibility_rejects_non_finite_and_keeps_its_bounds() {
    use crate::star_detection::centroid::fit_is_plausible;

    let at = DVec2::splat(8.0);
    let radius = 8usize;
    assert!(fit_is_plausible(at, at, radius));

    // `max_element` skips a NaN lane, so a NaN x beside a finite y must still be caught.
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            !fit_is_plausible(DVec2::new(bad, 8.0), at, radius),
            "x = {bad}"
        );
        assert!(
            !fit_is_plausible(DVec2::new(8.0, bad), at, radius),
            "y = {bad}"
        );
    }

    // A centre exactly `stamp_radius` away on either axis is still inside; beyond it is not.
    assert!(fit_is_plausible(DVec2::new(16.0, 8.0), at, radius));
    assert!(fit_is_plausible(DVec2::new(8.0, 0.0), at, radius));
    assert!(!fit_is_plausible(DVec2::new(16.01, 8.0), at, radius));
    assert!(!fit_is_plausible(DVec2::new(8.0, -0.01), at, radius));
}
