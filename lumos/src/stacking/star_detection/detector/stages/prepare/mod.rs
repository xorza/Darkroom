//! Image preparation stage: reduce to a single detection plane + CFA filter.

use rayon::prelude::*;

use crate::io::image::linear::LinearImage;
use crate::math::statistics::MedianMad;
use crate::stacking::star_detection::median_filter::median_filter_3x3;
use crate::stacking::star_detection::resources::DetectionResources;
use imaginarium::Buffer2;

/// Reduce an input image to a single-channel detection plane, applying CFA
/// median filtering if needed.
///
/// Steps:
///   1. Reduce to one plane: copy for grayscale, or an inverse-variance
///      (noise-weighted) channel combination for RGB (see `detection_channel_weights`).
///   2. 3×3 median filter to suppress demosaic interpolation artifacts (if interpolated).
///
/// The returned buffer is acquired from `pool`; the caller owns it.
pub(crate) fn prepare(image: &LinearImage, pool: &mut DetectionResources) -> Buffer2<f32> {
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

    // Only interpolated frames have the artifacts to suppress; a monochrome sensor's plane is
    // measured, and filtering it would blur the PSF that FWHM and flux are read off.
    if image.metadata.is_demosaiced() {
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
fn detection_channel_weights(image: &LinearImage, scratch: &mut [Buffer2<f32>; 3]) -> [f32; 3] {
    let mut inv_var = [0.0f32; 3];
    inv_var
        .as_mut_slice()
        .par_iter_mut()
        .zip(scratch.as_mut_slice().par_iter_mut())
        .enumerate()
        .for_each(|(c, (iv, buf))| {
            let dst = buf.pixels_mut();
            dst.copy_from_slice(image.channel(c).pixels());
            let sigma = MedianMad::of_mut(dst).sigma();
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
fn combine_channels(image: &LinearImage, weights: [f32; 3], output: &mut Buffer2<f32>) {
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
mod tests;
