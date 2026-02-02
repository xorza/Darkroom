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
///
/// For indices far out of bounds, clamps to valid range after reflection.
#[inline]
pub fn mirror_index(i: isize, len: usize) -> usize {
    debug_assert!(len > 0, "mirror_index requires len > 0");

    if i < 0 {
        let reflected = (-i) as usize;
        // Clamp to valid range if reflected index is still out of bounds
        reflected.min(len - 1)
    } else if i >= len as isize {
        let reflected = (2 * len).saturating_sub(2).saturating_sub(i as usize);
        // Clamp to valid range
        reflected.min(len - 1)
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

// ============================================================================
// Column convolution dispatch
// ============================================================================

/// Convolve columns using direct SIMD when available.
///
/// Falls back to scalar implementation on unsupported platforms.
#[inline]
pub(super) fn convolve_cols_direct(
    input: &[f32],
    output: &mut [f32],
    width: usize,
    height: usize,
    kernel: &[f32],
    radius: usize,
) {
    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            unsafe {
                sse::convolve_cols_avx2(input, output, width, height, kernel, radius);
            }
            return;
        }
        if cpu_features::has_sse4_1() {
            unsafe {
                sse::convolve_cols_sse41(input, output, width, height, kernel, radius);
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            neon::convolve_cols_neon(input, output, width, height, kernel, radius);
        }
        return;
    }

    // Scalar fallback
    #[allow(unreachable_code)]
    convolve_cols_scalar(input, output, width, height, kernel, radius);
}

/// Scalar implementation of column convolution.
#[inline]
pub(super) fn convolve_cols_scalar(
    input: &[f32],
    output: &mut [f32],
    width: usize,
    height: usize,
    kernel: &[f32],
    radius: usize,
) {
    for x in 0..width {
        for y in 0..height {
            let mut sum = 0.0f32;
            for (k, &kval) in kernel.iter().enumerate() {
                let sy = y as isize + k as isize - radius as isize;
                let sy = mirror_index(sy, height);
                sum += input[sy * width + x] * kval;
            }
            output[y * width + x] = sum;
        }
    }
}

/// Convolve a single row using 2D convolution with SIMD.
///
/// This processes one output row at a given y coordinate.
#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn convolve_2d_row(
    input: &[f32],
    output_row: &mut [f32],
    width: usize,
    height: usize,
    y: usize,
    kernel: &[f32],
    ksize: usize,
    radius: usize,
) {
    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            unsafe {
                sse::convolve_2d_row_avx2(
                    input, output_row, width, height, y, kernel, ksize, radius,
                );
            }
            return;
        }
        if cpu_features::has_sse4_1() {
            unsafe {
                sse::convolve_2d_row_sse41(
                    input, output_row, width, height, y, kernel, ksize, radius,
                );
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            neon::convolve_2d_row_neon(input, output_row, width, height, y, kernel, ksize, radius);
        }
        return;
    }

    // Scalar fallback
    #[allow(unreachable_code)]
    convolve_2d_row_scalar(input, output_row, width, height, y, kernel, ksize, radius);
}

/// Scalar implementation of single-row 2D convolution.
#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn convolve_2d_row_scalar(
    input: &[f32],
    output_row: &mut [f32],
    width: usize,
    height: usize,
    y: usize,
    kernel: &[f32],
    ksize: usize,
    radius: usize,
) {
    for (x, out_px) in output_row.iter_mut().enumerate() {
        let mut sum = 0.0f32;
        for ky in 0..ksize {
            let sy = y as isize + ky as isize - radius as isize;
            let sy = mirror_index(sy, height);
            for kx in 0..ksize {
                let sx = x as isize + kx as isize - radius as isize;
                let sx = mirror_index(sx, width);
                sum += input[sy * width + sx] * kernel[ky * ksize + kx];
            }
        }
        *out_px = sum;
    }
}
