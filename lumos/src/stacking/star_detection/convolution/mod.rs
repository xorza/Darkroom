//! Gaussian convolution for matched filtering in star detection.
//!
//! Implements separable Gaussian convolution which is O(n×k) instead of O(n×k²)
//! where k is the kernel size. This is the key technique used by DAOFIND and
//! SExtractor to boost SNR for faint star detection.
//!
//! Uses SIMD acceleration when available (AVX2/SSE on `x86_64`, NEON on aarch64).

mod simd;

use rayon::prelude::*;

use crate::image_ops::SAMPLES_PER_BLOCK;
use crate::math::fwhm::fwhm_to_sigma;
use crate::math::size2us::Size2us;
use imaginarium::Buffer2;

/// Maximum deviation of `axis_ratio` from 1.0 to use the faster separable
/// (circular) kernel path instead of full 2D elliptical convolution.
const CIRCULAR_KERNEL_THRESHOLD: f32 = 0.01;

#[derive(Debug)]
struct GaussianKernel2d {
    weights: Vec<f32>,
    size: usize,
}

/// Scratch buffers for [`matched_filter`]. Both must have the same dimensions as the input image.
#[derive(Debug)]
pub(crate) struct MatchedFilterBuffers<'a> {
    /// Convolved output (the result).
    pub(crate) output: &'a mut Buffer2<f32>,
    /// Temporary buffer for separable convolution passes.
    pub(crate) temp: &'a mut Buffer2<f32>,
}

/// Apply matched filter convolution optimized for star detection.
///
/// Convolves the residual — the image less its sky, so the convolution carries no pedestal — with a
/// Gaussian kernel matching the expected PSF. The output is normalized by `sqrt(sum(K^2))` so that
/// the noise level in the convolved image matches the original noise map, and the threshold
/// `filtered[px] > sigma * noise[px]` stays correct.
///
/// Follows the SEP matched filter approach (Barbary 2016):
/// `SNR = conv(D, K) / (sigma * sqrt(sum(K^2)))`
///
/// Supports elliptical PSF shapes for stars elongated due to tracking errors,
/// field rotation, or optical aberrations. For circular PSFs, use `axis_ratio = 1.0`.
pub(crate) fn matched_filter(
    residual: &Buffer2<f32>,
    fwhm: f32,
    axis_ratio: f32,
    angle: f32,
    buffers: &mut MatchedFilterBuffers<'_>,
) {
    let output = &mut *buffers.output;
    let temp = &mut *buffers.temp;
    assert_eq!(residual.width(), output.width());
    assert_eq!(residual.height(), output.height());
    assert!(
        axis_ratio > 0.0 && axis_ratio <= 1.0,
        "Axis ratio must be in (0, 1]"
    );

    let sigma = fwhm_to_sigma(fwhm);

    // sqrt(sum(K²)) of the kernel the convolution used; an axis ratio this close to 1 takes the
    // separable circular kernel.
    let noise_norm = if (axis_ratio - 1.0).abs() < CIRCULAR_KERNEL_THRESHOLD {
        gaussian_convolve(residual, sigma, output, temp)
    } else {
        elliptical_gaussian_convolve(residual, sigma, axis_ratio, angle, output)
    };

    // After convolution the noise is the map's times sqrt(sum(K²)); dividing it out puts the
    // filtered image back on the map's scale.
    let inv_norm = 1.0 / noise_norm;
    output
        .pixels_mut()
        .par_chunks_mut(SAMPLES_PER_BLOCK)
        .for_each(|block| block.iter_mut().for_each(|px| *px *= inv_norm));
}

/// Apply separable Gaussian convolution to an image.
///
/// Uses separable convolution: first convolve rows, then columns.
/// This is O(n×k) instead of O(n×k²) for a 2D convolution.
/// Returns `sqrt(sum(K²))` for the equivalent normalized 2D kernel.
fn gaussian_convolve(
    pixels: &Buffer2<f32>,
    sigma: f32,
    output: &mut Buffer2<f32>,
    temp: &mut Buffer2<f32>,
) -> f32 {
    assert!(sigma > 0.0, "Sigma must be positive");
    assert_eq!(pixels.width(), output.width());
    assert_eq!(pixels.height(), output.height());
    assert_eq!(pixels.width(), temp.width());
    assert_eq!(pixels.height(), temp.height());

    let kernel = gaussian_kernel_1d(sigma);
    gaussian_convolve_with_kernel(pixels, &kernel, output, temp);
    kernel.iter().map(|&weight| weight * weight).sum()
}

fn gaussian_convolve_with_kernel(
    pixels: &Buffer2<f32>,
    kernel: &[f32],
    output: &mut Buffer2<f32>,
    temp: &mut Buffer2<f32>,
) {
    // Both passes mirror at the edges per axis, so they hold for a kernel wider than the image
    // too — the outer-product kernel with the same per-axis mirror is algebraically this result.

    // Step 1: Convolve rows (horizontal pass)
    convolve_rows_parallel(pixels, temp, kernel);

    // Step 2: Convolve columns (vertical pass)
    convolve_cols(temp, output, kernel);
}

/// Apply elliptical Gaussian convolution to an image.
///
/// Unlike separable convolution for circular Gaussians, elliptical Gaussians
/// require full 2D convolution which is O(n×k²). This is used when the PSF
/// is known to be non-circular.
/// Returns `sqrt(sum(K²))` for the normalized 2D kernel.
fn elliptical_gaussian_convolve(
    pixels: &Buffer2<f32>,
    sigma: f32,
    axis_ratio: f32,
    angle: f32,
    output: &mut Buffer2<f32>,
) -> f32 {
    assert_eq!(pixels.width(), output.width());
    assert_eq!(pixels.height(), output.height());

    let kernel = elliptical_gaussian_kernel_2d(sigma, axis_ratio, angle);
    convolve_2d(pixels, &kernel, output);
    kernel
        .weights
        .iter()
        .map(|&weight| weight * weight)
        .sum::<f32>()
        .sqrt()
}

fn convolve_2d(pixels: &Buffer2<f32>, kernel: &GaussianKernel2d, output: &mut Buffer2<f32>) {
    let width = pixels.width();
    let height = pixels.height();
    let kernel = simd::Kernel2d::new(&kernel.weights, kernel.size);

    // Parallel SIMD 2D convolution - process rows in parallel
    output
        .pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out_row)| {
            simd::convolve_2d_row(
                pixels.pixels(),
                out_row,
                Size2us::new(width, height),
                y,
                kernel,
            );
        });
}

/// Compute 1D Gaussian kernel (normalized to sum to 1.0).
fn gaussian_kernel_1d(sigma: f32) -> Vec<f32> {
    assert!(sigma > 0.0, "Sigma must be positive");

    let radius = (3.0 * sigma).ceil() as usize;
    let size = 2 * radius + 1;
    let mut kernel = vec![0.0f32; size];

    let two_sigma_sq = 2.0 * sigma * sigma;
    let mut sum = 0.0f32;

    for (i, k) in kernel.iter_mut().enumerate() {
        let x = i as f32 - radius as f32;
        let value = (-x * x / two_sigma_sq).exp();
        *k = value;
        sum += value;
    }

    for v in &mut kernel {
        *v /= sum;
    }

    kernel
}

/// Convolve all rows in parallel using SIMD.
fn convolve_rows_parallel(input: &Buffer2<f32>, output: &mut Buffer2<f32>, kernel: &[f32]) {
    let width = input.width();
    let radius = kernel.len() / 2;

    output
        .pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out_row)| {
            simd::convolve_row(input.row(y), out_row, kernel, radius);
        });
}

/// Convolve all columns: rayon-parallel over output rows, SIMD across the columns within each row.
fn convolve_cols(input: &Buffer2<f32>, output: &mut Buffer2<f32>, kernel: &[f32]) {
    let size = Size2us::new(input.width(), input.height());
    let radius = kernel.len() / 2;

    simd::convolve_cols_direct(input.pixels(), output.pixels_mut(), size, kernel, radius);
}

/// Compute 2D elliptical Gaussian kernel (normalized to sum to 1.0).
fn elliptical_gaussian_kernel_2d(sigma: f32, axis_ratio: f32, angle: f32) -> GaussianKernel2d {
    assert!(sigma > 0.0, "Sigma must be positive");
    assert!(
        axis_ratio > 0.0 && axis_ratio <= 1.0,
        "Axis ratio must be in (0, 1]"
    );

    let radius = (3.0 * sigma).ceil() as usize;
    let size = 2 * radius + 1;

    let sigma_major = sigma;
    let sigma_minor = sigma * axis_ratio;

    let cos_a = angle.cos();
    let sin_a = angle.sin();

    let two_sigma_major_sq = 2.0 * sigma_major * sigma_major;
    let two_sigma_minor_sq = 2.0 * sigma_minor * sigma_minor;

    let mut kernel = vec![0.0f32; size * size];
    let mut sum = 0.0f32;

    for ky in 0..size {
        for kx in 0..size {
            let x = kx as f32 - radius as f32;
            let y = ky as f32 - radius as f32;

            // Rotate coordinates to align with ellipse axes
            let x_rot = x * cos_a + y * sin_a;
            let y_rot = -x * sin_a + y * cos_a;

            let value =
                (-x_rot * x_rot / two_sigma_major_sq - y_rot * y_rot / two_sigma_minor_sq).exp();

            kernel[ky * size + kx] = value;
            sum += value;
        }
    }

    for v in &mut kernel {
        *v /= sum;
    }

    GaussianKernel2d {
        weights: kernel,
        size,
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
