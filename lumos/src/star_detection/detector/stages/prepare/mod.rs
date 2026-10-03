//! Image preparation stage: reduce to a single detection plane + CFA filter.

use rayon::prelude::*;

use crate::io::image::linear::LinearImage;
use crate::io::image::pixel_flags::Flags;
use crate::math::noise::mrs_noise::MrsNoise;
use crate::math::size2us::Size2us;
use crate::star_detection::median_filter::median_filter_3x3;
use crate::star_detection::resources::DetectionResources;
use imaginarium::Buffer2;
use std::mem;

/// Reduce an input image to a single-channel detection plane, applying CFA
/// median filtering if needed.
///
/// Steps:
///   1. Reduce to one plane: copy for grayscale, or an inverse-variance
///      (noise-weighted) channel combination for RGB (see `inverse_variance_weights`).
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
        let weights = inverse_variance_weights(channel_noise(image));
        combine_channels(image, weights, &mut pixels);
    }

    // Only interpolated frames have the artifacts to suppress; a monochrome sensor's plane is
    // measured, and filtering it would blur the PSF that FWHM and flux are read off.
    if image.metadata.is_demosaiced() {
        let mut scratch = pool.acquire_f32();
        median_filter_3x3(&pixels, &mut scratch);
        mem::swap(&mut pixels, &mut scratch);
        pool.release_f32(scratch);
    }

    pixels
}

/// Each channel's white noise, by the multiresolution estimator over the pixels no flag names: the
/// noise stacking weighs by, and not the MAD, which a red nebula inflates in red.
fn channel_noise(image: &LinearImage) -> [f32; 3] {
    let flags = image.flags.as_ref();
    let excluded = |index: usize| flags.is_some_and(|flags| flags.at(index) != Flags::default());
    let size = Size2us::new(image.width(), image.height());
    [0, 1, 2].map(|channel| MrsNoise::estimate(image.channel(channel).pixels(), size, excluded))
}

/// Inverse-variance weights for collapsing RGB into the detection plane, summing to 1.
///
/// This is the optimal *linear* combiner for an unknown (flat) source SED, the linear analogue of
/// the SExtractor χ² detection image. It is kept linear rather than a χ² sum of squares because
/// flux, centroid, FWHM and SNR are measured on this plane downstream, and squaring would distort
/// the PSF and break flux linearity. Unlike Rec.709 luminance, it never zeroes a band, so red- and
/// blue-dominant stars stay detectable.
///
/// Each weight is `(σ_min/σ)²` before the sum, which needs no `1/σ²` that a tiny σ could overflow.
/// A channel with no measured noise is better than any with noise, so the channels at σ = 0 share
/// the whole weight: the limit of `1/σ²`. Only synthetic data has one.
fn inverse_variance_weights(sigmas: [f32; 3]) -> [f32; 3] {
    let quiet = sigmas.iter().filter(|&&sigma| sigma == 0.0).count();
    if quiet > 0 {
        return sigmas.map(|sigma| {
            if sigma == 0.0 {
                1.0 / quiet as f32
            } else {
                0.0
            }
        });
    }
    let least = sigmas.into_iter().fold(f32::INFINITY, f32::min);
    let relative = sigmas.map(|sigma| (least / sigma).powi(2));
    let sum: f32 = relative.iter().sum();
    relative.map(|weight| weight / sum)
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
