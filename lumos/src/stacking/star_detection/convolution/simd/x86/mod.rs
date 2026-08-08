//! SSE4.1 and AVX2 implementations of row convolution.
//!
//! These implementations use SIMD intrinsics to process multiple pixels
//! in parallel, achieving 4-8× speedup over scalar code.

// Allow indexed loops - necessary for SIMD code patterns where we need
// explicit index control for pointer arithmetic
#![allow(clippy::needless_range_loop)]

use std::arch::x86_64::*;

use crate::math::size2us::Size2us;
use crate::stacking::star_detection::convolution::simd::{Kernel2d, convolve_pixel_scalar};

/// Convolve a row using AVX2 + FMA intrinsics.
///
/// Processes 8 pixels at a time using 256-bit vectors.
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available (use `is_x86_feature_detected!`).
#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn convolve_row_avx2(
    input: &[f32],
    output: &mut [f32],
    kernel: &[f32],
    radius: usize,
) {
    unsafe {
        let width = input.len();

        // For small inputs, just use scalar
        if width < 16 + 2 * radius {
            for x in 0..width {
                output[x] = convolve_pixel_scalar(input, kernel, radius, x, width);
            }
            return;
        }

        // Process 8 pixels at a time in the middle section. A column is SIMD-safe only if its whole
        // kernel window stays in bounds (the interior does no mirroring). The widest source read for
        // the 8-wide block at x is `(x + 7) + (kernel.len() - 1) - radius`; requiring it `<= width-1`
        // gives the bound below. Derived from `kernel.len()` rather than assuming the symmetric
        // `2*radius+1`, so the SIMD interior matches the scalar mirror reference for any kernel.
        let safe_start = radius;
        let safe_end = (width + radius + 1).saturating_sub(8 + kernel.len());

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
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn convolve_row_sse41(
    input: &[f32],
    output: &mut [f32],
    kernel: &[f32],
    radius: usize,
) {
    unsafe {
        let width = input.len();

        // For small inputs, just use scalar
        if width < 8 + 2 * radius {
            for x in 0..width {
                output[x] = convolve_pixel_scalar(input, kernel, radius, x, width);
            }
            return;
        }

        // Process 4 pixels at a time in the middle section. A column is SIMD-safe only if its whole
        // kernel window stays in bounds (the interior does no mirroring). The widest source read for
        // the 4-wide block at x is `(x + 3) + (kernel.len() - 1) - radius`; requiring it `<= width-1`
        // gives the bound below — via `kernel.len()` (not the symmetric `2*radius+1`) so the SIMD
        // interior matches the scalar mirror reference for any kernel.
        let safe_start = radius;
        let safe_end = (width + radius + 1).saturating_sub(4 + kernel.len());

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

/// Convolve one output column-row `y` (8 columns at a time, AVX2+FMA).
///
/// The production column pass calls this per row across rayon workers; `out_row` is the single
/// output row (length `width`), `y` its absolute row index for mirror-edge input addressing.
///
/// # Safety
/// Caller must ensure AVX2+FMA is available.
#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn convolve_cols_row_avx2(
    input: &[f32],
    out_row: &mut [f32],
    size: Size2us,
    y: usize,
    kernel: &[f32],
    radius: usize,
) {
    unsafe {
        use crate::stacking::star_detection::convolution::simd::mirror_index;

        let mut x = 0;
        while x + 8 <= size.width {
            let mut sum = _mm256_setzero_ps();
            for (k, &kval) in kernel.iter().enumerate() {
                let sy = mirror_index(y as isize + k as isize - radius as isize, size.height);
                let vals = _mm256_loadu_ps(input.as_ptr().add(sy * size.width + x));
                sum = _mm256_fmadd_ps(vals, _mm256_set1_ps(kval), sum);
            }
            _mm256_storeu_ps(out_row.as_mut_ptr().add(x), sum);
            x += 8;
        }

        while x < size.width {
            let mut sum = 0.0f32;
            for (k, &kval) in kernel.iter().enumerate() {
                let sy = mirror_index(y as isize + k as isize - radius as isize, size.height);
                sum += input[sy * size.width + x] * kval;
            }
            out_row[x] = sum;
            x += 1;
        }
    }
}

/// Convolve one output column-row `y` (4 columns at a time, SSE4.1). See
/// [`convolve_cols_row_avx2`] for the per-row contract.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn convolve_cols_row_sse41(
    input: &[f32],
    out_row: &mut [f32],
    size: Size2us,
    y: usize,
    kernel: &[f32],
    radius: usize,
) {
    unsafe {
        use crate::stacking::star_detection::convolution::simd::mirror_index;

        let mut x = 0;
        while x + 4 <= size.width {
            let mut sum = _mm_setzero_ps();
            for (k, &kval) in kernel.iter().enumerate() {
                let sy = mirror_index(y as isize + k as isize - radius as isize, size.height);
                let vals = _mm_loadu_ps(input.as_ptr().add(sy * size.width + x));
                sum = _mm_add_ps(sum, _mm_mul_ps(vals, _mm_set1_ps(kval)));
            }
            _mm_storeu_ps(out_row.as_mut_ptr().add(x), sum);
            x += 4;
        }

        while x < size.width {
            let mut sum = 0.0f32;
            for (k, &kval) in kernel.iter().enumerate() {
                let sy = mirror_index(y as isize + k as isize - radius as isize, size.height);
                sum += input[sy * size.width + x] * kval;
            }
            out_row[x] = sum;
            x += 1;
        }
    }
}

/// Apply 2D convolution to a single row using AVX2 intrinsics.
///
/// Processes 8 output pixels at a time.
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available.
#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn convolve_2d_row_avx2(
    input: &[f32],
    output_row: &mut [f32],
    size: Size2us,
    y: usize,
    kernel: Kernel2d,
) {
    unsafe {
        use crate::stacking::star_detection::convolution::simd::mirror_index;

        let radius = kernel.radius() as isize;

        // Process 8 output pixels at a time
        let mut x = 0;
        while x + 8 <= size.width {
            let mut sum = _mm256_setzero_ps();

            for ky in 0..kernel.size() {
                let sy = mirror_index(y as isize + ky as isize - radius, size.height);
                let input_row_offset = sy * size.width;

                for kx in 0..kernel.size() {
                    let kval = kernel.at(ky, kx);
                    if kval.abs() < 1e-10 {
                        continue;
                    }

                    let kv = _mm256_set1_ps(kval);
                    let base_sx = x as isize + kx as isize - radius;

                    if base_sx >= 0 && base_sx + 8 <= size.width as isize {
                        let vals = _mm256_loadu_ps(
                            input.as_ptr().add(input_row_offset + base_sx as usize),
                        );
                        sum = _mm256_fmadd_ps(vals, kv, sum);
                    } else {
                        let mut vals = [0.0f32; 8];
                        for i in 0..8 {
                            let sx = base_sx + i as isize;
                            let sx = mirror_index(sx, size.width);
                            vals[i] = input[input_row_offset + sx];
                        }
                        let vvals = _mm256_loadu_ps(vals.as_ptr());
                        sum = _mm256_fmadd_ps(vvals, kv, sum);
                    }
                }
            }

            _mm256_storeu_ps(output_row.as_mut_ptr().add(x), sum);
            x += 8;
        }

        // Handle remaining pixels with scalar
        while x < size.width {
            let mut sum = 0.0f32;
            for ky in 0..kernel.size() {
                let sy = mirror_index(y as isize + ky as isize - radius, size.height);
                for kx in 0..kernel.size() {
                    let sx = mirror_index(x as isize + kx as isize - radius, size.width);
                    sum += input[sy * size.width + sx] * kernel.at(ky, kx);
                }
            }
            output_row[x] = sum;
            x += 1;
        }
    }
}

/// Apply 2D convolution to a single row using SSE4.1 intrinsics.
///
/// Processes 4 output pixels at a time.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn convolve_2d_row_sse41(
    input: &[f32],
    output_row: &mut [f32],
    size: Size2us,
    y: usize,
    kernel: Kernel2d,
) {
    unsafe {
        use crate::stacking::star_detection::convolution::simd::mirror_index;

        let radius = kernel.radius() as isize;

        // Process 4 output pixels at a time
        let mut x = 0;
        while x + 4 <= size.width {
            let mut sum = _mm_setzero_ps();

            for ky in 0..kernel.size() {
                let sy = mirror_index(y as isize + ky as isize - radius, size.height);
                let input_row_offset = sy * size.width;

                for kx in 0..kernel.size() {
                    let kval = kernel.at(ky, kx);
                    if kval.abs() < 1e-10 {
                        continue;
                    }

                    let kv = _mm_set1_ps(kval);
                    let base_sx = x as isize + kx as isize - radius;

                    if base_sx >= 0 && base_sx + 4 <= size.width as isize {
                        let vals =
                            _mm_loadu_ps(input.as_ptr().add(input_row_offset + base_sx as usize));
                        sum = _mm_add_ps(sum, _mm_mul_ps(vals, kv));
                    } else {
                        let mut vals = [0.0f32; 4];
                        for i in 0..4 {
                            let sx = base_sx + i as isize;
                            let sx = mirror_index(sx, size.width);
                            vals[i] = input[input_row_offset + sx];
                        }
                        let vvals = _mm_loadu_ps(vals.as_ptr());
                        sum = _mm_add_ps(sum, _mm_mul_ps(vvals, kv));
                    }
                }
            }

            _mm_storeu_ps(output_row.as_mut_ptr().add(x), sum);
            x += 4;
        }

        // Handle remaining pixels with scalar
        while x < size.width {
            let mut sum = 0.0f32;
            for ky in 0..kernel.size() {
                let sy = mirror_index(y as isize + ky as isize - radius, size.height);
                for kx in 0..kernel.size() {
                    let sx = mirror_index(x as isize + kx as isize - radius, size.width);
                    sum += input[sy * size.width + sx] * kernel.at(ky, kx);
                }
            }
            output_row[x] = sum;
            x += 1;
        }
    }
}

#[cfg(test)]
mod tests;
