//! Gaussian convolution for matched filtering in star detection: the key technique DAOFIND and
//! SExtractor use to lift faint stars above the noise.
//!
//! A Gaussian along the pixel axes is separable, a row pass and a column pass at O(n×k) for a
//! kernel of k taps rather than O(n×k²); an ellipse at another angle convolves with a 2D kernel.
//! Every pass runs as a vector kernel on the crate's tiers: AVX2 with FMA, NEON, and portable.

pub(crate) mod filter_kernels;
pub(super) mod simd;

use std::f64::consts::FRAC_PI_2;

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::math::pixel_gaussian::PixelGaussian;
use crate::math::size2us::Size2us;
use crate::simd::Kernel;
use crate::star_detection::convolution::simd::{Convolve2dRow, ConvolveColsRow, ConvolveRow};

/// The quarter turns of `angle` when it is the f32 nearest a multiple of π/2, which is as near
/// to the pixel axes as a stored angle comes.
fn quarter_turns(angle: f32) -> Option<i64> {
    let turns = (f64::from(angle) / FRAC_PI_2).round();
    ((turns * FRAC_PI_2) as f32 == angle).then_some(turns as i64)
}

/// A kernel's half-width: out to `3σ`.
#[expect(
    clippy::cast_sign_loss,
    reason = "σ derives from a validated, positive FWHM"
)]
fn kernel_radius(sigma: f32) -> usize {
    (3.0 * sigma).ceil() as usize
}

fn convolve_2d(pixels: &Buffer2<f32>, kernel: simd::Kernel2d<'_>, output: &mut Buffer2<f32>) {
    let size = Size2us::from(pixels);
    output
        .pixels_mut()
        .par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(y, output_row)| {
            Convolve2dRow {
                input: pixels,
                output_row,
                y,
                kernel,
            }
            .dispatch();
        });
}

/// Into `kernel`, the 1D Gaussian of `sigma` out to `radius`, each weight the profile's mean over
/// its pixel, normalized to sum to 1.
fn fill_gaussian_kernel_1d(sigma: f32, radius: usize, kernel: &mut Vec<f32>) {
    debug_assert!(sigma > 0.0, "Sigma must be positive");
    let gaussian = PixelGaussian {
        sigma: f64::from(sigma),
    };
    let mean = |i: usize| gaussian.mean_at(i as f64 - radius as f64);
    let sum: f64 = (0..=2 * radius).map(mean).sum();
    kernel.clear();
    kernel.extend((0..=2 * radius).map(|i| (mean(i) / sum) as f32));
}

/// Convolve all rows in parallel, vectors within each row.
fn convolve_rows_parallel(input: &Buffer2<f32>, output: &mut Buffer2<f32>, kernel: &[f32]) {
    output
        .pixels_mut()
        .par_chunks_mut(input.width())
        .enumerate()
        .for_each(|(y, output)| {
            ConvolveRow {
                input: input.row(y),
                output,
                kernel,
            }
            .dispatch();
        });
}

/// Convolve all columns: rayon-parallel over output rows, vectors across the columns within each
/// row. Each output row depends on input rows `[y-radius, y+radius]` (mirror edges), so rows are
/// independent.
fn convolve_cols(input: &Buffer2<f32>, output: &mut Buffer2<f32>, kernel: &[f32]) {
    let size = Size2us::from(input);
    output
        .pixels_mut()
        .par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(y, out_row)| {
            ConvolveColsRow {
                input,
                out_row,
                y,
                kernel,
            }
            .dispatch();
        });
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;
    use crate::star_detection::convolution::filter_kernels::{EllipticalGaussian, FilterKernels};
    use crate::star_detection::star_detection_config::fwhm_config::MatchedFilter;

    /// The 1D kernel of `sigma` out to `3σ`.
    pub(crate) fn gaussian_kernel_1d(sigma: f32) -> Vec<f32> {
        let mut kernel = Vec::new();
        fill_gaussian_kernel_1d(sigma, kernel_radius(sigma), &mut kernel);
        kernel
    }

    /// The circular separable convolution on fresh kernels.
    pub(crate) fn gaussian_convolve(
        values: &mut Buffer2<f32>,
        sigma: f32,
        temp: &mut Buffer2<f32>,
    ) -> f32 {
        FilterKernels::default().separable(values, [sigma, sigma], temp)
    }

    /// A 2D kernel's weights, row by row, and its side.
    #[derive(Debug)]
    pub(super) struct GaussianKernel2d {
        pub(super) weights: Vec<f32>,
        pub(super) size: usize,
    }

    pub(super) fn elliptical_gaussian_kernel_2d(ellipse: EllipticalGaussian) -> GaussianKernel2d {
        let mut weights = Vec::new();
        let size = ellipse.fill_kernel(&mut weights);
        GaussianKernel2d { weights, size }
    }

    /// The full 2D convolution on fresh kernels, at any angle.
    pub(super) fn elliptical_gaussian_convolve(
        pixels: &Buffer2<f32>,
        ellipse: EllipticalGaussian,
        output: &mut Buffer2<f32>,
    ) -> f32 {
        FilterKernels::default().elliptical(pixels, ellipse, output)
    }

    /// [`FilterKernels::matched_filter`] on fresh kernels.
    pub(crate) fn matched_filter_fresh(
        values: &mut Buffer2<f32>,
        filter: MatchedFilter,
        temp: &mut Buffer2<f32>,
    ) {
        FilterKernels::default().matched_filter(values, filter, temp);
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
