//! Real-data denoising: load the bundled stacked light frame, neutralize its background, denoise in
//! the linear domain, then stretch and SCNR into a viewable image — saving only the final result.
//! Gated behind the `real-data` feature.

use crate::color_calibration::{ScnrMethod, neutralize_background_planar, scnr_planar};
use crate::denoise::{DenoiseConfig, denoise_planar};
use crate::math::statistics::{mad_f32_with_scratch, mad_to_sigma, median_f32_mut};
use crate::stretching::stretch_planar;
use crate::testing::{calibration_dir, init_tracing, save_png};
use crate::{AstroImage, StretchConfig};

/// Robust high-frequency noise of a channel: the MAD-sigma of adjacent-pixel differences. Slow
/// gradients and extended signal cancel in the difference, so this isolates the pixel-scale noise
/// that denoising removes (unlike a global sigma, which is dominated by real structure).
fn highfreq_noise(image: &AstroImage, channel: usize) -> f32 {
    let buf = image.channel(channel);
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
#[cfg_attr(not(feature = "real-data"), ignore)]
fn denoise_reduces_linear_noise() {
    init_tracing();

    let mut img =
        AstroImage::from_file(calibration_dir().join("stacked_light.tiff")).expect("load");

    // Neutralize the background first so denoising runs on color-calibrated linear data.
    neutralize_background_planar(&mut img);

    // Measure pixel-scale noise per channel before and after denoising (both in the linear domain).
    let before: Vec<f32> = (0..3).map(|c| highfreq_noise(&img, c)).collect();
    denoise_planar(&mut img, DenoiseConfig::default());
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
    stretch_planar(&mut img, StretchConfig::auto_stf());
    scnr_planar(&mut img, ScnrMethod::AverageNeutral);
    save_png(&img, "denoise/stacked_light_denoised.png");
}
