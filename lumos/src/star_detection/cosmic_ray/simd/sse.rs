//! SSE4.1 and AVX2 SIMD implementations for cosmic ray Laplacian computation.

#![allow(clippy::needless_range_loop)]

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// Compute Laplacian for interior rows using AVX2.
///
/// Processes 8 pixels at a time using the kernel:
/// ```text
///  0  1  0
///  1 -4  1
///  0  1  0
/// ```
///
/// # Safety
/// - Caller must ensure AVX2 is available.
/// - `y` must be > 0 and < height - 1 (interior row).
/// - `width` must be >= 10 (8 SIMD + edges).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn compute_laplacian_row_avx2(
    pixels: &[f32],
    width: usize,
    y: usize,
    output: &mut [f32],
) {
    unsafe {
        let row_above = (y - 1) * width;
        let row_curr = y * width;
        let row_below = (y + 1) * width;

        let ptr_above = pixels.as_ptr().add(row_above);
        let ptr_curr = pixels.as_ptr().add(row_curr);
        let ptr_below = pixels.as_ptr().add(row_below);
        let out_ptr = output.as_mut_ptr().add(row_curr);

        let four = _mm256_set1_ps(4.0);

        // Process 8 pixels at a time for interior
        let chunks = (width - 2) / 8;
        for i in 0..chunks {
            let x = 1 + i * 8;

            // Load center row values (shifted left and right)
            let left = _mm256_loadu_ps(ptr_curr.add(x - 1));
            let center = _mm256_loadu_ps(ptr_curr.add(x));
            let right = _mm256_loadu_ps(ptr_curr.add(x + 1));

            // Load above and below
            let above = _mm256_loadu_ps(ptr_above.add(x));
            let below = _mm256_loadu_ps(ptr_below.add(x));

            // Compute Laplacian: left + right + above + below - 4*center
            let sum_neighbors =
                _mm256_add_ps(_mm256_add_ps(left, right), _mm256_add_ps(above, below));
            let laplacian = _mm256_sub_ps(sum_neighbors, _mm256_mul_ps(four, center));

            _mm256_storeu_ps(out_ptr.add(x), laplacian);
        }

        // Handle remainder pixels (between last SIMD chunk and right edge)
        let remainder_start = 1 + chunks * 8;
        for x in remainder_start..(width - 1) {
            let idx = row_curr + x;
            let left = pixels[idx - 1];
            let center = pixels[idx];
            let right = pixels[idx + 1];
            let above = pixels[idx - width];
            let below = pixels[idx + width];
            output[idx] = left + right + above + below - 4.0 * center;
        }
    }
}

/// Compute Laplacian for interior rows using SSE4.1.
///
/// Processes 4 pixels at a time.
///
/// # Safety
/// - Caller must ensure SSE4.1 is available.
/// - `y` must be > 0 and < height - 1 (interior row).
/// - `width` must be >= 6 (4 SIMD + edges).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
pub unsafe fn compute_laplacian_row_sse41(
    pixels: &[f32],
    width: usize,
    y: usize,
    output: &mut [f32],
) {
    unsafe {
        let row_above = (y - 1) * width;
        let row_curr = y * width;
        let row_below = (y + 1) * width;

        let ptr_above = pixels.as_ptr().add(row_above);
        let ptr_curr = pixels.as_ptr().add(row_curr);
        let ptr_below = pixels.as_ptr().add(row_below);
        let out_ptr = output.as_mut_ptr().add(row_curr);

        let four = _mm_set1_ps(4.0);

        // Process 4 pixels at a time for interior
        let chunks = (width - 2) / 4;
        for i in 0..chunks {
            let x = 1 + i * 4;

            // Load center row values (shifted left and right)
            let left = _mm_loadu_ps(ptr_curr.add(x - 1));
            let center = _mm_loadu_ps(ptr_curr.add(x));
            let right = _mm_loadu_ps(ptr_curr.add(x + 1));

            // Load above and below
            let above = _mm_loadu_ps(ptr_above.add(x));
            let below = _mm_loadu_ps(ptr_below.add(x));

            // Compute Laplacian: left + right + above + below - 4*center
            let sum_neighbors = _mm_add_ps(_mm_add_ps(left, right), _mm_add_ps(above, below));
            let laplacian = _mm_sub_ps(sum_neighbors, _mm_mul_ps(four, center));

            _mm_storeu_ps(out_ptr.add(x), laplacian);
        }

        // Handle remainder pixels
        let remainder_start = 1 + chunks * 4;
        for x in remainder_start..(width - 1) {
            let idx = row_curr + x;
            let left = pixels[idx - 1];
            let center = pixels[idx];
            let right = pixels[idx + 1];
            let above = pixels[idx - width];
            let below = pixels[idx + width];
            output[idx] = left + right + above + below - 4.0 * center;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compute_laplacian_scalar(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
        let mut output = vec![0.0f32; pixels.len()];
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let idx = y * width + x;
                let left = pixels[idx - 1];
                let center = pixels[idx];
                let right = pixels[idx + 1];
                let above = pixels[idx - width];
                let below = pixels[idx + width];
                output[idx] = left + right + above + below - 4.0 * center;
            }
        }
        output
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx2_laplacian_row() {
        if !is_x86_feature_detected!("avx2") {
            eprintln!("Skipping AVX2 test - not available");
            return;
        }

        let width = 64;
        let height = 10;
        let pixels: Vec<f32> = (0..width * height).map(|i| (i % 17) as f32 * 0.1).collect();

        let expected = compute_laplacian_scalar(&pixels, width, height);
        let mut output = vec![0.0f32; pixels.len()];

        // Test interior rows
        for y in 1..height - 1 {
            unsafe {
                compute_laplacian_row_avx2(&pixels, width, y, &mut output);
            }
        }

        // Compare interior pixels
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let idx = y * width + x;
                assert!(
                    (output[idx] - expected[idx]).abs() < 1e-5,
                    "Mismatch at ({}, {}): {} vs {}",
                    x,
                    y,
                    output[idx],
                    expected[idx]
                );
            }
        }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_sse41_laplacian_row() {
        if !is_x86_feature_detected!("sse4.1") {
            eprintln!("Skipping SSE4.1 test - not available");
            return;
        }

        let width = 32;
        let height = 10;
        let pixels: Vec<f32> = (0..width * height).map(|i| (i % 13) as f32 * 0.1).collect();

        let expected = compute_laplacian_scalar(&pixels, width, height);
        let mut output = vec![0.0f32; pixels.len()];

        // Test interior rows
        for y in 1..height - 1 {
            unsafe {
                compute_laplacian_row_sse41(&pixels, width, y, &mut output);
            }
        }

        // Compare interior pixels
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let idx = y * width + x;
                assert!(
                    (output[idx] - expected[idx]).abs() < 1e-5,
                    "Mismatch at ({}, {}): {} vs {}",
                    x,
                    y,
                    output[idx],
                    expected[idx]
                );
            }
        }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_simd_laplacian_peak_detection() {
        if !is_x86_feature_detected!("avx2") {
            eprintln!("Skipping peak detection test - AVX2 not available");
            return;
        }

        // Create image with a sharp peak (cosmic ray-like)
        let width = 32;
        let height = 10;
        let mut pixels = vec![0.1f32; width * height];
        pixels[5 * width + 16] = 1.0; // Sharp peak

        let mut output = vec![0.0f32; pixels.len()];
        for y in 1..height - 1 {
            unsafe {
                compute_laplacian_row_avx2(&pixels, width, y, &mut output);
            }
        }

        // Peak should have strongly negative Laplacian
        let peak_laplacian = output[5 * width + 16];
        assert!(
            peak_laplacian < -3.0,
            "Peak Laplacian should be strongly negative: {}",
            peak_laplacian
        );

        // Neighbors should have positive Laplacian
        assert!(
            output[5 * width + 15] > 0.0,
            "Left neighbor should be positive"
        );
        assert!(
            output[5 * width + 17] > 0.0,
            "Right neighbor should be positive"
        );
        assert!(
            output[4 * width + 16] > 0.0,
            "Above neighbor should be positive"
        );
        assert!(
            output[6 * width + 16] > 0.0,
            "Below neighbor should be positive"
        );
    }
}
