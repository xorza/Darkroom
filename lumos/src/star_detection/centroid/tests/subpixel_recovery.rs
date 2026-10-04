//! Sub-pixel recovery of noisy stars through an estimated sky: each method lands within the
//! scatter the noise propagates to it, so the error falls with the star's amplitude as derived.

use super::*;
use rayon::prelude::*;

/// One field of round Gaussians on a 0.1 sky with white noise σₙ = 0.01, its sky estimated at the
/// default tile size, each star measured from its nearest pixel at the matched FWHM 4.0, lands
/// within five of the position σ it reports, per axis, by either method. The converged windowed
/// centre is not moved by a flat sky error, whose windowed offset vanishes at the centre, so the
/// reported σ, the pixel noise's alone, bounds it.
///
/// The fit is checked to have converged — a failed fit falls back to the windowed centre — by its
/// FWHM, which replaces the moments' only then.
#[test]
fn noisy_stars_land_within_their_reported_sigma() {
    const NOISE: f32 = 0.01;
    const SKY: f32 = 0.1;
    let fwhm = 4.0f32;
    let sigma = fwhm_to_sigma(fwhm);
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
        SyntheticStar::new(centre.as_vec2(), amplitude, StarProfile::Gaussian { sigma })
            .add_exact(&mut pixels);
    }
    patterns::add_gaussian_noise(pixels.pixels_mut(), NOISE, 42);
    let background = background_map::estimate(&pixels, &BackgroundConfig::default());
    let measured = Measured::of(&pixels, &background);

    for &(centre, amplitude) in &stars {
        let truth = centre.as_vec2().as_dvec2();
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

        for (method, star) in [("moments", moments), ("fit", fit)] {
            let error = (star.pos - truth).abs().max_element();
            assert!(
                error <= 5.0 * star.position_sigma,
                "{truth}, A {amplitude}, {method}: {error} > 5 × {}",
                star.position_sigma
            );
        }
    }
}

/// Both sources of the position σ agree with the scatter they predict: one star of amplitude 0.3
/// and σ 1.7 at a sub-pixel offset under 1000 fixed-seed draws of white noise σₙ = 0.01. The mean
/// square error about the truth, per axis, is a variance from 2000 samples (x and y), so it
/// carries a standard error of `√(2/2000)` = 3.2% of itself; the mean reported σ² must lie within
/// 3 of those, 9.5%. The truth's own bias is far below the noise: a Gaussian is its own model
/// and the windowed centre lands on it within 1e-5 px.
/// One draw's squared error per axis and reported σ², from each source.
#[derive(Debug, Clone, Copy)]
struct Draw {
    windowed_scatter: f64,
    windowed_reported: f64,
    fit_scatter: f64,
    fit_reported: f64,
}

#[test]
fn the_position_sigma_matches_the_scatter() {
    const DRAWS: u64 = 1000;
    const NOISE: f32 = 0.01;
    let fwhm = 4.0f32;
    let truth = DVec2::new(16.3, 15.6);
    let clean = SyntheticStar::new(
        truth.as_vec2(),
        0.3,
        StarProfile::Gaussian {
            sigma: fwhm_to_sigma(fwhm),
        },
    )
    .stamp(Size2us::new(32, 32), 0.1);
    let grid = MeasureGrid::new(fwhm);
    // The draws are independent, so they run in parallel; the sums run in seed order.
    let draws: Vec<Draw> = (0..DRAWS)
        .into_par_iter()
        .map(|seed| {
            let mut pixels = clean.clone();
            patterns::add_gaussian_noise(pixels.pixels_mut(), NOISE, seed);
            let residual =
                background_map::uniform(Size2us::new(32, 32), 0.1, NOISE).residual_of(&pixels);
            let windowed = WindowedCentroid::measure(
                &residual,
                truth.round(),
                &grid,
                WindowedInputs {
                    offset: 0.0,
                    noise: StarNoise {
                        background_sigma: f64::from(NOISE),
                        electrons_per_unit: None,
                    },
                },
            )
            .unwrap();
            let fit = GaussianFit::new(&residual, windowed.pos, &grid.stamp, 0.0, None).unwrap();
            Draw {
                windowed_scatter: (windowed.pos - truth).length_squared() / 2.0,
                windowed_reported: windowed.sigma * windowed.sigma,
                fit_scatter: (fit.pos - truth).length_squared() / 2.0,
                fit_reported: fit.position_sigma * fit.position_sigma,
            }
        })
        .collect();
    let sum = |field: fn(&Draw) -> f64| draws.iter().map(field).sum::<f64>();
    let (windowed_scatter, windowed_reported) =
        (sum(|d| d.windowed_scatter), sum(|d| d.windowed_reported));
    let (fit_scatter, fit_reported) = (sum(|d| d.fit_scatter), sum(|d| d.fit_reported));
    for (source, scatter, reported) in [
        ("windowed", windowed_scatter, windowed_reported),
        ("fit", fit_scatter, fit_reported),
    ] {
        let ratio = reported / scatter;
        assert!(
            (ratio - 1.0).abs() <= 0.095,
            "{source}: reported/scatter {ratio}"
        );
    }
}
