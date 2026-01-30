//! SIMD-accelerated Laplacian computation for cosmic ray detection.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE4.1 on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

use crate::common::Buffer2;

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

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
pub fn compute_laplacian_simd(pixels: &Buffer2<f32>) -> Buffer2<f32> {
    let width = pixels.width();
    let height = pixels.height();
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

    Buffer2::new(width, height, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compute_laplacian_reference(pixels: &Buffer2<f32>) -> Buffer2<f32> {
        let width = pixels.width();
        let height = pixels.height();
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
        Buffer2::new(width, height, output)
    }

    #[test]
    fn test_compute_laplacian_simd_matches_reference() {
        let width = 64;
        let height = 32;
        let pixels = Buffer2::new(
            width,
            height,
            (0..width * height)
                .map(|i| ((i * 7) % 100) as f32 * 0.01)
                .collect(),
        );

        let expected = compute_laplacian_reference(&pixels);
        let result = compute_laplacian_simd(&pixels);

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
        let pixels = Buffer2::new_filled(width, height, 0.5f32);

        let result = compute_laplacian_simd(&pixels);

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
        let mut pixels_data = vec![0.1f32; width * height];
        pixels_data[8 * width + 16] = 1.0; // Sharp peak
        let pixels = Buffer2::new(width, height, pixels_data);

        let result = compute_laplacian_simd(&pixels);

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

    // ========== Comprehensive SIMD vs Scalar tests ==========

    #[test]
    fn test_compute_laplacian_simd_small_image() {
        // Test images smaller than SIMD width threshold
        for width in 4..16 {
            for height in 3..8 {
                let pixels = Buffer2::new(
                    width,
                    height,
                    (0..width * height)
                        .map(|i| (i as f32 * 0.1).sin() + 1.0)
                        .collect(),
                );

                let expected = compute_laplacian_reference(&pixels);
                let result = compute_laplacian_simd(&pixels);

                for y in 0..height {
                    for x in 0..width {
                        let idx = y * width + x;
                        assert!(
                            (result[idx] - expected[idx]).abs() < 1e-5,
                            "Small image {}x{} mismatch at ({}, {}): {} vs {}",
                            width,
                            height,
                            x,
                            y,
                            result[idx],
                            expected[idx]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_compute_laplacian_simd_edge_rows() {
        let width = 64;
        let height = 32;
        let pixels = Buffer2::new(
            width,
            height,
            (0..width * height)
                .map(|i| (i as f32 * 0.05).cos() * 10.0 + 50.0)
                .collect(),
        );

        let expected = compute_laplacian_reference(&pixels);
        let result = compute_laplacian_simd(&pixels);

        // Specifically check edge rows (y=0 and y=height-1)
        for x in 0..width {
            // Top row
            let idx_top = x;
            assert!(
                (result[idx_top] - expected[idx_top]).abs() < 1e-4,
                "Top edge mismatch at x={}: {} vs {}",
                x,
                result[idx_top],
                expected[idx_top]
            );

            // Bottom row
            let idx_bottom = (height - 1) * width + x;
            assert!(
                (result[idx_bottom] - expected[idx_bottom]).abs() < 1e-4,
                "Bottom edge mismatch at x={}: {} vs {}",
                x,
                result[idx_bottom],
                expected[idx_bottom]
            );
        }
    }

    #[test]
    fn test_compute_laplacian_simd_edge_columns() {
        let width = 64;
        let height = 32;
        let pixels = Buffer2::new(
            width,
            height,
            (0..width * height)
                .map(|i| (i as f32 * 0.03).sin() * 20.0 + 100.0)
                .collect(),
        );

        let expected = compute_laplacian_reference(&pixels);
        let result = compute_laplacian_simd(&pixels);

        // Specifically check edge columns (x=0 and x=width-1)
        for y in 0..height {
            // Left column
            let idx_left = y * width;
            assert!(
                (result[idx_left] - expected[idx_left]).abs() < 1e-4,
                "Left edge mismatch at y={}: {} vs {}",
                y,
                result[idx_left],
                expected[idx_left]
            );

            // Right column
            let idx_right = y * width + width - 1;
            assert!(
                (result[idx_right] - expected[idx_right]).abs() < 1e-4,
                "Right edge mismatch at y={}: {} vs {}",
                y,
                result[idx_right],
                expected[idx_right]
            );
        }
    }

    #[test]
    fn test_compute_laplacian_simd_corners() {
        let width = 64;
        let height = 32;
        let pixels = Buffer2::new(
            width,
            height,
            (0..width * height).map(|i| (i % 50) as f32 * 0.1).collect(),
        );

        let expected = compute_laplacian_reference(&pixels);
        let result = compute_laplacian_simd(&pixels);

        // Check all four corners
        let corners = [
            (0, 0),                  // top-left
            (width - 1, 0),          // top-right
            (0, height - 1),         // bottom-left
            (width - 1, height - 1), // bottom-right
        ];

        for (x, y) in corners {
            let idx = y * width + x;
            assert!(
                (result[idx] - expected[idx]).abs() < 1e-4,
                "Corner ({}, {}) mismatch: {} vs {}",
                x,
                y,
                result[idx],
                expected[idx]
            );
        }
    }

    #[test]
    fn test_compute_laplacian_simd_negative_values() {
        let width = 48;
        let height = 24;
        let pixels = Buffer2::new(
            width,
            height,
            (0..width * height)
                .map(|i| (i as f32 * 0.2).sin() * 50.0) // Mix of positive and negative
                .collect(),
        );

        let expected = compute_laplacian_reference(&pixels);
        let result = compute_laplacian_simd(&pixels);

        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                assert!(
                    (result[idx] - expected[idx]).abs() < 1e-4,
                    "Negative values mismatch at ({}, {}): {} vs {}",
                    x,
                    y,
                    result[idx],
                    expected[idx]
                );
            }
        }
    }

    #[test]
    fn test_compute_laplacian_simd_large_image() {
        let width = 256;
        let height = 128;
        let pixels = Buffer2::new(
            width,
            height,
            (0..width * height)
                .map(|i| ((i * 13) % 256) as f32 * 0.01)
                .collect(),
        );

        let expected = compute_laplacian_reference(&pixels);
        let result = compute_laplacian_simd(&pixels);

        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                assert!(
                    (result[idx] - expected[idx]).abs() < 1e-4,
                    "Large image mismatch at ({}, {}): {} vs {}",
                    x,
                    y,
                    result[idx],
                    expected[idx]
                );
            }
        }
    }

    #[test]
    fn test_compute_laplacian_simd_multiple_peaks() {
        let width = 64;
        let height = 32;
        let mut pixels_data = vec![0.1f32; width * height];

        // Add multiple sharp peaks
        let peaks = [(10, 8), (30, 12), (50, 20), (20, 25)];
        for (px, py) in peaks {
            pixels_data[py * width + px] = 1.0;
        }
        let pixels = Buffer2::new(width, height, pixels_data);

        let expected = compute_laplacian_reference(&pixels);
        let result = compute_laplacian_simd(&pixels);

        // Check all pixels match
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                assert!(
                    (result[idx] - expected[idx]).abs() < 1e-5,
                    "Multiple peaks mismatch at ({}, {}): {} vs {}",
                    x,
                    y,
                    result[idx],
                    expected[idx]
                );
            }
        }

        // Verify peaks have negative Laplacian
        for (px, py) in peaks {
            let idx = py * width + px;
            assert!(
                result[idx] < -3.0,
                "Peak at ({}, {}) should be negative: {}",
                px,
                py,
                result[idx]
            );
        }
    }

    #[test]
    fn test_compute_laplacian_simd_gradient() {
        // Test with a linear gradient (should have small Laplacian)
        let width = 64;
        let height = 32;
        let pixels = Buffer2::new(
            width,
            height,
            (0..width * height)
                .map(|i| {
                    let x = i % width;
                    let y = i / width;
                    (x as f32 * 0.1) + (y as f32 * 0.2)
                })
                .collect(),
        );

        let expected = compute_laplacian_reference(&pixels);
        let result = compute_laplacian_simd(&pixels);

        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                assert!(
                    (result[idx] - expected[idx]).abs() < 1e-4,
                    "Gradient mismatch at ({}, {}): {} vs {}",
                    x,
                    y,
                    result[idx],
                    expected[idx]
                );
            }
        }

        // Interior pixels of linear gradient should have near-zero Laplacian
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let idx = y * width + x;
                assert!(
                    result[idx].abs() < 1e-4,
                    "Linear gradient interior should be ~0: ({}, {}) = {}",
                    x,
                    y,
                    result[idx]
                );
            }
        }
    }

    #[test]
    fn test_compute_laplacian_row_simd_matches_scalar() {
        let width = 64;
        let height = 10;
        let pixels: Vec<f32> = (0..width * height)
            .map(|i| (i as f32 * 0.17).sin() * 30.0 + 50.0)
            .collect();

        // Test each interior row
        for y in 1..height - 1 {
            let mut output_simd = vec![0.0f32; pixels.len()];
            let mut output_scalar = vec![0.0f32; pixels.len()];

            compute_laplacian_row_simd(&pixels, width, y, &mut output_simd);
            compute_laplacian_row_scalar(&pixels, width, y, &mut output_scalar);

            // Check interior pixels of this row
            for x in 1..width - 1 {
                let idx = y * width + x;
                assert!(
                    (output_simd[idx] - output_scalar[idx]).abs() < 1e-4,
                    "Row {} mismatch at x={}: {} vs {}",
                    y,
                    x,
                    output_simd[idx],
                    output_scalar[idx]
                );
            }
        }
    }

    #[test]
    fn test_compute_laplacian_simd_various_widths() {
        // Test widths that exercise different SIMD paths
        let widths = [6, 7, 8, 9, 10, 11, 12, 15, 16, 17, 31, 32, 33, 63, 64, 65];
        let height = 10;

        for width in widths {
            let pixels = Buffer2::new(
                width,
                height,
                (0..width * height)
                    .map(|i| (i as f32 * 0.13).cos() * 25.0 + 50.0)
                    .collect(),
            );

            let expected = compute_laplacian_reference(&pixels);
            let result = compute_laplacian_simd(&pixels);

            for y in 0..height {
                for x in 0..width {
                    let idx = y * width + x;
                    assert!(
                        (result[idx] - expected[idx]).abs() < 1e-4,
                        "Width {} mismatch at ({}, {}): {} vs {}",
                        width,
                        x,
                        y,
                        result[idx],
                        expected[idx]
                    );
                }
            }
        }
    }
}
