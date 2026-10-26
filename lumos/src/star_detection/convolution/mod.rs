//! Gaussian convolution for matched filtering in star detection: the key technique DAOFIND and
//! SExtractor use to lift faint stars above the noise.
//!
//! A Gaussian along the pixel axes is separable, a row pass and a column pass at O(n×k) for a
//! kernel of k taps rather than O(n×k²); an ellipse at another angle convolves with a 2D kernel.
//! Every pass runs as a vector kernel on the crate's tiers: AVX2 with FMA, NEON, and portable.

pub(crate) mod filter_kernels;
pub(super) mod simd;

use std::f64::consts::FRAC_PI_2;

use crate::math::fwhm;
use crate::math::pixel_gaussian::PixelGaussian;
use crate::star_detection::star_detection_config::fwhm_config::MAX_FWHM;

/// The quarter turns of `angle` when it is the f32 nearest a multiple of π/2, which is as near
/// to the pixel axes as a stored angle comes.
fn quarter_turns(angle: f32) -> Option<i64> {
    let turns = (f64::from(angle) / FRAC_PI_2).round();
    ((turns * FRAC_PI_2) as f32 == angle).then_some(turns as i64)
}

/// The widest kernel's half-width: a star's [`MAX_FWHM`] out to `3σ`, along an ellipse's major
/// axis too.
pub(crate) fn max_kernel_radius() -> usize {
    kernel_radius(fwhm::to_sigma(MAX_FWHM))
}

/// A kernel's half-width: out to `3σ`.
#[expect(
    clippy::cast_sign_loss,
    reason = "σ derives from a validated, positive FWHM"
)]
fn kernel_radius(sigma: f32) -> usize {
    (3.0 * sigma).ceil() as usize
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

#[cfg(test)]
pub(crate) mod internals {
    use common::{Buffer2, par};
    use rayon::prelude::*;

    use crate::star_detection::convolution::filter_kernels::{EllipticalGaussian, FilterKernels};
    use crate::star_detection::convolution::simd::{
        Convolve2dRow, ConvolveColsRow, ConvolveRow, Kernel2d, RowRing,
    };
    use crate::star_detection::convolution::{fill_gaussian_kernel_1d, kernel_radius};
    use crate::star_detection::star_detection_config::fwhm_config::MatchedFilter;
    use ::simd::Kernel;

    /// The 1D kernel of `sigma` out to `3σ`.
    pub(crate) fn gaussian_kernel_1d(sigma: f32) -> Vec<f32> {
        let mut kernel = Vec::new();
        fill_gaussian_kernel_1d(sigma, kernel_radius(sigma), &mut kernel);
        kernel
    }

    /// Every row of `input` convolved with `kernel`, into `output`.
    pub(crate) fn convolve_rows(input: &Buffer2<f32>, output: &mut Buffer2<f32>, kernel: &[f32]) {
        par::chunks_mut(output.pixels_mut(), input.width())
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

    /// Every column of `input` convolved with `kernel`, into `output`, mirrored at the top and
    /// bottom.
    pub(crate) fn convolve_cols(input: &Buffer2<f32>, output: &mut Buffer2<f32>, kernel: &[f32]) {
        par::chunks_mut(output.pixels_mut(), input.width())
            .enumerate()
            .for_each(|(y, out_row)| {
                ConvolveColsRow {
                    input: RowRing::plane(input),
                    out_row,
                    y,
                    kernel,
                }
                .dispatch();
            });
    }

    /// `pixels` convolved with the 2D `kernel`, into `output`, mirrored on both axes.
    pub(super) fn convolve_2d(
        pixels: &Buffer2<f32>,
        kernel: Kernel2d<'_>,
        output: &mut Buffer2<f32>,
    ) {
        par::chunks_mut(output.pixels_mut(), pixels.width())
            .enumerate()
            .for_each(|(y, output_row)| {
                Convolve2dRow {
                    input: RowRing::plane(pixels),
                    output_row,
                    y,
                    kernel,
                }
                .dispatch();
            });
    }

    /// The circular separable convolution of `values` in place on fresh kernels, `temp` for the
    /// row pass; returns `sqrt(sum(K²))` of the equivalent 2D kernel.
    pub(crate) fn gaussian_convolve(
        values: &mut Buffer2<f32>,
        sigma: f32,
        temp: &mut Buffer2<f32>,
    ) -> f32 {
        separable_convolve(values, [sigma, sigma], temp)
    }

    /// The Gaussian of `[σx, σy]` along the axes convolved with `values` in place, out to `3σ` of
    /// the wider both ways, `temp` for the row pass; returns `sqrt(Σkx²·Σky²)`.
    pub(super) fn separable_convolve(
        values: &mut Buffer2<f32>,
        [sigma_x, sigma_y]: [f32; 2],
        temp: &mut Buffer2<f32>,
    ) -> f32 {
        let radius = kernel_radius(sigma_x.max(sigma_y));
        let (mut rows, mut cols) = (Vec::new(), Vec::new());
        fill_gaussian_kernel_1d(sigma_x, radius, &mut rows);
        fill_gaussian_kernel_1d(sigma_y, radius, &mut cols);
        convolve_rows(values, temp, &rows);
        convolve_cols(temp, values, &cols);
        let squares = |kernel: &[f32]| kernel.iter().map(|&weight| weight * weight).sum::<f32>();
        (squares(&rows) * squares(&cols)).sqrt()
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

    /// The full 2D convolution on fresh kernels, at any angle, into `output`; returns
    /// `sqrt(sum(K²))`.
    pub(super) fn elliptical_gaussian_convolve(
        pixels: &Buffer2<f32>,
        ellipse: EllipticalGaussian,
        output: &mut Buffer2<f32>,
    ) -> f32 {
        let GaussianKernel2d { weights, size } = elliptical_gaussian_kernel_2d(ellipse);
        convolve_2d(pixels, Kernel2d::new(&weights, size), output);
        weights
            .iter()
            .map(|&weight| weight * weight)
            .sum::<f32>()
            .sqrt()
    }

    /// The matched filter of `values` in place on fresh kernels, over whole planes, `temp` holding
    /// every row the taps read: the reference a band at a time is held to.
    pub(crate) fn matched_filter_fresh(
        values: &mut Buffer2<f32>,
        filter: MatchedFilter,
        temp: &mut Buffer2<f32>,
    ) {
        let mut kernels = FilterKernels::default();
        let kernel = kernels.matched(filter);
        let size = values.size();
        let width = size.width;
        par::chunks_mut(temp.pixels_mut(), width)
            .zip(par::chunks(values.pixels(), width))
            .for_each(|(ring_row, input)| kernel.ring_row(input, ring_row));
        par::chunks_mut(values.pixels_mut(), width)
            .enumerate()
            .for_each(|(y, output)| kernel.output_row(temp.pixels(), size, y, output));
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
