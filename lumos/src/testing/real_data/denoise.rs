//! Real-data denoising: load the bundled stacked light frame, neutralize its background, denoise in
//! the linear domain, then stretch and SCNR into a viewable image — saving only the final result.
//! Gated behind the `real-data` feature.

use crate::image_ops::test_support::channel_plane;
use crate::io::image::linear::LinearImage;
use crate::io::image::LoadContext;
use crate::math::statistics::{mad_f32_with_scratch, mad_to_sigma, median_f32_mut};
use crate::testing::{calibration_dir, init_tracing, save_png};
use crate::{Denoise, NeutralizeBackground, Scnr, Stretch};
use imaginarium::Image;

/// Robust high-frequency noise of a channel: the MAD-sigma of adjacent-pixel differences. Slow
/// gradients and extended signal cancel in the difference, so this isolates the pixel-scale noise
/// that denoising removes (unlike a global sigma, which is dominated by real structure).
fn highfreq_noise(image: &Image, channel: usize) -> f32 {
    let buf = channel_plane(image, channel);
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
    let median = median_f32_mut(&mut diffs);
    let mut scratch = Vec::new();
    mad_to_sigma(mad_f32_with_scratch(&diffs, median, &mut scratch))
}

#[test]
#[ignore = "real-data image-processing test; run explicitly with --ignored"]
fn denoise_reduces_linear_noise() {
    init_tracing();

    let mut img = Image::from(
        &LinearImage::from_file(
            calibration_dir().join("stacked_light.tiff"),
            &LoadContext::default(),
        )
        .expect("load"),
    );

    // Neutralize the background first so denoising runs on color-calibrated linear data.
    NeutralizeBackground.apply(&mut img).unwrap();

    // Measure pixel-scale noise per channel before and after denoising (both in the linear domain).
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

    // Finish the display chain — stretch then clean any residual green — and save the final image.
    Stretch::auto_stf().apply(&mut img).unwrap();
    Scnr::average_neutral().apply(&mut img).unwrap();
    save_png(&img, "denoise/stacked_light_denoised.png");
}
