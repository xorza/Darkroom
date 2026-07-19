use crate::stacking::star_detection::centroid::test_utils::{
    make_elliptical_star as make_elliptical_gaussian, make_moffat_star,
};
use crate::stacking::star_detection::centroid::tests::*;

/// Helper: run measure_star on a single-star image with given centroid method.
fn measure_single_star(
    pixels: &Buffer2<f32>,
    bg_value: f32,
    noise: f32,
    peak_pos: Vec2,
    centroid_method: CentroidMethod,
) -> Star {
    let width = pixels.width();
    let height = pixels.height();
    let bg = make_uniform_background(width, height, bg_value, noise);
    let config = MeasurementConfig {
        centroid_method,
        ..Default::default()
    };
    let region = Region {
        bbox: URect::new(
            Vec2us::new(
                (peak_pos.x as usize).saturating_sub(5),
                (peak_pos.y as usize).saturating_sub(5),
            ),
            Vec2us::new(
                (peak_pos.x as usize + 6).min(width),
                (peak_pos.y as usize + 6).min(height),
            ),
        ),
        peak: Vec2us::new(peak_pos.x.round() as usize, peak_pos.y.round() as usize),
        peak_value: pixels[peak_pos.y.round() as usize * width + peak_pos.x.round() as usize],
        area: 50,
    };
    measure_star(
        pixels,
        &bg,
        &region,
        &config,
        FwhmConfig::default().expected,
    )
    .expect("measure_star should succeed")
}

/// GaussianFit: FWHM should come from fit sigma, not moments.
///
/// Circular Gaussian with sigma=2.0. True FWHM = 2.35482 * 2.0 = 4.70964.
/// The fit recovers sigma accurately; moments are biased by the finite stamp.
#[test]
fn test_gaussian_fit_fwhm_from_fit_params() {
    let sigma = 2.0f32;
    // True FWHM = FWHM_TO_SIGMA * sigma = 2.35482 * 2.0 = 4.70964
    let true_fwhm = FWHM_TO_SIGMA * sigma;

    let pos = Vec2::new(64.0, 64.0);
    let pixels = make_elliptical_gaussian(128, 128, pos, sigma, sigma, 0.8, 0.1);

    let star = measure_single_star(&pixels, 0.1, 0.01, pos, CentroidMethod::GaussianFit);

    // Fit-derived FWHM should be close to the true value
    let fwhm_error = (star.fwhm - true_fwhm).abs();
    assert!(
        fwhm_error < 0.05,
        "GaussianFit FWHM should match true value: got {}, expected {}, error {}",
        star.fwhm,
        true_fwhm,
        fwhm_error
    );
}

/// GaussianFit: eccentricity should come from fit sigma_x/sigma_y ratio.
///
/// Elongated Gaussian with sigma_x=2.0, sigma_y=4.0.
/// True eccentricity = sqrt(1 - (sigma_min/sigma_max)^2) = sqrt(1 - (2/4)^2) = sqrt(0.75) ≈ 0.8660.
#[test]
fn test_gaussian_fit_eccentricity_from_fit_params() {
    let sigma_x = 2.0f32;
    let sigma_y = 4.0f32;
    // e = sqrt(1 - (min/max)^2) = sqrt(1 - (2/4)^2) = sqrt(0.75) = 0.8660
    let true_ecc = (1.0 - (sigma_x / sigma_y).powi(2)).sqrt();

    let pos = Vec2::new(64.0, 64.0);
    let pixels = make_elliptical_gaussian(128, 128, pos, sigma_x, sigma_y, 0.8, 0.1);

    let star = measure_single_star(&pixels, 0.1, 0.01, pos, CentroidMethod::GaussianFit);

    let ecc_error = (star.eccentricity - true_ecc).abs();
    assert!(
        ecc_error < 0.05,
        "GaussianFit eccentricity should match true value: got {}, expected {}, error {}",
        star.eccentricity,
        true_ecc,
        ecc_error
    );
}

/// GaussianFit: FWHM from fit is more accurate than from moments.
///
/// Moments-based FWHM is biased because:
/// 1. Finite stamp includes wings that bias sum_r2 upward
/// 2. Background subtraction imperfections
///
/// The fit models the Gaussian directly, recovering sigma more accurately.
#[test]
fn test_gaussian_fit_fwhm_more_accurate_than_moments() {
    let sigma = 2.5f32;
    let true_fwhm = FWHM_TO_SIGMA * sigma;

    let pos = Vec2::new(64.0, 64.0);
    let pixels = make_elliptical_gaussian(128, 128, pos, sigma, sigma, 0.8, 0.1);

    let fit_star = measure_single_star(&pixels, 0.1, 0.01, pos, CentroidMethod::GaussianFit);
    let moments_star =
        measure_single_star(&pixels, 0.1, 0.01, pos, CentroidMethod::WeightedMoments);

    let fit_error = (fit_star.fwhm - true_fwhm).abs();
    let moments_error = (moments_star.fwhm - true_fwhm).abs();

    assert!(
        fit_error < moments_error,
        "GaussianFit FWHM error ({}) should be smaller than moments error ({}). \
         fit_fwhm={}, moments_fwhm={}, true_fwhm={}",
        fit_error,
        moments_error,
        fit_star.fwhm,
        moments_star.fwhm,
        true_fwhm
    );
}

/// MoffatFit: FWHM should come from alpha_beta_to_fwhm(), not moments.
///
/// Moffat star with alpha=3.0, beta=2.5.
/// True FWHM = 2 * alpha * sqrt(2^(1/beta) - 1)
///           = 2 * 3.0 * sqrt(2^0.4 - 1)
///           = 6.0 * sqrt(1.31951 - 1)
///           = 6.0 * sqrt(0.31951)
///           = 6.0 * 0.56525 ≈ 3.3915
///
/// Moments-based FWHM is severely biased for Moffat profiles because
/// the extended wings contribute disproportionately to sum_r2.
#[test]
fn test_moffat_fit_fwhm_from_fit_params() {
    let alpha = 3.0f32;
    let beta = 2.5f32;
    let true_fwhm = alpha_beta_to_fwhm(alpha, beta);
    // Verify: 2 * 3.0 * sqrt(2^0.4 - 1) ≈ 3.3915
    assert!(
        (true_fwhm - 3.3915).abs() < 0.001,
        "FWHM formula check: {}",
        true_fwhm
    );

    let pos = Vec2::new(64.0, 64.0);
    let pixels = make_moffat_star(128, 128, pos, alpha, beta, 0.8, 0.1);

    let star = measure_single_star(&pixels, 0.1, 0.01, pos, CentroidMethod::MoffatFit { beta });

    let fwhm_error = (star.fwhm - true_fwhm).abs();
    assert!(
        fwhm_error < 0.15,
        "MoffatFit FWHM should match true value: got {}, expected {}, error {}",
        star.fwhm,
        true_fwhm,
        fwhm_error
    );
}

/// MoffatFit: eccentricity stays moment-based (Moffat is circular).
///
/// For a circular Moffat profile, eccentricity should be near zero
/// regardless of whether it comes from fit or moments.
#[test]
fn test_moffat_fit_eccentricity_stays_moment_based() {
    let alpha = 3.0f32;
    let beta = 2.5f32;
    let pos = Vec2::new(64.0, 64.0);
    let pixels = make_moffat_star(128, 128, pos, alpha, beta, 0.8, 0.1);

    let star = measure_single_star(&pixels, 0.1, 0.01, pos, CentroidMethod::MoffatFit { beta });

    // Circular source → low eccentricity
    assert!(
        star.eccentricity < 0.15,
        "Circular Moffat should have low eccentricity: got {}",
        star.eccentricity
    );
}

/// WeightedMoments: FWHM should be unchanged (no regression).
///
/// Circular Gaussian with sigma=2.5. True FWHM = 2.35482 * 2.5 = 5.887.
/// Moments-based FWHM is biased upward by finite stamp size — the pre-existing
/// behavior should be preserved exactly.
#[test]
fn test_moments_only_fwhm_unchanged() {
    let sigma = 2.5f32;
    let true_fwhm = FWHM_TO_SIGMA * sigma;

    let pos = Vec2::new(64.0, 64.0);
    let pixels = make_elliptical_gaussian(128, 128, pos, sigma, sigma, 0.8, 0.1);

    let star = measure_single_star(&pixels, 0.1, 0.01, pos, CentroidMethod::WeightedMoments);

    // Moments-based FWHM has some bias from finite stamp and background subtraction.
    // Verify it's reasonable (within 5% of true value).
    let rel_error = (star.fwhm - true_fwhm).abs() / true_fwhm;
    assert!(
        rel_error < 0.05,
        "WeightedMoments FWHM should be within 5% of true: got {}, true {}, rel_error {:.4}",
        star.fwhm,
        true_fwhm,
        rel_error
    );
}

/// MoffatFit: FWHM from fit is more accurate than moments for Moffat profiles.
///
/// Moffat profiles have heavy wings that heavily bias moment-based FWHM upward.
/// The fit directly recovers alpha, giving accurate FWHM.
#[test]
fn test_moffat_fit_fwhm_more_accurate_than_moments() {
    let alpha = 3.0f32;
    let beta = 2.5f32;
    let true_fwhm = alpha_beta_to_fwhm(alpha, beta);

    let pos = Vec2::new(64.0, 64.0);
    let pixels = make_moffat_star(128, 128, pos, alpha, beta, 0.8, 0.1);

    let fit_star = measure_single_star(&pixels, 0.1, 0.01, pos, CentroidMethod::MoffatFit { beta });
    let moments_star =
        measure_single_star(&pixels, 0.1, 0.01, pos, CentroidMethod::WeightedMoments);

    let fit_error = (fit_star.fwhm - true_fwhm).abs();
    let moments_error = (moments_star.fwhm - true_fwhm).abs();

    assert!(
        fit_error < moments_error,
        "MoffatFit FWHM error ({}) should be smaller than moments error ({}). \
         fit_fwhm={}, moments_fwhm={}, true_fwhm={}",
        fit_error,
        moments_error,
        fit_star.fwhm,
        moments_star.fwhm,
        true_fwhm
    );
}

#[test]
fn windowed_covariance_recovers_gaussian_sigma() {
    // A clean round Gaussian of σ=2.5: the window deconvolution must recover σ²
    // on both axes (unbiased FWHM) with a ~zero cross term (round → ecc ≈ 0).
    let (width, height) = (64, 64);
    let pos = Vec2::new(32.0, 32.0);
    let sigma = 2.5f32;
    let pixels = make_gaussian_star(width, height, pos, sigma, 1.0, 0.0);
    let bg = background_map::uniform(width, height, 0.0, 1.0);

    let cov = windowed_covariance(&pixels, &bg, None, pos, 12, (sigma * sigma) as f64)
        .expect("clean Gaussian should converge");

    let expected = (sigma * sigma) as f64; // σ² per axis
    assert!(
        (cov.xx - expected).abs() < 0.1 * expected,
        "cxx {} vs expected {expected}",
        cov.xx
    );
    assert!(
        (cov.yy - expected).abs() < 0.1 * expected,
        "cyy {} vs expected {expected}",
        cov.yy
    );
    assert!(
        cov.xy.abs() < 0.05 * expected,
        "cxy {} should be ~0",
        cov.xy
    );
}

#[test]
fn windowed_covariance_recovers_elliptical_axes() {
    // An elliptical Gaussian (σx=3, σy=2): a circular window is isotropic, so its
    // deconvolution recovers both axis variances exactly — the measurement must
    // not circularize the source (otherwise eccentricity would be lost).
    let (width, height) = (64, 64);
    let pos = Vec2::new(32.0, 32.0);
    let (sx, sy) = (3.0f32, 2.0f32);
    let pixels = make_elliptical_star(width, height, pos, sx, sy, 1.0, 0.0);
    let bg = background_map::uniform(width, height, 0.0, 1.0);

    let seed = ((sx * sx + sy * sy) / 2.0) as f64;
    let cov = windowed_covariance(&pixels, &bg, None, pos, 14, seed)
        .expect("clean elliptical Gaussian should converge");

    assert!(
        (cov.xx - (sx * sx) as f64).abs() < 0.12 * (sx * sx) as f64,
        "cxx {} vs {}",
        cov.xx,
        sx * sx
    );
    assert!(
        (cov.yy - (sy * sy) as f64).abs() < 0.12 * (sy * sy) as f64,
        "cyy {} vs {}",
        cov.yy,
        sy * sy
    );
    // Recovered axis ratio tracks the input (not washed toward 1).
    let ratio = (cov.yy / cov.xx).sqrt();
    let expected_ratio = (sy / sx) as f64;
    assert!(
        (ratio - expected_ratio).abs() < 0.08,
        "axis ratio {ratio} vs expected {expected_ratio}"
    );
}

#[test]
fn windowed_covariance_resists_wing_noise() {
    // The PR2 failure mode: signed moments over a fixed stamp sum in far-wing
    // noise, which inflates eccentricity for round stars. The window must suppress
    // it — a round noisy star stays ~circular.
    let (width, height) = (64, 64);
    let pos = Vec2::new(32.0, 32.0);
    let sigma = 2.5f32;
    let mut pixels = make_gaussian_star(width, height, pos, sigma, 1.0, 0.1);
    add_noise(pixels.pixels_mut(), 0.03, 12345);
    let bg = background_map::uniform(width, height, 0.1, 1.0);

    let cov = windowed_covariance(&pixels, &bg, None, pos, 12, (sigma * sigma) as f64)
        .expect("noisy Gaussian should still converge");

    let ratio = (cov.yy / cov.xx).sqrt();
    assert!(
        (0.85..1.18).contains(&ratio),
        "axis ratio {ratio} should stay ~1 under noise (no inflation)"
    );
}

#[test]
fn inverse_variance_weights_downweight_bright_pixels() {
    // CCD per-pixel variance = signal/G + sky² + (read_e/G)², G = e-/normalized unit.
    let bg = 0.1;
    let sky_noise = 0.02; // sky_var = 4e-4
    let noise_model = NoiseModel::from_normalized(1_000.0, 10.0);
    let data_z = [0.1, 0.6, 1.1]; // signals 0.0, 0.5, 1.0

    let w = inverse_variance_weights(&data_z, bg, sky_noise, noise_model);

    // signal 0.0: 1/(0      + 4e-4 + 1e-4) = 2000
    // signal 0.5: 1/(5e-4   + 5e-4)        = 1000
    // signal 1.0: 1/(1e-3   + 5e-4)        ≈ 666.67
    assert!((w[0] - 2000.0).abs() < 1e-9, "w0 = {}", w[0]);
    assert!((w[1] - 1000.0).abs() < 1e-9, "w1 = {}", w[1]);
    assert!((w[2] - 666.666_666_666_666_6).abs() < 1e-9, "w2 = {}", w[2]);
    assert!(
        w[0] > w[1] && w[1] > w[2],
        "weight must fall as signal rises"
    );
}
