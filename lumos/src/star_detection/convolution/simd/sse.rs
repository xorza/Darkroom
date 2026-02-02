//! SSE4.1 and AVX2 implementations of row convolution.
//!
//! These implementations use SIMD intrinsics to process multiple pixels
//! in parallel, achieving 4-8× speedup over scalar code.

// Allow indexed loops - necessary for SIMD code patterns where we need
// explicit index control for pointer arithmetic
#![allow(clippy::needless_range_loop)]

use std::arch::x86_64::*;

use super::convolve_pixel_scalar;

/// Convolve a row using AVX2 + FMA intrinsics.
///
/// Processes 8 pixels at a time using 256-bit vectors.
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available (use `is_x86_feature_detected!`).
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

/// Convolve columns directly using AVX2 intrinsics.
///
/// Processes rows in order for cache locality, with 8 columns at a time using SIMD.
/// Uses row-major traversal: for each row, process all column groups.
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available.
#[target_feature(enable = "avx2,fma")]
pub unsafe fn convolve_cols_avx2(
    input: &[f32],
    output: &mut [f32],
    width: usize,
    height: usize,
    kernel: &[f32],
    radius: usize,
) {
    unsafe {
        use super::mirror_index;

        // Process row by row for cache locality
        for y in 0..height {
            let out_row_offset = y * width;

            // Process 8 columns at a time with SIMD
            let mut x = 0;
            while x + 8 <= width {
                let mut sum = _mm256_setzero_ps();

                for (k, &kval) in kernel.iter().enumerate() {
                    let sy = y as isize + k as isize - radius as isize;
                    let sy = mirror_index(sy, height);

                    let vals = _mm256_loadu_ps(input.as_ptr().add(sy * width + x));
                    sum = _mm256_fmadd_ps(vals, _mm256_set1_ps(kval), sum);
                }

                _mm256_storeu_ps(output.as_mut_ptr().add(out_row_offset + x), sum);
                x += 8;
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
}

/// Convolve columns directly using SSE4.1 intrinsics.
///
/// Processes rows in order for cache locality, with 4 columns at a time using SIMD.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub unsafe fn convolve_cols_sse41(
    input: &[f32],
    output: &mut [f32],
    width: usize,
    height: usize,
    kernel: &[f32],
    radius: usize,
) {
    unsafe {
        use super::mirror_index;

        // Process row by row for cache locality
        for y in 0..height {
            let out_row_offset = y * width;

            // Process 4 columns at a time with SIMD
            let mut x = 0;
            while x + 4 <= width {
                let mut sum = _mm_setzero_ps();

                for (k, &kval) in kernel.iter().enumerate() {
                    let sy = y as isize + k as isize - radius as isize;
                    let sy = mirror_index(sy, height);

                    let vals = _mm_loadu_ps(input.as_ptr().add(sy * width + x));
                    sum = _mm_add_ps(sum, _mm_mul_ps(vals, _mm_set1_ps(kval)));
                }

                _mm_storeu_ps(output.as_mut_ptr().add(out_row_offset + x), sum);
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
}

/// Apply 2D convolution to a single row using AVX2 intrinsics.
///
/// Processes 8 output pixels at a time.
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available.
#[target_feature(enable = "avx2,fma")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn convolve_2d_row_avx2(
    input: &[f32],
    output_row: &mut [f32],
    width: usize,
    height: usize,
    y: usize,
    kernel: &[f32],
    ksize: usize,
    radius: usize,
) {
    unsafe {
        use super::mirror_index;

        // Process 8 output pixels at a time
        let mut x = 0;
        while x + 8 <= width {
            let mut sum = _mm256_setzero_ps();

            for ky in 0..ksize {
                let sy = y as isize + ky as isize - radius as isize;
                let sy = mirror_index(sy, height);
                let input_row_offset = sy * width;

                for kx in 0..ksize {
                    let kval = kernel[ky * ksize + kx];
                    if kval.abs() < 1e-10 {
                        continue;
                    }

                    let kv = _mm256_set1_ps(kval);
                    let base_sx = x as isize + kx as isize - radius as isize;

                    if base_sx >= 0 && base_sx + 8 <= width as isize {
                        let vals = _mm256_loadu_ps(
                            input.as_ptr().add(input_row_offset + base_sx as usize),
                        );
                        sum = _mm256_fmadd_ps(vals, kv, sum);
                    } else {
                        let mut vals = [0.0f32; 8];
                        for i in 0..8 {
                            let sx = base_sx + i as isize;
                            let sx = mirror_index(sx, width);
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
}

/// Apply 2D convolution to a single row using SSE4.1 intrinsics.
///
/// Processes 4 output pixels at a time.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn convolve_2d_row_sse41(
    input: &[f32],
    output_row: &mut [f32],
    width: usize,
    height: usize,
    y: usize,
    kernel: &[f32],
    ksize: usize,
    radius: usize,
) {
    unsafe {
        use super::mirror_index;

        // Process 4 output pixels at a time
        let mut x = 0;
        while x + 4 <= width {
            let mut sum = _mm_setzero_ps();

            for ky in 0..ksize {
                let sy = y as isize + ky as isize - radius as isize;
                let sy = mirror_index(sy, height);
                let input_row_offset = sy * width;

                for kx in 0..ksize {
                    let kval = kernel[ky * ksize + kx];
                    if kval.abs() < 1e-10 {
                        continue;
                    }

                    let kv = _mm_set1_ps(kval);
                    let base_sx = x as isize + kx as isize - radius as isize;

                    if base_sx >= 0 && base_sx + 4 <= width as isize {
                        let vals =
                            _mm_loadu_ps(input.as_ptr().add(input_row_offset + base_sx as usize));
                        sum = _mm_add_ps(sum, _mm_mul_ps(vals, kv));
                    } else {
                        let mut vals = [0.0f32; 4];
                        for i in 0..4 {
                            let sx = base_sx + i as isize;
                            let sx = mirror_index(sx, width);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::cpu_features;

    #[test]
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

    #[test]
    fn test_avx2_cols_matches_scalar() {
        if !cpu_features::has_avx2_fma() {
            eprintln!("Skipping AVX2 cols test: CPU does not support AVX2+FMA");
            return;
        }

        let width = 64;
        let height = 64;
        let input: Vec<f32> = (0..width * height)
            .map(|i| (i as f32 * 0.1).sin())
            .collect();
        let kernel = vec![0.05, 0.1, 0.2, 0.3, 0.2, 0.1, 0.05];
        let radius = 3;

        let mut output_avx2 = vec![0.0f32; width * height];
        let mut output_scalar = vec![0.0f32; width * height];

        unsafe {
            convolve_cols_avx2(&input, &mut output_avx2, width, height, &kernel, radius);
        }

        // Scalar reference
        for x in 0..width {
            for y in 0..height {
                let mut sum = 0.0f32;
                for (k, &kval) in kernel.iter().enumerate() {
                    let sy = y as isize + k as isize - radius as isize;
                    let sy = crate::star_detection::convolution::simd::mirror_index(sy, height);
                    sum += input[sy * width + x] * kval;
                }
                output_scalar[y * width + x] = sum;
            }
        }

        for i in 0..width * height {
            assert!(
                (output_avx2[i] - output_scalar[i]).abs() < 1e-5,
                "AVX2 cols mismatch at {}: {} vs {}",
                i,
                output_avx2[i],
                output_scalar[i]
            );
        }
    }

    #[test]
    fn test_sse41_cols_matches_scalar() {
        if !cpu_features::has_sse4_1() {
            eprintln!("Skipping SSE4.1 cols test: CPU does not support SSE4.1");
            return;
        }

        let width = 64;
        let height = 64;
        let input: Vec<f32> = (0..width * height)
            .map(|i| (i as f32 * 0.1).sin())
            .collect();
        let kernel = vec![0.05, 0.1, 0.2, 0.3, 0.2, 0.1, 0.05];
        let radius = 3;

        let mut output_sse = vec![0.0f32; width * height];
        let mut output_scalar = vec![0.0f32; width * height];

        unsafe {
            convolve_cols_sse41(&input, &mut output_sse, width, height, &kernel, radius);
        }

        // Scalar reference
        for x in 0..width {
            for y in 0..height {
                let mut sum = 0.0f32;
                for (k, &kval) in kernel.iter().enumerate() {
                    let sy = y as isize + k as isize - radius as isize;
                    let sy = crate::star_detection::convolution::simd::mirror_index(sy, height);
                    sum += input[sy * width + x] * kval;
                }
                output_scalar[y * width + x] = sum;
            }
        }

        for i in 0..width * height {
            assert!(
                (output_sse[i] - output_scalar[i]).abs() < 1e-5,
                "SSE4.1 cols mismatch at {}: {} vs {}",
                i,
                output_sse[i],
                output_scalar[i]
            );
        }
    }
}
