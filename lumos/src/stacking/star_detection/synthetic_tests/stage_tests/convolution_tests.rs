//! Convolution/filtering stage tests.
//!
//! Tests the Gaussian filtering for star enhancement.

use crate::math::fwhm_to_sigma;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::convolution::gaussian_convolve;
use crate::stacking::star_detection::synthetic_tests::Scenario;
use crate::stacking::star_detection::test_common::output::image_writer::save_grayscale;
use crate::testing::{estimate_background, init_tracing};
use common::test_utils::test_output_path;
use imaginarium::Buffer2;

use crate::stacking::star_detection::synthetic_tests::stage_tests::TILE_SIZE;

/// Normalize filtered output for visualization (handle negative values).
fn normalize_for_display(pixels: &[f32]) -> Vec<f32> {
    let min_val = pixels.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_val = pixels.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = (max_val - min_val).max(1e-10);

    pixels.iter().map(|&p| (p - min_val) / range).collect()
}

/// Test Gaussian filter response on sparse star field.
#[test]

fn test_gaussian_filter_sparse() {
    init_tracing();

    let width = 256;
    let height = 256;
    let fwhm = 3.5;

    // Sparse star field.
    let frame = Scenario {
        num_stars: 25,
        fwhm,
        ..Default::default()
    }
    .frame();
    let pixels = frame.image.channel(0).clone();
    let ground_truth = frame.truth.sources.clone();

    // Save input
    save_grayscale(
        pixels.pixels(),
        width,
        height,
        &test_output_path("synthetic_starfield/stage_conv_sparse_input.png"),
    );

    // Estimate and subtract background
    let background = estimate_background(
        &pixels,
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    let bg_subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();

    save_grayscale(
        &bg_subtracted,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_conv_sparse_bg_subtracted.png"),
    );

    // Apply Gaussian filter (matched filter for star detection)
    let sigma = fwhm_to_sigma(fwhm);
    let bg_subtracted_buf = Buffer2::new(width, height, bg_subtracted);
    let mut filtered = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    gaussian_convolve(&bg_subtracted_buf, sigma, &mut filtered, &mut temp);

    // Normalize for display
    let filtered_display = normalize_for_display(filtered.pixels());

    save_grayscale(
        &filtered_display,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_conv_sparse_filtered.png"),
    );

    // Verify stars are enhanced
    println!("Ground truth stars: {}", ground_truth.len());
    println!("Sigma: {:.2}", sigma);

    // Check that star positions have high filtered values
    let mut star_responses: Vec<f32> = Vec::new();
    for star in &ground_truth {
        let x = star.pos.x.round() as usize;
        let y = star.pos.y.round() as usize;
        if x > 10 && x < width - 10 && y > 10 && y < height - 10 {
            let response = filtered[y * width + x];
            star_responses.push(response);
        }
    }

    let mean_star_response: f32 =
        star_responses.iter().sum::<f32>() / star_responses.len().max(1) as f32;

    // Robust noise floor of the filtered image: stars are sparse, so the median and MAD
    // describe the star-free background (not the near-tautological whole-image mean).
    let (median, robust_sigma) = robust_floor(filtered.pixels());
    let min_star_response = star_responses.iter().cloned().fold(f32::INFINITY, f32::min);

    println!("Mean star response: {mean_star_response:.6}");
    println!("Filtered floor: median {median:.6}, sigma {robust_sigma:.6}");

    // Matched filtering must lift every detected star far above the noise floor.
    assert!(
        min_star_response > median + 5.0 * robust_sigma,
        "faintest star response {min_star_response:.6} should clear floor {median:.6} + 5σ ({:.6})",
        5.0 * robust_sigma
    );
}

/// Median and MAD-derived σ of a slice (robust background statistics).
fn robust_floor(pixels: &[f32]) -> (f32, f32) {
    let mut sorted: Vec<f32> = pixels.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    let mut dev: Vec<f32> = sorted.iter().map(|&v| (v - median).abs()).collect();
    dev.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (median, dev[dev.len() / 2] * 1.4826)
}

/// Test Gaussian filter with different FWHM values.
#[test]

fn test_gaussian_filter_fwhm_range() {
    init_tracing();

    let width = 256;
    let height = 256;

    // Stars at the instrument FWHM (legacy varied per-star).
    let frame = Scenario {
        num_stars: 30,
        ..Default::default()
    }
    .frame();
    let pixels = frame.image.channel(0).clone();
    let ground_truth = frame.truth.sources.clone();

    // Save input
    save_grayscale(
        pixels.pixels(),
        width,
        height,
        &test_output_path("synthetic_starfield/stage_conv_fwhm_range_input.png"),
    );

    // Background subtraction
    let background = estimate_background(
        &pixels,
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    let bg_subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();

    // The matched-filter property: a Gaussian kernel matched to the source FWHM (the Scenario
    // default, 4.0) maximises detection *SNR* — not raw peak response, which rises monotonically
    // for narrower kernels. SNR = mean star response above the filtered noise floor, in σ.
    let snr = |target_fwhm: f32| -> f32 {
        let sigma = fwhm_to_sigma(target_fwhm);
        let bg_buf = Buffer2::new(width, height, bg_subtracted.clone());
        let mut filtered = Buffer2::new_default(width, height);
        let mut temp = Buffer2::new_default(width, height);
        gaussian_convolve(&bg_buf, sigma, &mut filtered, &mut temp);
        let responses: Vec<f32> = ground_truth
            .iter()
            .filter_map(|s| {
                let (x, y) = (s.pos.x.round() as usize, s.pos.y.round() as usize);
                (x > 10 && x < width - 10 && y > 10 && y < height - 10)
                    .then(|| filtered[y * width + x])
            })
            .collect();
        let mean_resp = responses.iter().sum::<f32>() / responses.len() as f32;
        let (median, robust_sigma) = robust_floor(filtered.pixels());
        (mean_resp - median) / robust_sigma
    };

    let narrow = snr(2.5);
    let matched = snr(4.0);
    let wide = snr(5.5);
    println!("matched-filter SNR — narrow {narrow:.2}, matched {matched:.2}, wide {wide:.2}");

    assert!(
        matched > narrow && matched > wide,
        "kernel matched to source FWHM should maximise SNR: narrow {narrow:.2}, matched {matched:.2}, wide {wide:.2}"
    );
}

/// Test Gaussian filter noise rejection.
#[test]

fn test_gaussian_filter_noise() {
    init_tracing();

    let width = 256;
    let height = 256;
    let fwhm = 3.5;

    // Star field with elevated noise (shallow well + high read noise).
    let frame = Scenario {
        num_stars: 20,
        fwhm,
        full_well_e: 4_000.0,
        read_noise_e: 150.0,
        ..Default::default()
    }
    .frame();
    let pixels = frame.image.channel(0).clone();
    let ground_truth = frame.truth.sources.clone();

    // Save input
    save_grayscale(
        pixels.pixels(),
        width,
        height,
        &test_output_path("synthetic_starfield/stage_conv_noise_input.png"),
    );

    // Background subtraction
    let background = estimate_background(
        &pixels,
        &Config {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    );

    let bg_subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.background.iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();

    save_grayscale(
        &bg_subtracted,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_conv_noise_bg_subtracted.png"),
    );

    // Apply Gaussian filter
    let sigma = fwhm_to_sigma(fwhm);
    let bg_subtracted_buf = Buffer2::new(width, height, bg_subtracted);
    let mut filtered = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    gaussian_convolve(&bg_subtracted_buf, sigma, &mut filtered, &mut temp);
    let filtered_display = normalize_for_display(filtered.pixels());

    save_grayscale(
        &filtered_display,
        width,
        height,
        &test_output_path("synthetic_starfield/stage_conv_noise_filtered.png"),
    );

    // Check star detectability even with noise
    let mut star_responses: Vec<f32> = Vec::new();
    for star in &ground_truth {
        let x = star.pos.x.round() as usize;
        let y = star.pos.y.round() as usize;
        if x > 10 && x < width - 10 && y > 10 && y < height - 10 {
            let response = filtered[y * width + x];
            star_responses.push(response);
        }
    }

    let mean_star_response: f32 =
        star_responses.iter().sum::<f32>() / star_responses.len().max(1) as f32;

    // Robust noise of the filtered image: true MAD-derived σ (not the mean-abs-dev this used
    // to mislabel as MAD).
    let (median, robust_sigma) = robust_floor(filtered.pixels());
    let snr = (mean_star_response - median) / robust_sigma;
    println!(
        "Mean star response {mean_star_response:.6}, floor {median:.6} ± {robust_sigma:.6}, SNR {snr:.1}"
    );

    // Even with a shallow well + high read noise, matched-filtered stars stay detectable.
    assert!(snr > 3.0, "filtered star SNR {snr:.1} should exceed 3");
}
