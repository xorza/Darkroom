//! ARM NEON implementation of row convolution.
//!
//! Processes 4 pixels at a time using 128-bit vectors.

use std::arch::aarch64::*;

use crate::math::size2us::Size2us;
use crate::stacking::star_detection::convolution::simd::{Kernel2d, convolve_pixel_scalar};

/// Convolve a row using NEON intrinsics.
///
/// Processes 4 pixels at a time using 128-bit vectors.
///
/// # Safety
/// Caller must ensure running on aarch64 (NEON is always available on aarch64).
pub(super) unsafe fn convolve_row_neon(
    input: &[f32],
    output: &mut [f32],
    kernel: &[f32],
    radius: usize,
) {
    unsafe {
        let width = input.len();

        // Process 4 pixels at a time in the middle section. A column is SIMD-safe only if its whole
        // kernel window stays in bounds (the interior does no mirroring). The widest source read for
        // the 4-wide block at x is `(x + 3) + (kernel.len() - 1) - radius`; requiring it `<= width-1`
        // gives the bound below — via `kernel.len()` (not the symmetric `2*radius+1`) so the SIMD
        // interior matches the scalar mirror reference for any kernel.
        let safe_start = radius;
        let safe_end = (width + radius + 1).saturating_sub(4 + kernel.len());

        // Handle left edge with scalar
        for (x, out) in output.iter_mut().enumerate().take(safe_start.min(width)) {
            *out = convolve_pixel_scalar(input, kernel, radius, x, width);
        }

        // SIMD middle section. `safe_end` is the last fully-in-bounds block start, so the bound is
        // `x <= safe_end` and not an `x + 4 <= safe_end + radius` variant: for radius > 4 — the
        // common case — that over-reads one element and skips the mirroring at the boundary column.
        let mut x = safe_start;
        if safe_start < safe_end {
            while x <= safe_end {
                let mut sum = vdupq_n_f32(0.0);

                for (k, &kval) in kernel.iter().enumerate() {
                    let kv = vdupq_n_f32(kval);
                    let sx = x + k - radius;

                    // Load 4 input values
                    let vals = vld1q_f32(input.as_ptr().add(sx));

                    // Multiply-accumulate (FMA)
                    sum = vfmaq_f32(sum, vals, kv);
                }

                // Store 4 output values
                vst1q_f32(output.as_mut_ptr().add(x), sum);
                x += 4;
            }
        }

        // Handle remaining middle pixels with scalar (including when SIMD section was skipped)
        while x < width.saturating_sub(radius) {
            output[x] = convolve_pixel_scalar(input, kernel, radius, x, width);
            x += 1;
        }

        // Handle right edge with scalar (mirroring)
        for (x, out) in output
            .iter_mut()
            .enumerate()
            .take(width)
            .skip(width.saturating_sub(radius))
        {
            *out = convolve_pixel_scalar(input, kernel, radius, x, width);
        }
    }
}

/// Convolve one output column-row `y` (4 columns at a time, NEON). See
/// `convolve_cols_row_avx2` for the per-row contract.
///
/// # Safety
/// Caller must ensure running on aarch64.
pub(super) unsafe fn convolve_cols_row_neon(
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
            let mut sum = vdupq_n_f32(0.0);
            for (k, &kval) in kernel.iter().enumerate() {
                let sy = mirror_index(y as isize + k as isize - radius as isize, size.height);
                let vals = vld1q_f32(input.as_ptr().add(sy * size.width + x));
                sum = vfmaq_f32(sum, vals, vdupq_n_f32(kval));
            }
            vst1q_f32(out_row.as_mut_ptr().add(x), sum);
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

/// Apply 2D convolution to a single row using NEON intrinsics.
///
/// Processes 4 output pixels at a time.
///
/// # Safety
/// Caller must ensure running on aarch64.
pub(super) unsafe fn convolve_2d_row_neon(
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
            let mut sum = vdupq_n_f32(0.0);

            for ky in 0..kernel.size() {
                let sy = mirror_index(y as isize + ky as isize - radius, size.height);
                let input_row_offset = sy * size.width;

                for kx in 0..kernel.size() {
                    let kval = kernel.at(ky, kx);
                    if kval.abs() < 1e-10 {
                        continue;
                    }

                    let kv = vdupq_n_f32(kval);
                    let base_sx = x as isize + kx as isize - radius;

                    if base_sx >= 0 && base_sx + 4 <= size.width as isize {
                        let vals =
                            vld1q_f32(input.as_ptr().add(input_row_offset + base_sx as usize));
                        sum = vfmaq_f32(sum, vals, kv);
                    } else {
                        let mut vals = [0.0f32; 4];
                        for (i, val) in vals.iter_mut().enumerate() {
                            let sx = base_sx + i as isize;
                            let sx = mirror_index(sx, size.width);
                            *val = input[input_row_offset + sx];
                        }
                        let vvals = vld1q_f32(vals.as_ptr());
                        sum = vfmaq_f32(sum, vvals, kv);
                    }
                }
            }

            vst1q_f32(output_row.as_mut_ptr().add(x), sum);
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
mod tests {
    use crate::stacking::star_detection::convolution::simd::neon::*;

    #[test]
    fn neon_matches_scalar() {
        let input: Vec<f32> = (0..256).map(|i| (i as f32).sin()).collect();
        let kernel = vec![0.05, 0.1, 0.2, 0.3, 0.2, 0.1, 0.05];
        let radius = 3;

        let mut output_neon = vec![0.0f32; 256];
        let mut output_scalar = vec![0.0f32; 256];

        unsafe {
            convolve_row_neon(&input, &mut output_neon, &kernel, radius);
        }

        for (x, out) in output_scalar.iter_mut().enumerate() {
            *out = convolve_pixel_scalar(&input, &kernel, radius, x, 256);
        }

        for i in 0..256 {
            assert!(
                (output_neon[i] - output_scalar[i]).abs() < 1e-5,
                "NEON mismatch at {}: {} vs {}",
                i,
                output_neon[i],
                output_scalar[i]
            );
        }
    }
}
