//! Real-data denoising: load the bundled stacked light frame, neutralize its background, denoise in
//! the linear domain, then stretch and SCNR into a viewable image — saving only the final result.
//! Gated behind the `real-data` feature.

use crate::image::linear_image::LinearImage;
use crate::image_ops::color_calibration::neutralize_bg::NeutralizeBg;
use crate::image_ops::color_calibration::scnr::Scnr;
use crate::image_ops::denoise::Denoise;
use crate::image_ops::stretch::Stretch;
use crate::internals::{init_tracing, real_data, visual};
use crate::math::statistics::median_mad::MedianMad;

/// Robust high-frequency noise of a channel: the MAD-sigma of adjacent-pixel differences. Slow
/// gradients and extended signal cancel in the difference, so this isolates the pixel-scale noise
/// that denoising removes (unlike a global sigma, which is dominated by real structure).
fn highfreq_noise(image: &LinearImage, channel: usize) -> f32 {
    let buf = image.channel(channel).clone();
    let width = buf.width();
    let px = buf.pixels();
    let n = px.len();
    let stride = (n / 500_000).max(1);
    // `i % width != width - 1` keeps the difference within a row (no wrap), which also guarantees
    // `i + 1 < n` since the final pixel is always a last-column pixel.
    let mut diffs: Vec<f32> = (0..n)
        .step_by(stride)
        .filter(|&i| i % width != width - 1)
        .map(|i| px[i + 1] - px[i])
        .collect();
    MedianMad::of_mut(&mut diffs).sigma()
}

#[test]
fn denoise_reduces_linear_noise() {
    init_tracing();

    let mut img = real_data::linear_master();

    // Neutralize the background first so denoising runs on color-calibrated linear data.
    NeutralizeBg.apply(&mut img).unwrap();

    let before: Vec<f32> = (0..3).map(|c| highfreq_noise(&img, c)).collect();
    Denoise::default().apply(&mut img).unwrap();
    let after: Vec<f32> = (0..3).map(|c| highfreq_noise(&img, c)).collect();

    for c in 0..3 {
        eprintln!(
            "channel {c} high-frequency noise: {:.6} -> {:.6}  ({:.0}% reduction)",
            before[c],
            after[c],
            100.0 * (1.0 - after[c] / before[c])
        );
        assert!(
            after[c] < 0.85 * before[c],
            "denoise reduced channel {c} pixel noise (before {:.6}, after {:.6})",
            before[c],
            after[c]
        );
    }

    Stretch::auto_stf().apply(&mut img).unwrap();
    Scnr::average_neutral(1.0).apply(&mut img).unwrap();
    visual::save_linear(&img, "denoise/stacked_light_denoised");
}
