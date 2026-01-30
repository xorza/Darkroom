//! Gaussian convolution for matched filtering in star detection.
//!
//! Implements separable Gaussian convolution which is O(n×k) instead of O(n×k²)
//! where k is the kernel size. This is the key technique used by DAOFIND and
//! SExtractor to boost SNR for faint star detection.
//!
//! Uses SIMD acceleration when available (AVX2/SSE on x86_64, NEON on aarch64).

#[cfg(test)]
mod tests;

#[cfg(feature = "bench")]
pub mod bench;

mod simd;

use rayon::prelude::*;

use super::common::ROWS_PER_CHUNK;
use crate::math::fwhm_to_sigma;
use crate::star_detection::Buffer2;

/// Compute 1D Gaussian kernel.
///
/// The kernel is normalized so that it sums to 1.0.
/// Kernel radius is chosen as ceil(3 * sigma) to capture 99.7% of the Gaussian.
///
/// # Arguments
/// * `sigma` - Standard deviation of the Gaussian (in pixels)
///
/// # Returns
/// Vector containing the kernel values, length is 2 * radius + 1
pub fn gaussian_kernel_1d(sigma: f32) -> Vec<f32> {
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

    // Normalize
    for v in &mut kernel {
        *v /= sum;
    }

    kernel
}

/// Apply separable Gaussian convolution to an image.
///
/// Uses separable convolution: first convolve rows, then columns.
/// This is O(n×k) instead of O(n×k²) for a 2D convolution.
///
/// # Arguments
/// * `pixels` - Input image buffer
/// * `sigma` - Gaussian sigma (use `fwhm_to_sigma` to convert from FWHM)
///
/// # Returns
/// Convolved image of the same size
pub fn gaussian_convolve(pixels: &Buffer2<f32>, sigma: f32) -> Buffer2<f32> {
    assert!(sigma > 0.0, "Sigma must be positive");

    let width = pixels.width();
    let height = pixels.height();
    let kernel = gaussian_kernel_1d(sigma);
    let radius = kernel.len() / 2;

    // If kernel is larger than image dimension, fall back to direct 2D convolution
    if radius >= width.min(height) / 2 {
        return gaussian_convolve_2d_direct(pixels, sigma);
    }

    // Step 1: Convolve rows (horizontal pass)
    let mut temp = Buffer2::new_default(width, height);
    convolve_rows_parallel(pixels, &mut temp, &kernel);

    // Step 2: Convolve columns (vertical pass)
    let mut output = Buffer2::new_default(width, height);
    convolve_cols_parallel(&temp, &mut output, &kernel);

    output
}

/// Convolve all rows in parallel with chunking to reduce false cache sharing.
///
/// Instead of giving each thread a single row, we process multiple rows per chunk.
/// This improves cache locality and reduces potential false sharing when output
/// rows are close together in memory.
fn convolve_rows_parallel(input: &Buffer2<f32>, output: &mut Buffer2<f32>, kernel: &[f32]) {
    let width = input.width();
    let radius = kernel.len() / 2;

    output
        .pixels_mut()
        .par_chunks_mut(width * ROWS_PER_CHUNK)
        .enumerate()
        .for_each(|(chunk_idx, out_chunk)| {
            let y_start = chunk_idx * ROWS_PER_CHUNK;
            let rows_in_chunk = out_chunk.len() / width;

            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                let in_row = &input[y * width..(y + 1) * width];
                let out_row = &mut out_chunk[local_y * width..(local_y + 1) * width];
                convolve_row(in_row, out_row, kernel, radius);
            }
        });
}

/// Convolve a single row with 1D kernel.
///
/// Uses SIMD acceleration when available (AVX2/SSE on x86_64, NEON on aarch64).
#[inline]
fn convolve_row(input: &[f32], output: &mut [f32], kernel: &[f32], radius: usize) {
    simd::convolve_row_simd(input, output, kernel, radius);
}

/// Convolve all columns in parallel with chunking to reduce false cache sharing.
///
/// Processes multiple rows per chunk to improve cache locality and reduce
/// false sharing between threads writing to adjacent output rows.
fn convolve_cols_parallel(input: &Buffer2<f32>, output: &mut Buffer2<f32>, kernel: &[f32]) {
    let width = input.width();
    let height = input.height();
    let radius = kernel.len() / 2;

    output
        .pixels_mut()
        .par_chunks_mut(width * ROWS_PER_CHUNK)
        .enumerate()
        .for_each(|(chunk_idx, out_chunk)| {
            let y_start = chunk_idx * ROWS_PER_CHUNK;
            let rows_in_chunk = out_chunk.len() / width;

            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                let out_row = &mut out_chunk[local_y * width..(local_y + 1) * width];

                for x in 0..width {
                    let mut sum = 0.0f32;

                    for (k, &kval) in kernel.iter().enumerate() {
                        let sy = y as isize + k as isize - radius as isize;

                        // Mirror boundary handling
                        let sy = if sy < 0 {
                            (-sy) as usize
                        } else if sy >= height as isize {
                            2 * height - 2 - sy as usize
                        } else {
                            sy as usize
                        };

                        sum += input[sy * width + x] * kval;
                    }

                    out_row[x] = sum;
                }
            }
        });
}

/// Direct 2D Gaussian convolution for small images or large kernels.
fn gaussian_convolve_2d_direct(pixels: &Buffer2<f32>, sigma: f32) -> Buffer2<f32> {
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

    let mut output = vec![0.0f32; width * height];

    for y in 0..height {
        for x in 0..width {
            let mut sum = 0.0f32;

            for ky in 0..ksize {
                for kx in 0..ksize {
                    let sx = x as isize + kx as isize - radius as isize;
                    let sy = y as isize + ky as isize - radius as isize;

                    // Mirror boundary
                    let sx: usize = if sx < 0 {
                        (-sx) as usize
                    } else if sx >= width as isize {
                        2 * width - 2 - sx as usize
                    } else {
                        sx as usize
                    };

                    let sy: usize = if sy < 0 {
                        (-sy) as usize
                    } else if sy >= height as isize {
                        2 * height - 2 - sy as usize
                    } else {
                        sy as usize
                    };

                    sum += pixels[sy * width + sx] * kernel_2d[ky * ksize + kx];
                }
            }

            output[y * width + x] = sum;
        }
    }

    Buffer2::new(width, height, output)
}

/// Apply matched filter convolution optimized for star detection.
///
/// This convolves the background-subtracted image with a Gaussian kernel
/// matching the expected PSF. The result is normalized to preserve flux
/// and can be directly thresholded.
///
/// # Arguments
/// * `pixels` - Input image buffer
/// * `background` - Per-pixel background values
/// * `fwhm` - Expected FWHM of stars in pixels
///
/// # Returns
/// Convolved, background-subtracted image
pub fn matched_filter(pixels: &Buffer2<f32>, background: &Buffer2<f32>, fwhm: f32) -> Buffer2<f32> {
    assert_eq!(pixels.width(), background.width());
    assert_eq!(pixels.height(), background.height());

    // Subtract background first
    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.iter())
        .map(|(&p, &b): (&f32, &f32)| (p - b).max(0.0))
        .collect();

    // Convolve with Gaussian matching expected PSF
    let sigma = fwhm_to_sigma(fwhm);
    gaussian_convolve(
        &Buffer2::new(pixels.width(), pixels.height(), subtracted),
        sigma,
    )
}

/// Apply matched filter with an elliptical Gaussian kernel.
///
/// This variant allows for non-circular PSF shapes, useful when stars are
/// elongated due to tracking errors, field rotation, or optical aberrations.
///
/// # Arguments
/// * `pixels` - Input image buffer
/// * `background` - Per-pixel background values
/// * `fwhm` - Expected FWHM of stars in pixels (major axis)
/// * `axis_ratio` - Ratio of minor to major axis (0 < ratio <= 1, where 1 = circular)
/// * `angle` - Position angle of major axis in radians (0 = along x-axis)
///
/// # Returns
/// Convolved, background-subtracted image
pub fn matched_filter_elliptical(
    pixels: &Buffer2<f32>,
    background: &Buffer2<f32>,
    fwhm: f32,
    axis_ratio: f32,
    angle: f32,
) -> Buffer2<f32> {
    assert_eq!(pixels.width(), background.width());
    assert_eq!(pixels.height(), background.height());
    assert!(
        axis_ratio > 0.0 && axis_ratio <= 1.0,
        "Axis ratio must be in (0, 1]"
    );

    // Subtract background first
    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.iter())
        .map(|(&p, &b): (&f32, &f32)| (p - b).max(0.0))
        .collect();

    // Convolve with elliptical Gaussian kernel
    let sigma = fwhm_to_sigma(fwhm);
    elliptical_gaussian_convolve(
        &Buffer2::new(pixels.width(), pixels.height(), subtracted),
        sigma,
        axis_ratio,
        angle,
    )
}

/// Compute 2D elliptical Gaussian kernel.
///
/// The kernel is normalized so that it sums to 1.0.
///
/// # Arguments
/// * `sigma` - Standard deviation along the major axis (in pixels)
/// * `axis_ratio` - Ratio of minor to major axis (0 < ratio <= 1)
/// * `angle` - Position angle of major axis in radians (0 = along x-axis)
///
/// # Returns
/// Tuple of (kernel values, kernel width, kernel height)
pub fn elliptical_gaussian_kernel_2d(sigma: f32, axis_ratio: f32, angle: f32) -> (Vec<f32>, usize) {
    assert!(sigma > 0.0, "Sigma must be positive");
    assert!(
        axis_ratio > 0.0 && axis_ratio <= 1.0,
        "Axis ratio must be in (0, 1]"
    );

    // Kernel radius based on major axis sigma
    let radius = (3.0 * sigma).ceil() as usize;
    let size = 2 * radius + 1;

    // Sigma values for major and minor axes
    let sigma_major = sigma;
    let sigma_minor = sigma * axis_ratio;

    // Precompute rotation matrix components
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    // Precompute denominators
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

            // Compute elliptical Gaussian value
            let value =
                (-x_rot * x_rot / two_sigma_major_sq - y_rot * y_rot / two_sigma_minor_sq).exp();

            kernel[ky * size + kx] = value;
            sum += value;
        }
    }

    // Normalize
    for v in &mut kernel {
        *v /= sum;
    }

    (kernel, size)
}

/// Apply elliptical Gaussian convolution to an image.
///
/// Unlike separable convolution for circular Gaussians, elliptical Gaussians
/// require full 2D convolution which is O(n×k²). This is used when the PSF
/// is known to be non-circular.
///
/// # Arguments
/// * `pixels` - Input image buffer
/// * `sigma` - Gaussian sigma along major axis (use `fwhm_to_sigma` to convert from FWHM)
/// * `axis_ratio` - Ratio of minor to major axis (0 < ratio <= 1, where 1 = circular)
/// * `angle` - Position angle of major axis in radians (0 = along x-axis)
///
/// # Returns
/// Convolved image of the same size
pub fn elliptical_gaussian_convolve(
    pixels: &Buffer2<f32>,
    sigma: f32,
    axis_ratio: f32,
    angle: f32,
) -> Buffer2<f32> {
    let width = pixels.width();
    let height = pixels.height();

    // For axis_ratio very close to 1.0, use faster separable convolution
    if (axis_ratio - 1.0).abs() < 0.01 {
        return gaussian_convolve(pixels, sigma);
    }

    let (kernel, ksize) = elliptical_gaussian_kernel_2d(sigma, axis_ratio, angle);
    let radius = ksize / 2;

    let mut output = vec![0.0f32; width * height];

    output
        .par_chunks_mut(width * ROWS_PER_CHUNK)
        .enumerate()
        .for_each(|(chunk_idx, out_chunk)| {
            let y_start = chunk_idx * ROWS_PER_CHUNK;
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

                            // Mirror boundary handling
                            let sx: usize = if sx < 0 {
                                (-sx) as usize
                            } else if sx >= width as isize {
                                2 * width - 2 - sx as usize
                            } else {
                                sx as usize
                            };

                            let sy: usize = if sy < 0 {
                                (-sy) as usize
                            } else if sy >= height as isize {
                                2 * height - 2 - sy as usize
                            } else {
                                sy as usize
                            };

                            sum += pixels[sy * width + sx] * kernel[ky * ksize + kx];
                        }
                    }

                    *out_px = sum;
                }
            }
        });

    Buffer2::new(width, height, output)
}
