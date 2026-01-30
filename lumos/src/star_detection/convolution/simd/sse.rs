//! SSE4.1 and AVX2 implementations of row convolution.
//!
//! These implementations use SIMD intrinsics to process multiple pixels
//! in parallel, achieving 4-8× speedup over scalar code.

// Allow indexed loops - necessary for SIMD code patterns where we need
// explicit index control for pointer arithmetic
#![allow(clippy::needless_range_loop)]

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// Convolve a row using AVX2 + FMA intrinsics.
///
/// Processes 8 pixels at a time using 256-bit vectors.
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available (use `is_x86_feature_detected!`).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
pub unsafe fn convolve_row_avx2(input: &[f32], output: &mut [f32], kernel: &[f32], radius: usize) {
    unsafe {
        let width = input.len();

        // For small inputs, just use scalar
        if width < 16 + 2 * radius {
            for x in 0..width {
                output[x] = convolve_pixel_scalar(input, kernel, radius, x, width);
            }
            return;
        }

        // Process 8 pixels at a time in the middle section
        // Safe region: we can load 8 contiguous floats starting at (x - radius)
        // and ending at (x + 7 + radius) without boundary issues
        let safe_start = radius;
        let safe_end = width - radius - 7; // Last x where we can safely load

        // Handle left edge with scalar
        for x in 0..safe_start {
            output[x] = convolve_pixel_scalar(input, kernel, radius, x, width);
        }

        // SIMD middle section
        let mut x = safe_start;
        while x <= safe_end {
            let mut sum = _mm256_setzero_ps();

            for (k, &kval) in kernel.iter().enumerate() {
                let kv = _mm256_set1_ps(kval);
                let sx = x + k - radius;

                // Load 8 input values
                let vals = _mm256_loadu_ps(input.as_ptr().add(sx));

                // Multiply-accumulate
                sum = _mm256_fmadd_ps(vals, kv, sum);
            }

            // Store 8 output values
            _mm256_storeu_ps(output.as_mut_ptr().add(x), sum);
            x += 8;
        }

        // Handle right edge with scalar
        while x < width {
            output[x] = convolve_pixel_scalar(input, kernel, radius, x, width);
            x += 1;
        }
    }
}

/// Convolve a row using SSE4.1 intrinsics.
///
/// Processes 4 pixels at a time using 128-bit vectors.
///
/// # Safety
/// Caller must ensure SSE4.1 is available (use `is_x86_feature_detected!`).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
pub unsafe fn convolve_row_sse41(input: &[f32], output: &mut [f32], kernel: &[f32], radius: usize) {
    unsafe {
        let width = input.len();

        // For small inputs, just use scalar
        if width < 8 + 2 * radius {
            for x in 0..width {
                output[x] = convolve_pixel_scalar(input, kernel, radius, x, width);
            }
            return;
        }

        // Process 4 pixels at a time in the middle section
        // Safe region: we can load 4 contiguous floats starting at (x - radius)
        // and ending at (x + 3 + radius) without boundary issues
        let safe_start = radius;
        let safe_end = width - radius - 3; // Last x where we can safely load

        // Handle left edge with scalar
        for x in 0..safe_start {
            output[x] = convolve_pixel_scalar(input, kernel, radius, x, width);
        }

        // SIMD middle section
        let mut x = safe_start;
        while x <= safe_end {
            let mut sum = _mm_setzero_ps();

            for (k, &kval) in kernel.iter().enumerate() {
                let kv = _mm_set1_ps(kval);
                let sx = x + k - radius;

                // Load 4 input values
                let vals = _mm_loadu_ps(input.as_ptr().add(sx));

                // Multiply-accumulate (no FMA, so separate mul and add)
                sum = _mm_add_ps(sum, _mm_mul_ps(vals, kv));
            }

            // Store 4 output values
            _mm_storeu_ps(output.as_mut_ptr().add(x), sum);
            x += 4;
        }

        // Handle right edge with scalar
        while x < width {
            output[x] = convolve_pixel_scalar(input, kernel, radius, x, width);
            x += 1;
        }
    }
}

/// Scalar convolution for a single pixel with mirror boundary handling.
#[inline]
fn convolve_pixel_scalar(
    input: &[f32],
    kernel: &[f32],
    radius: usize,
    x: usize,
    width: usize,
) -> f32 {
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

    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_arch = "x86_64")]
    use common::cpu_features;

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx2_matches_scalar() {
        if !cpu_features::has_avx2_fma() {
            eprintln!("Skipping AVX2 test: CPU does not support AVX2+FMA");
            return;
        }

        let input: Vec<f32> = (0..256).map(|i| (i as f32).sin()).collect();
        let kernel = vec![0.05, 0.1, 0.2, 0.3, 0.2, 0.1, 0.05];
        let radius = 3;

        let mut output_avx2 = vec![0.0f32; 256];
        let mut output_scalar = vec![0.0f32; 256];

        unsafe {
            convolve_row_avx2(&input, &mut output_avx2, &kernel, radius);
        }

        for x in 0..256 {
            output_scalar[x] = convolve_pixel_scalar(&input, &kernel, radius, x, 256);
        }

        for i in 0..256 {
            assert!(
                (output_avx2[i] - output_scalar[i]).abs() < 1e-5,
                "AVX2 mismatch at {}: {} vs {}",
                i,
                output_avx2[i],
                output_scalar[i]
            );
        }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_sse41_matches_scalar() {
        if !cpu_features::has_sse4_1() {
            eprintln!("Skipping SSE4.1 test: CPU does not support SSE4.1");
            return;
        }

        let input: Vec<f32> = (0..256).map(|i| (i as f32).sin()).collect();
        let kernel = vec![0.05, 0.1, 0.2, 0.3, 0.2, 0.1, 0.05];
        let radius = 3;

        let mut output_sse = vec![0.0f32; 256];
        let mut output_scalar = vec![0.0f32; 256];

        unsafe {
            convolve_row_sse41(&input, &mut output_sse, &kernel, radius);
        }

        for x in 0..256 {
            output_scalar[x] = convolve_pixel_scalar(&input, &kernel, radius, x, 256);
        }

        for i in 0..256 {
            assert!(
                (output_sse[i] - output_scalar[i]).abs() < 1e-5,
                "SSE4.1 mismatch at {}: {} vs {}",
                i,
                output_sse[i],
                output_scalar[i]
            );
        }
    }
}
