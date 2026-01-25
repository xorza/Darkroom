//! SSE4.1 and AVX2 SIMD implementations for interpolation.

#![allow(clippy::needless_range_loop)]

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use crate::registration::types::TransformMatrix;

/// Warp a row using AVX2 SIMD with bilinear interpolation.
///
/// Processes 8 output pixels at a time.
///
/// # Safety
/// - Caller must ensure AVX2 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn warp_row_bilinear_avx2(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_row: &mut [f32],
    output_y: usize,
    inverse: &TransformMatrix,
    border_value: f32,
) {
    unsafe {
        let output_width = output_row.len();
        let y = output_y as f64;

        // Extract transform coefficients
        let t = &inverse.data;
        let a = t[0] as f32;
        let b = t[1] as f32;
        let c = t[2] as f32;
        let d = t[3] as f32;
        let e = t[4] as f32;
        let f = t[5] as f32;
        let g = t[6] as f32;
        let h = t[7] as f32;

        // Pre-compute y-dependent terms
        let by_c = b * y as f32 + c;
        let ey_f = e * y as f32 + f;
        let hy_1 = h * y as f32 + 1.0;

        let a_vec = _mm256_set1_ps(a);
        let d_vec = _mm256_set1_ps(d);
        let g_vec = _mm256_set1_ps(g);
        let by_c_vec = _mm256_set1_ps(by_c);
        let ey_f_vec = _mm256_set1_ps(ey_f);
        let hy_1_vec = _mm256_set1_ps(hy_1);

        let one = _mm256_set1_ps(1.0);

        let chunks = output_width / 8;

        for chunk in 0..chunks {
            let base_x = chunk * 8;

            // Load x coordinates: [base_x, base_x+1, ..., base_x+7]
            let x_coords = _mm256_set_ps(
                (base_x + 7) as f32,
                (base_x + 6) as f32,
                (base_x + 5) as f32,
                (base_x + 4) as f32,
                (base_x + 3) as f32,
                (base_x + 2) as f32,
                (base_x + 1) as f32,
                base_x as f32,
            );

            // Compute source coordinates
            // src_x = (a*x + by_c) / (g*x + hy_1)
            // src_y = (d*x + ey_f) / (g*x + hy_1)
            let ax = _mm256_mul_ps(a_vec, x_coords);
            let dx = _mm256_mul_ps(d_vec, x_coords);
            let gx = _mm256_mul_ps(g_vec, x_coords);

            let num_x = _mm256_add_ps(ax, by_c_vec);
            let num_y = _mm256_add_ps(dx, ey_f_vec);
            let denom = _mm256_add_ps(gx, hy_1_vec);

            let src_x = _mm256_div_ps(num_x, denom);
            let src_y = _mm256_div_ps(num_y, denom);

            // Compute integer coordinates
            let x0 = _mm256_floor_ps(src_x);
            let y0 = _mm256_floor_ps(src_y);
            let x1 = _mm256_add_ps(x0, one);
            let y1 = _mm256_add_ps(y0, one);

            // Compute fractional parts
            let fx = _mm256_sub_ps(src_x, x0);
            let fy = _mm256_sub_ps(src_y, y0);

            // Sample the four corners (scalar fallback for gather since it's complex)
            let mut p00 = [0.0f32; 8];
            let mut p10 = [0.0f32; 8];
            let mut p01 = [0.0f32; 8];
            let mut p11 = [0.0f32; 8];

            let mut x0_arr = [0.0f32; 8];
            let mut y0_arr = [0.0f32; 8];
            let mut x1_arr = [0.0f32; 8];
            let mut y1_arr = [0.0f32; 8];

            _mm256_storeu_ps(x0_arr.as_mut_ptr(), x0);
            _mm256_storeu_ps(y0_arr.as_mut_ptr(), y0);
            _mm256_storeu_ps(x1_arr.as_mut_ptr(), x1);
            _mm256_storeu_ps(y1_arr.as_mut_ptr(), y1);

            for i in 0..8 {
                let ix0 = x0_arr[i] as i32;
                let iy0 = y0_arr[i] as i32;
                let ix1 = x1_arr[i] as i32;
                let iy1 = y1_arr[i] as i32;

                p00[i] =
                    sample_pixel_scalar(input, input_width, input_height, ix0, iy0, border_value);
                p10[i] =
                    sample_pixel_scalar(input, input_width, input_height, ix1, iy0, border_value);
                p01[i] =
                    sample_pixel_scalar(input, input_width, input_height, ix0, iy1, border_value);
                p11[i] =
                    sample_pixel_scalar(input, input_width, input_height, ix1, iy1, border_value);
            }

            let p00_vec = _mm256_loadu_ps(p00.as_ptr());
            let p10_vec = _mm256_loadu_ps(p10.as_ptr());
            let p01_vec = _mm256_loadu_ps(p01.as_ptr());
            let p11_vec = _mm256_loadu_ps(p11.as_ptr());

            // Bilinear interpolation
            // top = p00 + fx * (p10 - p00)
            // bottom = p01 + fx * (p11 - p01)
            // result = top + fy * (bottom - top)
            let top = _mm256_add_ps(p00_vec, _mm256_mul_ps(fx, _mm256_sub_ps(p10_vec, p00_vec)));
            let bottom = _mm256_add_ps(p01_vec, _mm256_mul_ps(fx, _mm256_sub_ps(p11_vec, p01_vec)));
            let result = _mm256_add_ps(top, _mm256_mul_ps(fy, _mm256_sub_ps(bottom, top)));

            // Store result
            _mm256_storeu_ps(output_row.as_mut_ptr().add(base_x), result);
        }

        // Handle remainder with scalar
        let remainder_start = chunks * 8;
        for x in remainder_start..output_width {
            let (src_x, src_y) = inverse.transform_point(x as f64, y);
            output_row[x] = super::bilinear_sample(
                input,
                input_width,
                input_height,
                src_x as f32,
                src_y as f32,
                border_value,
            );
        }
    }
}

/// Warp a row using SSE4.1 SIMD with bilinear interpolation.
///
/// Processes 4 output pixels at a time.
///
/// # Safety
/// - Caller must ensure SSE4.1 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
pub unsafe fn warp_row_bilinear_sse(
    input: &[f32],
    input_width: usize,
    input_height: usize,
    output_row: &mut [f32],
    output_y: usize,
    inverse: &TransformMatrix,
    border_value: f32,
) {
    unsafe {
        let output_width = output_row.len();
        let y = output_y as f64;

        // Extract transform coefficients
        let t = &inverse.data;
        let a = t[0] as f32;
        let b = t[1] as f32;
        let c = t[2] as f32;
        let d = t[3] as f32;
        let e = t[4] as f32;
        let f = t[5] as f32;
        let g = t[6] as f32;
        let h = t[7] as f32;

        // Pre-compute y-dependent terms
        let by_c = b * y as f32 + c;
        let ey_f = e * y as f32 + f;
        let hy_1 = h * y as f32 + 1.0;

        let a_vec = _mm_set1_ps(a);
        let d_vec = _mm_set1_ps(d);
        let g_vec = _mm_set1_ps(g);
        let by_c_vec = _mm_set1_ps(by_c);
        let ey_f_vec = _mm_set1_ps(ey_f);
        let hy_1_vec = _mm_set1_ps(hy_1);

        let chunks = output_width / 4;

        for chunk in 0..chunks {
            let base_x = chunk * 4;

            // Load x coordinates
            let x_coords = _mm_set_ps(
                (base_x + 3) as f32,
                (base_x + 2) as f32,
                (base_x + 1) as f32,
                base_x as f32,
            );

            // Compute source coordinates
            let ax = _mm_mul_ps(a_vec, x_coords);
            let dx = _mm_mul_ps(d_vec, x_coords);
            let gx = _mm_mul_ps(g_vec, x_coords);

            let num_x = _mm_add_ps(ax, by_c_vec);
            let num_y = _mm_add_ps(dx, ey_f_vec);
            let denom = _mm_add_ps(gx, hy_1_vec);

            let src_x = _mm_div_ps(num_x, denom);
            let src_y = _mm_div_ps(num_y, denom);

            // Compute integer coordinates
            let x0 = _mm_floor_ps(src_x);
            let y0 = _mm_floor_ps(src_y);

            // Compute fractional parts
            let fx = _mm_sub_ps(src_x, x0);
            let fy = _mm_sub_ps(src_y, y0);

            // Sample the four corners (scalar)
            let mut p00 = [0.0f32; 4];
            let mut p10 = [0.0f32; 4];
            let mut p01 = [0.0f32; 4];
            let mut p11 = [0.0f32; 4];

            let mut x0_arr = [0.0f32; 4];
            let mut y0_arr = [0.0f32; 4];

            _mm_storeu_ps(x0_arr.as_mut_ptr(), x0);
            _mm_storeu_ps(y0_arr.as_mut_ptr(), y0);

            for i in 0..4 {
                let ix0 = x0_arr[i] as i32;
                let iy0 = y0_arr[i] as i32;
                let ix1 = ix0 + 1;
                let iy1 = iy0 + 1;

                p00[i] =
                    sample_pixel_scalar(input, input_width, input_height, ix0, iy0, border_value);
                p10[i] =
                    sample_pixel_scalar(input, input_width, input_height, ix1, iy0, border_value);
                p01[i] =
                    sample_pixel_scalar(input, input_width, input_height, ix0, iy1, border_value);
                p11[i] =
                    sample_pixel_scalar(input, input_width, input_height, ix1, iy1, border_value);
            }

            let p00_vec = _mm_loadu_ps(p00.as_ptr());
            let p10_vec = _mm_loadu_ps(p10.as_ptr());
            let p01_vec = _mm_loadu_ps(p01.as_ptr());
            let p11_vec = _mm_loadu_ps(p11.as_ptr());

            // Bilinear interpolation
            let top = _mm_add_ps(p00_vec, _mm_mul_ps(fx, _mm_sub_ps(p10_vec, p00_vec)));
            let bottom = _mm_add_ps(p01_vec, _mm_mul_ps(fx, _mm_sub_ps(p11_vec, p01_vec)));
            let result = _mm_add_ps(top, _mm_mul_ps(fy, _mm_sub_ps(bottom, top)));

            // Store result
            _mm_storeu_ps(output_row.as_mut_ptr().add(base_x), result);
        }

        // Handle remainder with scalar
        let remainder_start = chunks * 4;
        for x in remainder_start..output_width {
            let (src_x, src_y) = inverse.transform_point(x as f64, y);
            output_row[x] = super::bilinear_sample(
                input,
                input_width,
                input_height,
                src_x as f32,
                src_y as f32,
                border_value,
            );
        }
    }
}

/// Scalar pixel sampling with bounds checking.
#[inline]
fn sample_pixel_scalar(
    data: &[f32],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
    border: f32,
) -> f32 {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        border
    } else {
        data[y as usize * width + x as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_arch = "x86_64")]
    use crate::common::cpu_features;

    fn create_test_image(width: usize, height: usize) -> Vec<f32> {
        (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| (x as f32 + y as f32 * 0.5) / (width + height) as f32)
            })
            .collect()
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx2_warp_row() {
        if !cpu_features::has_avx2() {
            eprintln!("Skipping AVX2 test - not available");
            return;
        }

        let width = 128;
        let height = 64;
        let input = create_test_image(width, height);
        let transform = TransformMatrix::translation(2.5, 1.5);
        let inverse = transform.inverse();

        let mut output_avx2 = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];
        let y = 30;

        unsafe {
            warp_row_bilinear_avx2(&input, width, height, &mut output_avx2, y, &inverse, -1.0);
        }

        super::super::warp_row_bilinear_scalar(
            &input,
            width,
            height,
            &mut output_scalar,
            y,
            &inverse,
            -1.0,
        );

        for x in 0..width {
            assert!(
                (output_avx2[x] - output_scalar[x]).abs() < 1e-5,
                "x={}: AVX2 {} vs Scalar {}",
                x,
                output_avx2[x],
                output_scalar[x]
            );
        }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_sse_warp_row() {
        if !cpu_features::has_sse4_1() {
            eprintln!("Skipping SSE test - not available");
            return;
        }

        let width = 64;
        let height = 64;
        let input = create_test_image(width, height);
        let transform = TransformMatrix::similarity(1.0, 2.0, 0.05, 1.02);
        let inverse = transform.inverse();

        let mut output_sse = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];
        let y = 25;

        unsafe {
            warp_row_bilinear_sse(&input, width, height, &mut output_sse, y, &inverse, 0.0);
        }

        super::super::warp_row_bilinear_scalar(
            &input,
            width,
            height,
            &mut output_scalar,
            y,
            &inverse,
            0.0,
        );

        for x in 0..width {
            assert!(
                (output_sse[x] - output_scalar[x]).abs() < 1e-5,
                "x={}: SSE {} vs Scalar {}",
                x,
                output_sse[x],
                output_scalar[x]
            );
        }
    }
}
