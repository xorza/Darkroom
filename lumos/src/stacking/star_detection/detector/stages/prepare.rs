//! Image preparation stage: reduce to a single detection plane + CFA filter.

use rayon::prelude::*;

use crate::AstroImage;
use crate::math::statistics::{mad_to_sigma, median_and_mad_f32_mut};
use crate::stacking::star_detection::buffer_pool::BufferPool;
use crate::stacking::star_detection::median_filter::median_filter_3x3;
use imaginarium::Buffer2;

/// Reduce an input image to a single-channel detection plane, applying CFA
/// median filtering if needed.
///
/// Steps:
///   1. Reduce to one plane: copy for grayscale, or an inverse-variance
///      (noise-weighted) channel combination for RGB (see `detection_channel_weights`).
///   2. 3×3 median filter to suppress Bayer/X-Trans artifacts (if CFA).
///
/// The returned buffer is acquired from `pool`; the caller owns it.
pub(crate) fn prepare(image: &AstroImage, pool: &mut BufferPool) -> Buffer2<f32> {
    let mut pixels = pool.acquire_f32();

    if image.is_grayscale() {
        pixels
            .pixels_mut()
            .copy_from_slice(image.channel(0).pixels());
    } else {
        let mut scratch = [pool.acquire_f32(), pool.acquire_f32(), pool.acquire_f32()];
        let weights = detection_channel_weights(image, &mut scratch);
        combine_channels(image, weights, &mut pixels);
        for buf in scratch {
            pool.release_f32(buf);
        }
    }

    // CFA median filter
    if image.metadata.cfa_type.is_some() {
        let mut scratch = pool.acquire_f32();
        median_filter_3x3(&pixels, &mut scratch);
        std::mem::swap(&mut pixels, &mut scratch);
        pool.release_f32(scratch);
    }

    pixels
}

/// Inverse-variance ("noise") weights for collapsing RGB into the detection plane.
///
/// Each channel is weighted by `1/σ²` — σ from the per-channel MAD, the same
/// noise convention stacking uses for `Weighting::Noise` — and the weights are
/// normalized to sum to 1. This is the optimal *linear* combiner for an unknown
/// (flat) source SED, i.e. the linear analogue of the SExtractor χ² detection
/// image. It is deliberately kept linear rather than a χ² sum-of-squares because
/// flux, centroid, FWHM, and SNR are all measured on this plane downstream, and
/// squaring would distort the PSF and break flux linearity.
///
/// Unlike Rec.709 luminance (a perceptual weighting that discards ~79% of red and
/// ~93% of blue signal), this never zeroes a band — it only down-weights noisier
/// ones — so red- and blue-dominant stars stay detectable.
///
/// The three per-channel median+MAD passes are independent and run concurrently,
/// each reusing one caller-supplied scratch buffer (so there is no per-call
/// allocation). Each scratch buffer must be one channel long; they are clobbered.
fn detection_channel_weights(image: &AstroImage, scratch: &mut [Buffer2<f32>; 3]) -> [f32; 3] {
    let mut inv_var = [0.0f32; 3];
    inv_var
        .as_mut_slice()
        .par_iter_mut()
        .zip(scratch.as_mut_slice().par_iter_mut())
        .enumerate()
        .for_each(|(c, (iv, buf))| {
            let dst = buf.pixels_mut();
            dst.copy_from_slice(image.channel(c).pixels());
            let (_median, mad) = median_and_mad_f32_mut(dst);
            let sigma = mad_to_sigma(mad);
            *iv = if sigma > f32::EPSILON {
                1.0 / (sigma * sigma)
            } else {
                0.0
            };
        });

    let sum: f32 = inv_var.iter().sum();
    if sum > f32::EPSILON {
        [inv_var[0] / sum, inv_var[1] / sum, inv_var[2] / sum]
    } else {
        // Every channel is flat (degenerate / synthetic) — fall back to the mean.
        [1.0 / 3.0; 3]
    }
}

/// Write `Σ wₖ·channelₖ` into `output` (RGB only).
fn combine_channels(image: &AstroImage, weights: [f32; 3], output: &mut Buffer2<f32>) {
    let r = image.channel(0).pixels();
    let g = image.channel(1).pixels();
    let b = image.channel(2).pixels();
    output
        .pixels_mut()
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, o)| {
            *o = weights[0] * r[i] + weights[1] * g[i] + weights[2] * b[i];
        });
}

#[cfg(test)]
mod tests {
    use crate::stacking::star_detection::detector::stages::prepare::*;
    use crate::{AstroImage, ImageDimensions};

    /// Build a 16-pixel channel whose median is `center` and MAD is exactly `mad`
    /// (8 pixels at `center - mad`, 8 at `center + mad`).
    fn channel_with_mad(center: f32, mad: f32) -> Vec<f32> {
        let mut v = vec![center - mad; 8];
        v.extend(vec![center + mad; 8]);
        v
    }

    #[test]
    fn test_prepare_uniform() {
        let dim = ImageDimensions::new((64, 64), 1);
        let data = vec![0.5f32; 64 * 64];
        let image = AstroImage::from_pixels(dim, data);

        let mut pool = BufferPool::new(64, 64);
        let result = prepare(&image, &mut pool);

        assert_eq!(result.width(), 64);
        assert_eq!(result.height(), 64);
        for &v in result.pixels() {
            assert!((v - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn test_prepare_with_star() {
        let width = 64;
        let height = 64;
        let mut data = vec![0.1f32; width * height];
        // Add bright pixel (simulating a star)
        data[32 * width + 32] = 0.9;

        let dim = ImageDimensions::new((width, height), 1);
        let image = AstroImage::from_pixels(dim, data);

        let mut pool = BufferPool::new(width, height);
        let result = prepare(&image, &mut pool);

        // Star pixel should be preserved (no CFA, no defects)
        assert!((result[(32, 32)] - 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_detection_weights_equal_noise() {
        // Identical per-channel MAD → equal inverse-variance weights (≈ 1/3 each).
        let dims = ImageDimensions::new((4, 4), 3);
        let ch = channel_with_mad(0.5, 0.02);
        let image = AstroImage::from_planar_channels(dims, vec![ch.clone(), ch.clone(), ch]);

        let mut scratch = [
            Buffer2::new_default(4, 4),
            Buffer2::new_default(4, 4),
            Buffer2::new_default(4, 4),
        ];
        let w = detection_channel_weights(&image, &mut scratch);

        for &wi in &w {
            assert!((wi - 1.0 / 3.0).abs() < 1e-4, "expected ~1/3, got {wi}");
        }
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_detection_weights_downweight_noisy_channel() {
        // R,G clean (MAD 0.02), B noisy (MAD 0.08). σ ∝ MAD, so w ∝ 1/MAD².
        // w_R / w_B = (0.08 / 0.02)² = 16.
        let dims = ImageDimensions::new((4, 4), 3);
        let clean = channel_with_mad(0.5, 0.02);
        let noisy = channel_with_mad(0.5, 0.08);
        let image = AstroImage::from_planar_channels(dims, vec![clean.clone(), clean, noisy]);

        let mut scratch = [
            Buffer2::new_default(4, 4),
            Buffer2::new_default(4, 4),
            Buffer2::new_default(4, 4),
        ];
        let w = detection_channel_weights(&image, &mut scratch);

        assert!((w[0] - w[1]).abs() < 1e-5, "R and G have equal noise");
        let ratio = w[0] / w[2];
        assert!(
            (ratio - 16.0).abs() < 0.05,
            "w_R/w_B should be ~16, got {ratio}"
        );
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_detection_weights_all_flat_falls_back_to_mean() {
        // Uniform channels have MAD 0 → degenerate; weights fall back to 1/3 each.
        let dims = ImageDimensions::new((4, 4), 3);
        let flat = vec![0.5f32; 16];
        let image = AstroImage::from_planar_channels(dims, vec![flat.clone(), flat.clone(), flat]);

        let mut scratch = [
            Buffer2::new_default(4, 4),
            Buffer2::new_default(4, 4),
            Buffer2::new_default(4, 4),
        ];
        let w = detection_channel_weights(&image, &mut scratch);

        assert_eq!(w, [1.0 / 3.0; 3]);
    }

    #[test]
    fn test_prepare_rgb_equal_noise_is_mean() {
        // Distinct per-channel levels but identical spread → equal weights, so the
        // detection plane is the plain mean of the three channels.
        let dims = ImageDimensions::new((4, 4), 3);
        let r = channel_with_mad(0.30, 0.02);
        let g = channel_with_mad(0.50, 0.02);
        let b = channel_with_mad(0.70, 0.02);
        let image = AstroImage::from_planar_channels(dims, vec![r.clone(), g.clone(), b.clone()]);

        let mut pool = BufferPool::new(4, 4);
        let out = prepare(&image, &mut pool);

        for (i, &out_v) in out.pixels().iter().enumerate() {
            let expected = (r[i] + g[i] + b[i]) / 3.0;
            assert!(
                (out_v - expected).abs() < 1e-4,
                "pixel {i}: expected mean {expected}, got {out_v}"
            );
        }
    }

    #[test]
    fn test_prepare_rgb_red_star_survives() {
        // A star bright only in R must remain prominent in the detection plane.
        // With equal-noise channels the weights are ~1/3, so the star peak lands at
        // ~1/3 of its R amplitude — far above Rec.709's 0.21× crush of red.
        let dims = ImageDimensions::new((4, 4), 3);
        let mut r = channel_with_mad(0.10, 0.01);
        r[5] = 0.90; // bright red star, off the symmetric background
        let g = channel_with_mad(0.10, 0.01);
        let b = channel_with_mad(0.10, 0.01);
        let image = AstroImage::from_planar_channels(dims, vec![r, g, b]);

        let mut pool = BufferPool::new(4, 4);
        let out = prepare(&image, &mut pool);

        // Background ~0.10; star pixel should be well above it (~0.10*2/3 + 0.90/3 ≈ 0.37).
        assert!(out[5] > 0.30, "red star should survive, got {}", out[5]);
    }
}
