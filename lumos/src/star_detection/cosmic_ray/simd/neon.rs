//! ARM NEON SIMD implementations for cosmic ray Laplacian computation.

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

/// Compute Laplacian for interior rows using NEON.
///
/// Processes 4 pixels at a time using the kernel:
/// ```text
///  0  1  0
///  1 -4  1
///  0  1  0
/// ```
///
/// # Safety
/// - `y` must be > 0 and < height - 1 (interior row).
/// - `width` must be >= 6 (4 SIMD + edges).
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub unsafe fn compute_laplacian_row_neon(
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

        let four = vdupq_n_f32(4.0);

        // Process 4 pixels at a time for interior
        let chunks = (width - 2) / 4;
        for i in 0..chunks {
            let x = 1 + i * 4;

            // Load center row values (shifted left and right)
            let left = vld1q_f32(ptr_curr.add(x - 1));
            let center = vld1q_f32(ptr_curr.add(x));
            let right = vld1q_f32(ptr_curr.add(x + 1));

            // Load above and below
            let above = vld1q_f32(ptr_above.add(x));
            let below = vld1q_f32(ptr_below.add(x));

            // Compute Laplacian: left + right + above + below - 4*center
            let sum_lr = vaddq_f32(left, right);
            let sum_ud = vaddq_f32(above, below);
            let sum_neighbors = vaddq_f32(sum_lr, sum_ud);
            let center_scaled = vmulq_f32(four, center);
            let laplacian = vsubq_f32(sum_neighbors, center_scaled);

            vst1q_f32(out_ptr.add(x), laplacian);
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
    #[allow(unused_imports)]
    use super::*;

    #[cfg(target_arch = "aarch64")]
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
    #[cfg(target_arch = "aarch64")]
    fn test_neon_laplacian_row() {
        let width = 32;
        let height = 10;
        let pixels: Vec<f32> = (0..width * height).map(|i| (i % 13) as f32 * 0.1).collect();

        let expected = compute_laplacian_scalar(&pixels, width, height);
        let mut output = vec![0.0f32; pixels.len()];

        // Test interior rows
        for y in 1..height - 1 {
            unsafe {
                compute_laplacian_row_neon(&pixels, width, y, &mut output);
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
}
