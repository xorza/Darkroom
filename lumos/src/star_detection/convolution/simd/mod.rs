//! SIMD-accelerated convolution implementations.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

use common::{cfg_aarch64, cfg_x86_64};

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

cfg_x86_64! {
    pub mod sse;
}

cfg_aarch64! {
    pub mod neon;
}

#[cfg(test)]
mod tests;

// ============================================================================
// Shared utilities
// ============================================================================

/// Mirror boundary handling for convolution.
///
/// Maps an index that may be out of bounds to a valid index using reflection.
/// For index < 0: reflects at 0 (e.g., -1 -> 1, -2 -> 2)
/// For index >= len: reflects at len-1 (e.g., len -> len-2, len+1 -> len-3)
#[inline]
pub fn mirror_index(i: isize, len: usize) -> usize {
    if i < 0 {
        (-i) as usize
    } else if i >= len as isize {
        2 * len - 2 - i as usize
    } else {
        i as usize
    }
}

/// Scalar convolution for a single pixel with mirror boundary handling.
#[inline]
pub fn convolve_pixel_scalar(
    input: &[f32],
    kernel: &[f32],
    radius: usize,
    x: usize,
    width: usize,
) -> f32 {
    let mut sum = 0.0f32;

    for (k, &kval) in kernel.iter().enumerate() {
        let sx = x as isize + k as isize - radius as isize;
        let sx = mirror_index(sx, width);
        sum += input[sx] * kval;
    }

    sum
}

// ============================================================================
// Row convolution dispatch
// ============================================================================

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
        *out = convolve_pixel_scalar(input, kernel, radius, x, width);
    }
}
