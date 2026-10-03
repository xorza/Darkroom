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
            let fit = GaussianFit::new(
                &gaussian,
                seed,
                &StampGrid::new(8),
                0.1,
                None,
                &GaussianFitConfig::default(),
            )
            .expect("the Gaussian fit lands");
            assert!(fit.converged, "Gaussian at {truth}");
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
            let config = MoffatFitConfig {
                fixed_beta: 2.5,
                ..Default::default()
            };
            let fit = MoffatFit::new(&moffat, seed, &StampGrid::new(8), 0.1, None, &config)
                .expect("the Moffat fit lands");
            assert!(fit.converged, "Moffat at {truth}");
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
/// eccentricity, √(1 − λ₂/λ₁), turns a rounding δ in the ratio into √δ: ≤ 3e-4. The marginals of an
/// axis-aligned star peak at `A·σ_y·√(2π)` and `A·σ_x·√(2π)`, so GROUND = (σy − σx)/(σx + σy) — up
/// to the stamp's truncation, ≤ 4.5e-5 at 3.75σ — and SROUND is zero for any star symmetric in both
/// axes. Sharpness is the peak over the 3 × 3 core, 1/(1 + 2·e^(−1/2σ²))² for a round star on a
/// pixel centre.
#[test]
fn moment_metrics_of_gaussian_stars() {
    let size = Size2us::new(128, 128);
    let at = DVec2::splat(64.0);
    for (sigma_x, sigma_y) in [
        (1.5f32, 1.5f32),
        (2.0, 2.0),
        (2.5, 2.5),
        (3.0, 3.0),
        (3.5, 3.5),
        (4.0, 4.0),
        (3.0, 2.0),
        (4.0, 2.0),
        (2.0, 4.0),
    ] {
        let profile = StarProfile::Elliptical {
            sigma_x,
            sigma_y,
            angle: 0.0,
        };
        let pixels = SyntheticStar::new(at.as_vec2(), 0.8, profile).stamp(size, 0.1);
        let radius = compute_stamp_radius(sigma_to_fwhm(sigma_x.max(sigma_y)));
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
        let ground = (sigma_y - sigma_x) / (sigma_x + sigma_y);
        assert!(
            (star.roundness.ground - ground).abs() <= 1e-4,
            "{label}: GROUND {}",
            star.roundness.ground
        );
        assert_eq!(star.roundness.sround, 0.0, "{label}: SROUND");
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

#[test]
fn snr_uses_normalized_noise_units() {
    let model = NoiseModel::from_normalized(1_000.0, 10.0);

    // Model variance = 2/1000 + 4 × (0.02² + (10/1000)²) = 0.004.
    let modeled = compute_snr(2.0, 0.02, 4, Some(&model));
    let expected_modeled = 2.0 / 0.004_f32.sqrt();
    assert!((modeled - expected_modeled).abs() < 1e-5);

    // Background-only variance = 4 × 0.02² = 0.0016.
    let background_only = compute_snr(2.0, 0.02, 4, None);
    assert!((background_only - 50.0).abs() < 1e-5);
    assert_ne!(modeled, background_only);
}

/// The SNR depends on the flux against the noise, not on the frame's scale: flux 2 against σ 0.02
/// over 4 pixels is 50, and scaled by 2⁻²⁰ or 2⁻⁶⁰, both together, bit for bit the same, since a
/// power of two scales every product and the square root exactly.
#[test]
fn snr_is_invariant_to_the_frames_scale() {
    let native = compute_snr(2.0, 0.02, 4, None);
    assert!((native - 50.0).abs() < 1e-5, "{native}");
    for scale in [2.0f32.powi(-20), 2.0f32.powi(-60)] {
        assert_eq!(
            compute_snr(2.0 * scale, 0.02 * scale, 4, None).to_bits(),
            native.to_bits(),
            "scale {scale}"
        );
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
