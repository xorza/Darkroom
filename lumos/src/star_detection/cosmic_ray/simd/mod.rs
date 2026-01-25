//! SIMD-accelerated Laplacian computation for cosmic ray detection.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE4.1 on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

#[cfg(target_arch = "x86_64")]
use crate::common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;

/// Compute Laplacian for an interior row using SIMD.
///
/// This function dispatches to the best available SIMD implementation at runtime.
///
/// # Safety Requirements
/// - `y` must be > 0 and < height - 1 (interior row).
/// - `width` must be >= 6 for SSE or >= 10 for AVX2.
/// - `output` must have the same length as `pixels`.
///
/// # Arguments
/// * `pixels` - Input image data
/// * `width` - Image width
/// * `y` - Row index to process
/// * `output` - Output buffer (same size as pixels)
#[inline]
pub fn compute_laplacian_row_simd(pixels: &[f32], width: usize, y: usize, output: &mut [f32]) {
    #[cfg(target_arch = "x86_64")]
    {
        if width >= 10 && cpu_features::has_avx2() {
            unsafe {
                sse::compute_laplacian_row_avx2(pixels, width, y, output);
            }
            return;
        }
        if width >= 6 && cpu_features::has_sse4_1() {
            unsafe {
                sse::compute_laplacian_row_sse41(pixels, width, y, output);
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if width >= 6 {
            unsafe {
                neon::compute_laplacian_row_neon(pixels, width, y, output);
            }
            return;
        }
    }

    // Scalar fallback
    #[allow(unreachable_code)]
    compute_laplacian_row_scalar(pixels, width, y, output);
}

/// Scalar implementation of Laplacian row computation.
#[inline]
fn compute_laplacian_row_scalar(pixels: &[f32], width: usize, y: usize, output: &mut [f32]) {
    let row_curr = y * width;
    for x in 1..width - 1 {
        let idx = row_curr + x;
        let left = pixels[idx - 1];
        let center = pixels[idx];
        let right = pixels[idx + 1];
        let above = pixels[idx - width];
        let below = pixels[idx + width];
        output[idx] = left + right + above + below - 4.0 * center;
    }
}

/// Compute full image Laplacian using SIMD acceleration.
///
/// Handles edge pixels with clamping, uses SIMD for interior rows.
pub fn compute_laplacian_simd(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut output = vec![0.0f32; pixels.len()];

    // Handle edge rows with scalar code (clamping)
    for y in 0..height {
        if y == 0 || y == height - 1 {
            // Edge row - use clamping
            for x in 0..width {
                let idx = y * width + x;
                let left = if x > 0 { pixels[idx - 1] } else { pixels[idx] };
                let right = if x + 1 < width {
                    pixels[idx + 1]
                } else {
                    pixels[idx]
                };
                let above = if y > 0 {
                    pixels[idx - width]
                } else {
                    pixels[idx]
                };
                let below = if y + 1 < height {
                    pixels[idx + width]
                } else {
                    pixels[idx]
                };
                output[idx] = left + right + above + below - 4.0 * pixels[idx];
            }
        } else {
            // Interior row - edge pixels first
            let row_curr = y * width;

            // Left edge (x=0)
            let idx = row_curr;
            output[idx] = pixels[idx] + pixels[idx + 1] + pixels[idx - width] + pixels[idx + width]
                - 4.0 * pixels[idx];

            // Use SIMD for interior
            compute_laplacian_row_simd(pixels, width, y, &mut output);

            // Right edge (x=width-1)
            let idx = row_curr + width - 1;
            output[idx] = pixels[idx - 1] + pixels[idx] + pixels[idx - width] + pixels[idx + width]
                - 4.0 * pixels[idx];
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compute_laplacian_reference(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
        let mut output = vec![0.0f32; pixels.len()];
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                let left = if x > 0 { pixels[idx - 1] } else { pixels[idx] };
                let right = if x + 1 < width {
                    pixels[idx + 1]
                } else {
                    pixels[idx]
                };
                let above = if y > 0 {
                    pixels[idx - width]
                } else {
                    pixels[idx]
                };
                let below = if y + 1 < height {
                    pixels[idx + width]
                } else {
                    pixels[idx]
                };
                output[idx] = left + right + above + below - 4.0 * pixels[idx];
            }
        }
        output
    }

    #[test]
    fn test_compute_laplacian_simd_matches_reference() {
        let width = 64;
        let height = 32;
        let pixels: Vec<f32> = (0..width * height)
            .map(|i| ((i * 7) % 100) as f32 * 0.01)
            .collect();

        let expected = compute_laplacian_reference(&pixels, width, height);
        let result = compute_laplacian_simd(&pixels, width, height);

        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                assert!(
                    (result[idx] - expected[idx]).abs() < 1e-5,
                    "Mismatch at ({}, {}): {} vs {}",
                    x,
                    y,
                    result[idx],
                    expected[idx]
                );
            }
        }
    }

    #[test]
    fn test_compute_laplacian_simd_flat_image() {
        let width = 32;
        let height = 16;
        let pixels = vec![0.5f32; width * height];

        let result = compute_laplacian_simd(&pixels, width, height);

        // Interior pixels should have zero Laplacian for flat image
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let idx = y * width + x;
                assert!(
                    result[idx].abs() < 1e-6,
                    "Interior Laplacian should be 0: ({}, {}) = {}",
                    x,
                    y,
                    result[idx]
                );
            }
        }
    }

    #[test]
    fn test_compute_laplacian_simd_sharp_peak() {
        let width = 32;
        let height = 16;
        let mut pixels = vec![0.1f32; width * height];
        pixels[8 * width + 16] = 1.0; // Sharp peak

        let result = compute_laplacian_simd(&pixels, width, height);

        // Peak should have strongly negative Laplacian
        let peak_laplacian = result[8 * width + 16];
        assert!(
            peak_laplacian < -3.0,
            "Peak Laplacian should be strongly negative: {}",
            peak_laplacian
        );
    }

    #[test]
    fn test_compute_laplacian_row_scalar() {
        let width = 16;
        let height = 5;
        let pixels: Vec<f32> = (0..width * height).map(|i| (i % 10) as f32 * 0.1).collect();

        let mut output = vec![0.0f32; pixels.len()];
        compute_laplacian_row_scalar(&pixels, width, 2, &mut output);

        // Verify a specific interior pixel
        let x = 5;
        let y = 2;
        let idx = y * width + x;
        let expected =
            pixels[idx - 1] + pixels[idx + 1] + pixels[idx - width] + pixels[idx + width]
                - 4.0 * pixels[idx];
        assert!(
            (output[idx] - expected).abs() < 1e-6,
            "Scalar Laplacian mismatch: {} vs {}",
            output[idx],
            expected
        );
    }
}
