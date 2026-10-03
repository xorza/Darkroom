//! Sub-pixel recovery of noisy stars through an estimated sky: each method lands within the
//! scatter the noise propagates to it, so the error falls with the star's amplitude as derived.

use super::*;
use std::f64::consts::PI;

/// One field of round Gaussians on a 0.1 sky with white noise σₙ = 0.01, its sky estimated at the
/// default tile size, each star measured from its nearest pixel at the matched FWHM 4.0 (σ = 1.70).
///
/// Per axis, five times the scatter each method propagates bounds its error:
/// - The Gaussian fit is efficient on its own model: the Cramér–Rao bound `√(2/π)·σₙ/A`. Its
///   sky is a free parameter, so the estimate's error does not reach it.
/// - The moments fixed point `p = F(p)` moves by `δF / (1 − c)`, with `c` = 1/1.64 the step's
///   contraction and `δF` one step's noise, `σₙ·√Σ(w·dx)² / Σ w·I` (`one_noisy_step_scatters_as_propagated`).
///   To that add what ten steps leave of the seed, and the pull of a sky estimated off by δ,
///   `δ·Σ w·|dx| / Σ w·I` per step.
///
/// The fit is checked to have converged — a failed fit falls back to the moments silently — by its
/// FWHM, which replaces the moments' only then.
#[test]
fn noisy_stars_land_within_their_propagated_scatter() {
    const NOISE: f64 = 0.01;
    const SKY: f32 = 0.1;
    let fwhm = 4.0f32;
    let sigma = f64::from(fwhm_to_sigma(fwhm));
    let radius = compute_stamp_radius(fwhm);
    let window_sq = (0.8 * sigma).powi(2);
    let contraction = sigma.powi(2) / (sigma.powi(2) + window_sq);
    let stars = [
        (DVec2::new(50.0, 50.0), 0.28f32),
        (DVec2::new(100.3, 50.2), 0.28),
        (DVec2::new(50.7, 100.8), 0.28),
        (DVec2::new(150.5, 150.5), 0.28),
        (DVec2::new(100.25, 100.75), 0.28),
        (DVec2::new(40.42, 200.37), 0.14),
        (DVec2::new(100.42, 200.37), 0.08),
        (DVec2::new(160.42, 200.37), 0.055),
    ];
    let mut pixels = Buffer2::new_filled(256, 256, SKY);
    for &(centre, amplitude) in &stars {
        SyntheticStar::new(
            centre.as_vec2(),
            amplitude,
            StarProfile::Gaussian {
                sigma: sigma as f32,
            },
        )
        .add_exact(&mut pixels);
    }
    patterns::add_gaussian_noise(pixels.pixels_mut(), NOISE as f32, 42);
    let background = background_map::estimate(&pixels, &BackgroundConfig::default());
    let sky_error = background
        .background
        .iter()
        .map(|&level| f64::from((level - SKY).abs()))
        .fold(0.0, f64::max);
    let measured = Measured::of(&pixels, &background);

    for &(centre, amplitude) in &stars {
        let truth = centre.as_vec2().as_dvec2();
        let amplitude = f64::from(amplitude);
        let seed = truth.round();
        let (mut spread, mut pull, mut light) = (0.0f64, 0.0f64, 0.0f64);
        let r = radius as i32;
        for dy in -r..=r {
            for dx in -r..=r {
                let offset = seed + DVec2::new(f64::from(dx), f64::from(dy)) - truth;
                let weight = (-offset.length_squared() / (2.0 * window_sq)).exp();
                let star = amplitude * (-offset.length_squared() / (2.0 * sigma.powi(2))).exp();
                spread += (weight * offset.x).powi(2).max((weight * offset.y).powi(2));
                pull += weight * offset.x.abs().max(offset.y.abs());
                light += weight * star;
            }
        }
        let moments_bound =
            (5.0 * NOISE * spread.sqrt() + sky_error * pull) / light / (1.0 - contraction)
                + 1.01 * (seed - truth).length() * contraction.powi(10);
        let fit_bound = 5.0 * (2.0 / PI).sqrt() * NOISE / amplitude;

        let region = measured.region_at(truth);
        let measure = |centroid_method| {
            let config = MeasurementConfig {
                centroid_method,
                ..Default::default()
            };
            measured
                .measure(&region, &config, fwhm)
                .expect("the star measures")
        };
        let moments = measure(CentroidMethod::WeightedMoments);
        let fit = measure(CentroidMethod::GaussianFit);
        assert_ne!(fit.fwhm, moments.fwhm, "{truth}: the fit converged");

        for (method, star, bound) in [("moments", moments, moments_bound), ("fit", fit, fit_bound)]
        {
            let error = (star.pos - truth).abs().max_element();
            assert!(
                error <= bound,
                "{truth}, A {amplitude}, {method}: {error} > {bound}"
            );
        }
    }
}
