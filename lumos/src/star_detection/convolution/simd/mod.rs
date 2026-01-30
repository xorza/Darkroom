//! SIMD-accelerated convolution implementations.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;

#[cfg(test)]
mod tests;

/// Convolve a single row with 1D kernel using SIMD when available.
///
/// Falls back to scalar implementation on unsupported platforms.
#[inline]
pub(super) fn convolve_row(input: &[f32], output: &mut [f32], kernel: &[f32], radius: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            unsafe {
                sse::convolve_row_avx2(input, output, kernel, radius);
            }
            return;
        }
        if cpu_features::has_sse4_1() {
            unsafe {
                sse::convolve_row_sse41(input, output, kernel, radius);
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            neon::convolve_row_neon(input, output, kernel, radius);
        }
        return;
    }

    // Scalar fallback
    #[allow(unreachable_code)]
    convolve_row_scalar(input, output, kernel, radius);
}

/// Scalar implementation of row convolution.
#[inline]
pub(super) fn convolve_row_scalar(
    input: &[f32],
    output: &mut [f32],
    kernel: &[f32],
    radius: usize,
) {
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
