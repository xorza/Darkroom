//! Gaussian convolution for matched filtering in star detection.
//!
//! Implements separable Gaussian convolution which is O(n×k) instead of O(n×k²)
//! where k is the kernel size. This is the key technique used by DAOFIND and
//! SExtractor to boost SNR for faint star detection.

#[cfg(test)]
mod tests;

use rayon::prelude::*;

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

/// Convert FWHM to Gaussian sigma.
///
/// FWHM = 2 * sqrt(2 * ln(2)) * sigma ≈ 2.355 * sigma
#[inline]
pub fn fwhm_to_sigma(fwhm: f32) -> f32 {
    fwhm / 2.355
}

/// Apply separable Gaussian convolution to an image.
///
/// Uses separable convolution: first convolve rows, then columns.
/// This is O(n×k) instead of O(n×k²) for a 2D convolution.
///
/// # Arguments
/// * `pixels` - Input image data
/// * `width` - Image width
/// * `height` - Image height
/// * `sigma` - Gaussian sigma (use `fwhm_to_sigma` to convert from FWHM)
///
/// # Returns
/// Convolved image of the same size
pub fn gaussian_convolve(pixels: &[f32], width: usize, height: usize, sigma: f32) -> Vec<f32> {
    assert_eq!(pixels.len(), width * height, "Pixel count mismatch");
    assert!(sigma > 0.0, "Sigma must be positive");

    let kernel = gaussian_kernel_1d(sigma);
    let radius = kernel.len() / 2;

    // If kernel is larger than image dimension, fall back to direct 2D convolution
    if radius >= width.min(height) / 2 {
        return gaussian_convolve_2d_direct(pixels, width, height, sigma);
    }

    // Step 1: Convolve rows (horizontal pass)
    let mut temp = vec![0.0f32; width * height];
    convolve_rows_parallel(pixels, &mut temp, width, &kernel);

    // Step 2: Convolve columns (vertical pass)
    let mut output = vec![0.0f32; width * height];
    convolve_cols_parallel(&temp, &mut output, width, height, &kernel);

    output
}

/// Convolve all rows in parallel.
fn convolve_rows_parallel(input: &[f32], output: &mut [f32], width: usize, kernel: &[f32]) {
    let radius = kernel.len() / 2;

    output
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &input[y * width..(y + 1) * width];
            convolve_row(in_row, out_row, kernel, radius);
        });
}

/// Convolve a single row with 1D kernel.
#[inline]
fn convolve_row(input: &[f32], output: &mut [f32], kernel: &[f32], radius: usize) {
    let width = input.len();

    for (x, out) in output.iter_mut().enumerate() {
        let mut sum = 0.0f32;

        for (k, &kval) in kernel.iter().enumerate() {
            let sx = x as isize + k as isize - radius as isize;

            // Mirror boundary handling
            let sx = if sx < 0 {
                (-sx) as usize
            } else if sx >= width as isize {
                2 * width - 2 - sx as usize
            } else {
                sx as usize
            };

            sum += input[sx] * kval;
        }

        *out = sum;
    }
}

/// Convolve all columns in parallel.
fn convolve_cols_parallel(
    input: &[f32],
    output: &mut [f32],
    width: usize,
    height: usize,
    kernel: &[f32],
) {
    let radius = kernel.len() / 2;

    // Process rows in parallel, each row convolves all columns vertically
    output
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out_row)| {
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
        });
}

/// Direct 2D Gaussian convolution for small images or large kernels.
fn gaussian_convolve_2d_direct(
    pixels: &[f32],
    width: usize,
    height: usize,
    sigma: f32,
) -> Vec<f32> {
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
                    let sx = if sx < 0 {
                        (-sx) as usize
                    } else if sx >= width as isize {
                        (2 * width - 2).saturating_sub(sx as usize)
                    } else {
                        sx as usize
                    };

                    let sy = if sy < 0 {
                        (-sy) as usize
                    } else if sy >= height as isize {
                        (2 * height - 2).saturating_sub(sy as usize)
                    } else {
                        sy as usize
                    };

                    sum += pixels[sy * width + sx] * kernel_2d[ky * ksize + kx];
                }
            }

            output[y * width + x] = sum;
        }
    }

    output
}

/// Apply matched filter convolution optimized for star detection.
///
/// This convolves the background-subtracted image with a Gaussian kernel
/// matching the expected PSF. The result is normalized to preserve flux
/// and can be directly thresholded.
///
/// # Arguments
/// * `pixels` - Input image data
/// * `width` - Image width
/// * `height` - Image height
/// * `background` - Per-pixel background values
/// * `fwhm` - Expected FWHM of stars in pixels
///
/// # Returns
/// Convolved, background-subtracted image
pub fn matched_filter(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &[f32],
    fwhm: f32,
) -> Vec<f32> {
    assert_eq!(pixels.len(), width * height);
    assert_eq!(background.len(), width * height);

    // Subtract background first
    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.iter())
        .map(|(&p, &b)| (p - b).max(0.0))
        .collect();

    // Convolve with Gaussian matching expected PSF
    let sigma = fwhm_to_sigma(fwhm);
    gaussian_convolve(&subtracted, width, height, sigma)
}
