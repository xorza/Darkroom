//! SIMD-accelerated convolution implementations.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

use rayon::prelude::*;

use crate::math::size2us::Size2us;
use crate::simd::dispatch;

#[cfg(target_arch = "x86_64")]
mod x86;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(test)]
mod tests;

/// A borrowed square 2D convolution kernel, stored row-major.
///
/// Bundles the coefficients with the side length they are laid out by, so the two
/// cannot disagree, and derives the centre offset every tap is measured from.
#[derive(Debug, Clone, Copy)]
pub(super) struct Kernel2d<'a> {
    weights: &'a [f32],
    size: usize,
}

impl<'a> Kernel2d<'a> {
    /// Panics if `weights` is not exactly `size × size` — a cold, once-per-image check,
    /// not per row or per pixel.
    pub(super) fn new(weights: &'a [f32], size: usize) -> Self {
        assert_eq!(
            weights.len(),
            size * size,
            "a {size}x{size} kernel needs {} weights",
            size * size
        );
        Self { weights, size }
    }

    /// Side length in taps.
    #[inline]
    pub(super) fn size(self) -> usize {
        self.size
    }

    /// Offset from the kernel's first tap to its centre.
    #[inline]
    pub(super) fn radius(self) -> usize {
        self.size / 2
    }

    #[inline]
    pub(super) fn at(self, ky: usize, kx: usize) -> f32 {
        self.weights[ky * self.size + kx]
    }
}

/// Mirror boundary handling for convolution.
///
/// Maps an index that may be out of bounds to a valid index using reflection.
/// For index < 0: reflects at 0 (e.g., -1 -> 1, -2 -> 2)
/// For index >= len: reflects at len-1 (e.g., len -> len-2, len+1 -> len-3)
///
/// For indices far out of bounds, clamps to valid range after reflection.
#[inline]
pub(super) fn mirror_index(i: isize, len: usize) -> usize {
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
        let sx = mirror_index(sx, width);
        sum += input[sx] * kval;
    }

    sum
}

/// Convolve a single row with 1D kernel using SIMD when available.
///
/// Falls back to scalar implementation on unsupported platforms.
#[inline]
pub(super) fn convolve_row(input: &[f32], output: &mut [f32], kernel: &[f32], radius: usize) {
    dispatch! {
        x86: avx2_fma => x86::convolve_row_avx2(input, output, kernel, radius),
        x86: sse4_1 => x86::convolve_row_sse41(input, output, kernel, radius),
        aarch64 => neon::convolve_row_neon(input, output, kernel, radius),
        scalar => convolve_row_scalar(input, output, kernel, radius),
    }
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

/// Column (vertical) convolution over the whole image. Each output row depends on input rows
/// `[y-radius, y+radius]` (mirror edges), so rows are independent and computed in parallel across
/// rayon workers — one row per chunk, SIMD across the columns within a row.
pub(super) fn convolve_cols_direct(
    input: &[f32],
    output: &mut [f32],
    size: Size2us,
    kernel: &[f32],
    radius: usize,
) {
    output
        .par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(y, out_row)| {
            convolve_cols_row(input, out_row, size, y, kernel, radius);
        });
}

/// Convolve one output column-row `y` with the 1D kernel (mirror edges), SIMD when available.
#[inline]
fn convolve_cols_row(
    input: &[f32],
    out_row: &mut [f32],
    size: Size2us,
    y: usize,
    kernel: &[f32],
    radius: usize,
) {
    dispatch! {
        x86: avx2_fma => x86::convolve_cols_row_avx2(input, out_row, size, y, kernel, radius),
        x86: sse4_1 => x86::convolve_cols_row_sse41(input, out_row, size, y, kernel, radius),
        aarch64 => neon::convolve_cols_row_neon(input, out_row, size, y, kernel, radius),
        scalar => convolve_cols_row_scalar(input, out_row, size, y, kernel, radius),
    }
}

/// Scalar single column-row convolution.
#[inline]
fn convolve_cols_row_scalar(
    input: &[f32],
    out_row: &mut [f32],
    size: Size2us,
    y: usize,
    kernel: &[f32],
    radius: usize,
) {
    for (x, out) in out_row.iter_mut().enumerate() {
        let mut sum = 0.0f32;
        for (k, &kval) in kernel.iter().enumerate() {
            let sy = mirror_index(y as isize + k as isize - radius as isize, size.height);
            sum += input[sy * size.width + x] * kval;
        }
        *out = sum;
    }
}

/// Convolve a single row using 2D convolution with SIMD.
///
/// This processes one output row at a given y coordinate.
#[inline]
pub(super) fn convolve_2d_row(
    input: &[f32],
    output_row: &mut [f32],
    size: Size2us,
    y: usize,
    kernel: Kernel2d,
) {
    dispatch! {
        x86: avx2_fma => x86::convolve_2d_row_avx2(input, output_row, size, y, kernel),
        x86: sse4_1 => x86::convolve_2d_row_sse41(input, output_row, size, y, kernel),
        aarch64 => neon::convolve_2d_row_neon(input, output_row, size, y, kernel),
        scalar => convolve_2d_row_scalar(input, output_row, size, y, kernel),
    }
}

/// Scalar implementation of single-row 2D convolution.
#[inline]
fn convolve_2d_row_scalar(
    input: &[f32],
    output_row: &mut [f32],
    size: Size2us,
    y: usize,
    kernel: Kernel2d,
) {
    let radius = kernel.radius() as isize;
    for (x, out_px) in output_row.iter_mut().enumerate() {
        let mut sum = 0.0f32;
        for ky in 0..kernel.size() {
            let sy = mirror_index(y as isize + ky as isize - radius, size.height);
            for kx in 0..kernel.size() {
                let sx = mirror_index(x as isize + kx as isize - radius, size.width);
                sum += input[sy * size.width + sx] * kernel.at(ky, kx);
            }
        }
        *out_px = sum;
    }
}
