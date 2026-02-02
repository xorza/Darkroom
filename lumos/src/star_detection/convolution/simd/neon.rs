//! ARM NEON implementation of row convolution.
//!
//! Processes 4 pixels at a time using 128-bit vectors.

use std::arch::aarch64::*;

use super::convolve_pixel_scalar;

/// Convolve a row using NEON intrinsics.
///
/// Processes 4 pixels at a time using 128-bit vectors.
///
/// # Safety
/// Caller must ensure running on aarch64 (NEON is always available on aarch64).
pub unsafe fn convolve_row_neon(input: &[f32], output: &mut [f32], kernel: &[f32], radius: usize) {
    let width = input.len();

    // Process 4 pixels at a time in the middle section
    let safe_start = radius;
    let safe_end = width.saturating_sub(radius + 4);

    // Handle left edge with scalar
    for (x, out) in output.iter_mut().enumerate().take(safe_start.min(width)) {
        *out = convolve_pixel_scalar(input, kernel, radius, x, width);
    }

    // SIMD middle section
    let mut x = safe_start;
    if safe_start < safe_end {
        unsafe {
            while x + 4 <= safe_end + radius {
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

/// Convolve columns directly using NEON intrinsics.
///
/// Processes rows in order for cache locality, with 4 columns at a time using SIMD.
///
/// # Safety
/// Caller must ensure running on aarch64.
pub unsafe fn convolve_cols_neon(
    input: &[f32],
    output: &mut [f32],
    width: usize,
    height: usize,
    kernel: &[f32],
    radius: usize,
) {
    use super::mirror_index;

    // Process row by row for cache locality
    for y in 0..height {
        let out_row_offset = y * width;

        // Process 4 columns at a time with SIMD
        let mut x = 0;
        while x + 4 <= width {
            let mut sum = vdupq_n_f32(0.0);

            for (k, &kval) in kernel.iter().enumerate() {
                let sy = y as isize + k as isize - radius as isize;
                let sy = mirror_index(sy, height);

                let vals = vld1q_f32(input.as_ptr().add(sy * width + x));
                sum = vfmaq_f32(sum, vals, vdupq_n_f32(kval));
            }

            vst1q_f32(output.as_mut_ptr().add(out_row_offset + x), sum);
            x += 4;
        }

        // Handle remaining columns with scalar
        while x < width {
            let mut sum = 0.0f32;
            for (k, &kval) in kernel.iter().enumerate() {
                let sy = y as isize + k as isize - radius as isize;
                let sy = mirror_index(sy, height);
                sum += input[sy * width + x] * kval;
            }
            output[out_row_offset + x] = sum;
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
#[allow(clippy::too_many_arguments)]
pub unsafe fn convolve_2d_row_neon(
    input: &[f32],
    output_row: &mut [f32],
    width: usize,
    height: usize,
    y: usize,
    kernel: &[f32],
    ksize: usize,
    radius: usize,
) {
    use super::mirror_index;

    // Process 4 output pixels at a time
    let mut x = 0;
    while x + 4 <= width {
        let mut sum = vdupq_n_f32(0.0);

        for ky in 0..ksize {
            let sy = y as isize + ky as isize - radius as isize;
            let sy = mirror_index(sy, height);
            let input_row_offset = sy * width;

            for kx in 0..ksize {
                let kval = kernel[ky * ksize + kx];
                if kval.abs() < 1e-10 {
                    continue;
                }

                let kv = vdupq_n_f32(kval);
                let base_sx = x as isize + kx as isize - radius as isize;

                if base_sx >= 0 && base_sx + 4 <= width as isize {
                    let vals = vld1q_f32(input.as_ptr().add(input_row_offset + base_sx as usize));
                    sum = vfmaq_f32(sum, vals, kv);
                } else {
                    let mut vals = [0.0f32; 4];
                    for i in 0..4 {
                        let sx = base_sx + i as isize;
                        let sx = mirror_index(sx, width);
                        vals[i] = input[input_row_offset + sx];
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
    while x < width {
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
        output_row[x] = sum;
        x += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_neon_matches_scalar() {
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
