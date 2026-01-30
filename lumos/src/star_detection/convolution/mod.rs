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

use crate::math::fwhm_to_sigma;
use crate::star_detection::Buffer2;
use common::parallel;

// ============================================================================
// Public API
// ============================================================================

/// Apply matched filter convolution optimized for star detection.
///
/// This convolves the background-subtracted image with a Gaussian kernel
/// matching the expected PSF. The result is normalized to preserve flux
/// and can be directly thresholded.
///
/// Supports elliptical PSF shapes for stars elongated due to tracking errors,
/// field rotation, or optical aberrations. For circular PSFs, use `axis_ratio = 1.0`.
pub fn matched_filter(
    pixels: &Buffer2<f32>,
    background: &Buffer2<f32>,
    fwhm: f32,
    axis_ratio: f32,
    angle: f32,
    output: &mut Buffer2<f32>,
) {
    assert_eq!(pixels.width(), background.width());
    assert_eq!(pixels.height(), background.height());
    assert_eq!(pixels.width(), output.width());
    assert_eq!(pixels.height(), output.height());
    assert!(
        axis_ratio > 0.0 && axis_ratio <= 1.0,
        "Axis ratio must be in (0, 1]"
    );

    // Subtract background first (parallel)
    let mut subtracted = vec![0.0f32; pixels.len()];
    let pixels_data = pixels.pixels();
    let bg_data = background.pixels();
    parallel::par_chunks_auto(&mut subtracted).for_each(|(offset, chunk)| {
        for (i, out) in chunk.iter_mut().enumerate() {
            let idx = offset + i;
            *out = (pixels_data[idx] - bg_data[idx]).max(0.0);
        }
    });

    // Convolve with elliptical Gaussian kernel
    let sigma = fwhm_to_sigma(fwhm);
    elliptical_gaussian_convolve(
        &Buffer2::new(pixels.width(), pixels.height(), subtracted),
        sigma,
        axis_ratio,
        angle,
        output,
    );
}

// ============================================================================
// Internal API (visible to parent module and tests)
// ============================================================================

/// Apply separable Gaussian convolution to an image.
///
/// Uses separable convolution: first convolve rows, then columns.
/// This is O(n×k) instead of O(n×k²) for a 2D convolution.
pub(super) fn gaussian_convolve(pixels: &Buffer2<f32>, sigma: f32, output: &mut Buffer2<f32>) {
    assert!(sigma > 0.0, "Sigma must be positive");
    assert_eq!(pixels.width(), output.width());
    assert_eq!(pixels.height(), output.height());

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
    let mut temp = Buffer2::new_default(width, height);
    convolve_rows_parallel(pixels, &mut temp, &kernel);

    // Step 2: Convolve columns (vertical pass)
    convolve_cols_parallel(&temp, output, &kernel);
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
) {
    let width = pixels.width();
    let height = pixels.height();
    assert_eq!(width, output.width());
    assert_eq!(height, output.height());

    // For axis_ratio very close to 1.0, use faster separable convolution
    if (axis_ratio - 1.0).abs() < 0.01 {
        gaussian_convolve(pixels, sigma, output);
        return;
    }

    let (kernel, ksize) = elliptical_gaussian_kernel_2d(sigma, axis_ratio, angle);
    let radius = ksize / 2;

    parallel::par_chunks_auto_aligned(output.pixels_mut(), width).for_each(
        |(y_start, out_chunk)| {
            let rows_in_chunk = out_chunk.len() / width;

            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                let out_row = &mut out_chunk[local_y * width..(local_y + 1) * width];

                for (x, out_px) in out_row.iter_mut().enumerate() {
                    let mut sum = 0.0f32;

                    for ky in 0..ksize {
                        for kx in 0..ksize {
                            let sx = x as isize + kx as isize - radius as isize;
                            let sy = y as isize + ky as isize - radius as isize;

                            let sx = mirror_index(sx, width);
                            let sy = mirror_index(sy, height);

                            sum += pixels[sy * width + sx] * kernel[ky * ksize + kx];
                        }
                    }

                    *out_px = sum;
                }
            }
        },
    );
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

    parallel::par_chunks_auto_aligned(output.pixels_mut(), width).for_each(
        |(y_start, out_chunk)| {
            let rows_in_chunk = out_chunk.len() / width;

            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                let in_row = &input[y * width..(y + 1) * width];
                let out_row = &mut out_chunk[local_y * width..(local_y + 1) * width];
                simd::convolve_row(in_row, out_row, kernel, radius);
            }
        },
    );
}

/// Convolve all columns in parallel using the optimal method.
///
/// For smaller images (< 4M pixels), uses transpose → SIMD row convolve → transpose.
/// For larger images, uses scalar column convolution which has better cache behavior
/// and avoids the transpose memory overhead.
#[cfg_attr(test, allow(dead_code))]
pub(super) fn convolve_cols_parallel(
    input: &Buffer2<f32>,
    output: &mut Buffer2<f32>,
    kernel: &[f32],
) {
    let width = input.width();
    let height = input.height();
    let total_pixels = width * height;

    // Threshold determined by benchmarks:
    // - Below ~4M pixels: SIMD transpose is faster
    // - Above ~4M pixels: scalar is faster due to transpose memory overhead
    const SIMD_TRANSPOSE_THRESHOLD: usize = 4 * 1024 * 1024;

    if total_pixels < SIMD_TRANSPOSE_THRESHOLD {
        convolve_cols_simd_transpose_parallel(input, output, kernel);
    } else {
        convolve_cols_scalar_parallel(input, output, kernel);
    }
}

/// Convolve columns using SIMD via transpose.
///
/// Uses the approach: transpose → row convolve (SIMD) → transpose back.
pub(super) fn convolve_cols_simd_transpose_parallel(
    input: &Buffer2<f32>,
    output: &mut Buffer2<f32>,
    kernel: &[f32],
) {
    let width = input.width();
    let height = input.height();
    let radius = kernel.len() / 2;

    // Transpose input (width×height → height×width)
    let mut transposed_in = vec![0.0f32; width * height];
    transpose_parallel(input.pixels(), &mut transposed_in, width, height);

    // Convolve rows on transposed data (now columns become rows)
    let mut transposed_out = vec![0.0f32; width * height];
    // After transpose: dimensions are (height, width) - height rows of width elements
    parallel::par_chunks_auto_aligned(&mut transposed_out, width).for_each(
        |(y_start, out_chunk)| {
            let rows_in_chunk = out_chunk.len() / width;

            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                let in_row = &transposed_in[y * width..(y + 1) * width];
                let out_row = &mut out_chunk[local_y * width..(local_y + 1) * width];
                simd::convolve_row(in_row, out_row, kernel, radius);
            }
        },
    );

    // Transpose back (height×width → width×height)
    transpose_parallel(&transposed_out, output.pixels_mut(), height, width);
}

/// Convolve columns using scalar operations.
///
/// Better for large images where transpose memory overhead exceeds SIMD benefit.
pub(super) fn convolve_cols_scalar_parallel(
    input: &Buffer2<f32>,
    output: &mut Buffer2<f32>,
    kernel: &[f32],
) {
    let width = input.width();
    let height = input.height();
    let radius = kernel.len() / 2;

    parallel::par_chunks_auto_aligned(output.pixels_mut(), width).for_each(
        |(y_start, out_chunk)| {
            let rows_in_chunk = out_chunk.len() / width;

            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                let out_row = &mut out_chunk[local_y * width..(local_y + 1) * width];

                for x in 0..width {
                    let mut sum = 0.0f32;

                    for (k, &kval) in kernel.iter().enumerate() {
                        let sy = y as isize + k as isize - radius as isize;
                        let sy = mirror_index(sy, height);
                        sum += input[sy * width + x] * kval;
                    }

                    out_row[x] = sum;
                }
            }
        },
    );
}

/// Transpose a 2D buffer in parallel.
///
/// Transposes data from row-major (width×height) to column-major (height×width).
/// Uses cache-friendly block transposition for better performance.
fn transpose_parallel(input: &[f32], output: &mut [f32], width: usize, height: usize) {
    use std::sync::atomic::{AtomicU32, Ordering};

    const BLOCK_SIZE: usize = 64;

    // Reinterpret output as atomic for safe parallel writes
    // Safety: f32 and AtomicU32 have the same size and alignment
    let output_atomic =
        unsafe { std::slice::from_raw_parts(output.as_ptr() as *const AtomicU32, output.len()) };

    // Process in blocks for cache efficiency
    let block_rows = height.div_ceil(BLOCK_SIZE);
    let block_cols = width.div_ceil(BLOCK_SIZE);

    (0..block_rows * block_cols)
        .into_par_iter()
        .for_each(|block_idx| {
            let block_y = block_idx / block_cols;
            let block_x = block_idx % block_cols;

            let y_start = block_y * BLOCK_SIZE;
            let x_start = block_x * BLOCK_SIZE;
            let y_end = (y_start + BLOCK_SIZE).min(height);
            let x_end = (x_start + BLOCK_SIZE).min(width);

            for y in y_start..y_end {
                for x in x_start..x_end {
                    // input[y, x] -> output[x, y]
                    // input is width×height, output is height×width
                    let src_idx = y * width + x;
                    let dst_idx = x * height + y;
                    // Safety: indices are within bounds by construction, and each
                    // (x, y) pair maps to a unique dst_idx, so no data races
                    unsafe {
                        let val = *input.get_unchecked(src_idx);
                        output_atomic
                            .get_unchecked(dst_idx)
                            .store(val.to_bits(), Ordering::Relaxed);
                    }
                }
            }
        });
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

    parallel::par_chunks_auto_aligned(output.pixels_mut(), width).for_each(
        |(y_start, out_chunk)| {
            let rows_in_chunk = out_chunk.len() / width;

            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                let out_row = &mut out_chunk[local_y * width..(local_y + 1) * width];

                for (x, out_pixel) in out_row.iter_mut().enumerate() {
                    let mut sum = 0.0f32;

                    for ky in 0..ksize {
                        for kx in 0..ksize {
                            let sx = x as isize + kx as isize - radius as isize;
                            let sy = y as isize + ky as isize - radius as isize;

                            let sx = mirror_index(sx, width);
                            let sy = mirror_index(sy, height);

                            sum += pixels[sy * width + sx] * kernel_2d[ky * ksize + kx];
                        }
                    }

                    *out_pixel = sum;
                }
            }
        },
    );
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
