//! AVX2 implementations of row convolution.
//!
//! Every lane accumulates its taps in the scalar path's order with an unfused multiply then add,
//! so each backend is bit-identical to the scalar reference: the threshold that reads the
//! filtered image is itself unfused to be exact at `px == threshold`, which a fused sum here
//! would undo.

#![expect(
    clippy::needless_range_loop,
    reason = "the edge columns index `output` beside pointer arithmetic on the same `x`"
)]

use std::arch::x86_64::*;

use crate::math::size2us::Size2us;
use crate::star_detection::convolution::simd::{Kernel2d, convolve_pixel_scalar};

/// Convolve a row using AVX2 intrinsics.
///
/// Processes 8 pixels at a time using 256-bit vectors.
///
/// # Safety
/// Caller must ensure AVX2 is available (use `is_x86_feature_detected!`).
#[target_feature(enable = "avx2")]
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
        // kernel window stays in bounds (the interior does no mirroring). The widest source read
        // for the 8-wide block at x is `(x + 7) + (kernel.len() - 1) - radius`; requiring it `<=
        // width-1` gives the bound below. Derived from `kernel.len()` rather than assuming the
        // symmetric `2*radius+1`, so the SIMD interior matches the scalar mirror reference for any
        // kernel.
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

                sum = _mm256_add_ps(sum, _mm256_mul_ps(vals, kv));
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

/// Convolve one output column-row `y` (8 columns at a time, AVX2).
///
/// The production column pass calls this per row across rayon workers; `out_row` is the single
/// output row (length `width`), `y` its absolute row index for mirror-edge input addressing.
///
/// # Safety
/// Caller must ensure AVX2 is available.
#[target_feature(enable = "avx2")]
#[expect(
    clippy::cast_possible_wrap,
    reason = "pixel coordinates and kernel taps index a slice, whose length Rust caps at isize::MAX"
)]
pub(super) unsafe fn convolve_cols_row_avx2(
    input: &[f32],
    out_row: &mut [f32],
    size: Size2us,
    y: usize,
    kernel: &[f32],
    radius: usize,
) {
    unsafe {
        use crate::star_detection::convolution::simd::mirror_index;

        let mut x = 0;
        while x + 8 <= size.width {
            let mut sum = _mm256_setzero_ps();
            for (k, &kval) in kernel.iter().enumerate() {
                let sy = mirror_index(y as isize + k as isize - radius as isize, size.height);
                let vals = _mm256_loadu_ps(input.as_ptr().add(sy * size.width + x));
                sum = _mm256_add_ps(sum, _mm256_mul_ps(vals, _mm256_set1_ps(kval)));
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

/// Apply 2D convolution to a single row using AVX2 intrinsics.
///
/// Processes 8 output pixels at a time.
///
/// # Safety
/// Caller must ensure AVX2 is available.
#[target_feature(enable = "avx2")]
#[expect(
    clippy::cast_possible_wrap,
    reason = "pixel coordinates and kernel taps index a slice, whose length Rust caps at isize::MAX"
)]
pub(super) unsafe fn convolve_2d_row_avx2(
    input: &[f32],
    output_row: &mut [f32],
    size: Size2us,
    y: usize,
    kernel: Kernel2d<'_>,
) {
    unsafe {
        use crate::star_detection::convolution::simd::mirror_index;

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

                    let kv = _mm256_set1_ps(kval);
                    let base_sx = x as isize + kx as isize - radius;

                    if let Ok(start) = usize::try_from(base_sx)
                        && start + 8 <= size.width
                    {
                        let vals = _mm256_loadu_ps(input.as_ptr().add(input_row_offset + start));
                        sum = _mm256_add_ps(sum, _mm256_mul_ps(vals, kv));
                    } else {
                        let mut vals = [0.0f32; 8];
                        for i in 0..8 {
                            let sx = base_sx + i as isize;
                            let sx = mirror_index(sx, size.width);
                            vals[i] = input[input_row_offset + sx];
                        }
                        let vvals = _mm256_loadu_ps(vals.as_ptr());
                        sum = _mm256_add_ps(sum, _mm256_mul_ps(vvals, kv));
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
