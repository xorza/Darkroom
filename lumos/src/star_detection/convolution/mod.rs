//! Gaussian convolution for matched filtering in star detection.
//!
//! Implements separable Gaussian convolution which is O(n×k) instead of O(n×k²)
//! where k is the kernel size. This is the key technique used by DAOFIND and
//! SExtractor to boost SNR for faint star detection.
//!
//! Uses SIMD acceleration when available (AVX2/SSE on x86_64, NEON on aarch64).

mod simd;

use simd::mirror_index;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

use rayon::prelude::*;

use crate::common::Buffer2;

/// Maximum deviation of axis_ratio from 1.0 to use the faster separable
/// (circular) kernel path instead of full 2D elliptical convolution.
const CIRCULAR_KERNEL_THRESHOLD: f32 = 0.01;
use crate::math::fwhm_to_sigma;

// ============================================================================
// Public API
// ============================================================================

/// Apply matched filter convolution optimized for star detection.
///
/// Convolves the background-subtracted image with a Gaussian kernel matching
/// the expected PSF. The output is normalized by `sqrt(sum(K^2))` so that the
/// noise level in the convolved image matches the original noise map. This
/// means the existing threshold `filtered[px] > sigma * noise[px]` is correct.
///
/// Follows the SEP matched filter approach (Barbary 2016):
/// `SNR = conv(D, K) / (sigma * sqrt(sum(K^2)))`
///
/// Supports elliptical PSF shapes for stars elongated due to tracking errors,
/// field rotation, or optical aberrations. For circular PSFs, use `axis_ratio = 1.0`.
///
/// # Arguments
/// * `pixels` - Input image
/// * `background` - Background model to subtract
/// * `fwhm` - Full width at half maximum of PSF
/// * `axis_ratio` - PSF ellipticity (1.0 = circular)
/// * `angle` - PSF rotation angle in radians
/// * `output` - Output buffer for convolved result
/// * `subtraction_scratch` - Scratch buffer for background subtraction (reuse to avoid allocation)
#[allow(clippy::too_many_arguments)]
pub fn matched_filter(
    pixels: &Buffer2<f32>,
    background: &Buffer2<f32>,
    fwhm: f32,
    axis_ratio: f32,
    angle: f32,
    output: &mut Buffer2<f32>,
    subtraction_scratch: &mut Buffer2<f32>,
    temp: &mut Buffer2<f32>,
) {
    assert_eq!(pixels.width(), background.width());
    assert_eq!(pixels.height(), background.height());
    assert_eq!(pixels.width(), output.width());
    assert_eq!(pixels.height(), output.height());
    assert_eq!(pixels.width(), subtraction_scratch.width());
    assert_eq!(pixels.height(), subtraction_scratch.height());
    assert!(
        axis_ratio > 0.0 && axis_ratio <= 1.0,
        "Axis ratio must be in (0, 1]"
    );

    // Subtract background (parallel) - allow negative residuals for correct
    // noise statistics during convolution (no clipping)
    subtraction_scratch
        .pixels_mut()
        .par_iter_mut()
        .zip(pixels.pixels().par_iter())
        .zip(background.pixels().par_iter())
        .for_each(|((out, &px), &bg)| {
            *out = px - bg;
        });

    // Convolve with elliptical Gaussian kernel
    let sigma = fwhm_to_sigma(fwhm);

    // Compute noise normalization factor: sqrt(sum(K^2))
    // For separable kernel: sum_2d(K^2) = sum_1d(K^2)^2
    // axis_ratio close to 1.0 → circular kernel; use faster separable path
    let noise_norm = if (axis_ratio - 1.0).abs() < CIRCULAR_KERNEL_THRESHOLD {
        let kernel_1d = gaussian_kernel_1d(sigma);
        let sum_k_sq: f32 = kernel_1d.iter().map(|&k| k * k).sum();
        (sum_k_sq * sum_k_sq).sqrt()
    } else {
        let (kernel_2d, _) = elliptical_gaussian_kernel_2d(sigma, axis_ratio, angle);
        let sum_k_sq: f32 = kernel_2d.iter().map(|&k| k * k).sum();
        sum_k_sq.sqrt()
    };

    elliptical_gaussian_convolve(subtraction_scratch, sigma, axis_ratio, angle, output, temp);

    // Normalize output so noise matches original noise map.
    // After convolution: conv_noise = orig_noise * sqrt(sum(K^2))
    // After division: normalized_noise = orig_noise
    // So threshold `filtered[px] > sigma * noise[px]` uses the correct noise.
    let inv_norm = 1.0 / noise_norm;
    output
        .pixels_mut()
        .par_iter_mut()
        .for_each(|px| *px *= inv_norm);
}

// ============================================================================
// Internal API (visible to parent module and tests)
// ============================================================================

/// Apply separable Gaussian convolution to an image.
///
/// Uses separable convolution: first convolve rows, then columns.
/// This is O(n×k) instead of O(n×k²) for a 2D convolution.
pub(super) fn gaussian_convolve(
    pixels: &Buffer2<f32>,
    sigma: f32,
    output: &mut Buffer2<f32>,
    temp: &mut Buffer2<f32>,
) {
    assert!(sigma > 0.0, "Sigma must be positive");
    assert_eq!(pixels.width(), output.width());
    assert_eq!(pixels.height(), output.height());
    assert_eq!(pixels.width(), temp.width());
    assert_eq!(pixels.height(), temp.height());

    let width = pixels.width();
    let height = pixels.height();
    let kernel = gaussian_kernel_1d(sigma);
    let radius = kernel.len() / 2;

    // If kernel is larger than image dimension, fall back to direct 2D convolution
    if radius >= width.min(height) / 2 {
        gaussian_convolve_2d_direct(pixels, sigma, output);
        return;
    }

    // Step 1: Convolve rows (horizontal pass)
    convolve_rows_parallel(pixels, temp, &kernel);

    // Step 2: Convolve columns (vertical pass)
    convolve_cols(temp, output, &kernel);
}

/// Apply elliptical Gaussian convolution to an image.
///
/// Unlike separable convolution for circular Gaussians, elliptical Gaussians
/// require full 2D convolution which is O(n×k²). This is used when the PSF
/// is known to be non-circular.
pub(super) fn elliptical_gaussian_convolve(
    pixels: &Buffer2<f32>,
    sigma: f32,
    axis_ratio: f32,
    angle: f32,
    output: &mut Buffer2<f32>,
    temp: &mut Buffer2<f32>,
) {
    let width = pixels.width();
    let height = pixels.height();
    assert_eq!(width, output.width());
    assert_eq!(height, output.height());

    // For axis_ratio very close to 1.0, use faster separable convolution
    if (axis_ratio - 1.0).abs() < CIRCULAR_KERNEL_THRESHOLD {
        gaussian_convolve(pixels, sigma, output, temp);
        return;
    }

    let (kernel, ksize) = elliptical_gaussian_kernel_2d(sigma, axis_ratio, angle);
    let radius = ksize / 2;

    // Parallel SIMD 2D convolution - process rows in parallel
    output
        .pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out_row)| {
            simd::convolve_2d_row(
                pixels.pixels(),
                out_row,
                width,
                height,
                y,
                &kernel,
                ksize,
                radius,
            );
        });
}

/// Compute 1D Gaussian kernel (normalized to sum to 1.0).
pub(super) fn gaussian_kernel_1d(sigma: f32) -> Vec<f32> {
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

// ============================================================================
// Private implementation
// ============================================================================

/// Convolve all rows in parallel using SIMD.
#[cfg_attr(test, allow(dead_code))]
pub(super) fn convolve_rows_parallel(
    input: &Buffer2<f32>,
    output: &mut Buffer2<f32>,
    kernel: &[f32],
) {
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

/// Convolve all columns in parallel using direct SIMD.
///
/// Processes multiple columns simultaneously using SIMD vectors.
/// Each SIMD lane handles a different column with row-major traversal for cache locality.
#[cfg_attr(test, allow(dead_code))]
pub(super) fn convolve_cols(input: &Buffer2<f32>, output: &mut Buffer2<f32>, kernel: &[f32]) {
    let width = input.width();
    let height = input.height();
    let radius = kernel.len() / 2;

    simd::convolve_cols_direct(
        input.pixels(),
        output.pixels_mut(),
        width,
        height,
        kernel,
        radius,
    );
}

/// Direct 2D Gaussian convolution for small images or large kernels.
fn gaussian_convolve_2d_direct(pixels: &Buffer2<f32>, sigma: f32, output: &mut Buffer2<f32>) {
    let width = pixels.width();
    let height = pixels.height();
    let kernel_1d = gaussian_kernel_1d(sigma);
    let radius = kernel_1d.len() / 2;

    // Build 2D kernel
    let ksize = kernel_1d.len();
    let mut kernel_2d = vec![0.0f32; ksize * ksize];
    for ky in 0..ksize {
        for kx in 0..ksize {
            kernel_2d[ky * ksize + kx] = kernel_1d[ky] * kernel_1d[kx];
        }
    }

    output
        .pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out_row)| {
            for (x, out_pixel) in out_row.iter_mut().enumerate() {
                let mut sum = 0.0f32;

                for ky in 0..ksize {
                    for kx in 0..ksize {
                        let sx = x as isize + kx as isize - radius as isize;
                        let sy = y as isize + ky as isize - radius as isize;

                        let sx = mirror_index(sx, width);
                        let sy = mirror_index(sy, height);

                        sum += pixels.row(sy)[sx] * kernel_2d[ky * ksize + kx];
                    }
                }

                *out_pixel = sum;
            }
        });
}

/// Compute 2D elliptical Gaussian kernel (normalized to sum to 1.0).
fn elliptical_gaussian_kernel_2d(sigma: f32, axis_ratio: f32, angle: f32) -> (Vec<f32>, usize) {
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

    (kernel, size)
}
