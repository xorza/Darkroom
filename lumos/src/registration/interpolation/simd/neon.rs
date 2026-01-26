//! ARM NEON SIMD implementations for interpolation.

#![allow(clippy::needless_range_loop)]

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

use crate::registration::types::TransformMatrix;

/// Warp a row using NEON SIMD with bilinear interpolation.
///
/// Processes 4 output pixels at a time using float32x4 vectors.
///
/// # Safety
/// - Caller must ensure this is running on aarch64 with NEON.
#[cfg(target_arch = "aarch64")]
pub unsafe fn warp_row_bilinear_neon(
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

        let a_vec = vdupq_n_f32(a);
        let d_vec = vdupq_n_f32(d);
        let g_vec = vdupq_n_f32(g);
        let by_c_vec = vdupq_n_f32(by_c);
        let ey_f_vec = vdupq_n_f32(ey_f);
        let hy_1_vec = vdupq_n_f32(hy_1);

        let chunks = output_width / 4;

        for chunk in 0..chunks {
            let base_x = chunk * 4;

            // Load x coordinates: [base_x, base_x+1, base_x+2, base_x+3]
            let x_coords = vld1q_f32(
                [
                    base_x as f32,
                    (base_x + 1) as f32,
                    (base_x + 2) as f32,
                    (base_x + 3) as f32,
                ]
                .as_ptr(),
            );

            // Compute source coordinates
            // src_x = (a*x + by_c) / (g*x + hy_1)
            // src_y = (d*x + ey_f) / (g*x + hy_1)
            let ax = vmulq_f32(a_vec, x_coords);
            let dx = vmulq_f32(d_vec, x_coords);
            let gx = vmulq_f32(g_vec, x_coords);

            let num_x = vaddq_f32(ax, by_c_vec);
            let num_y = vaddq_f32(dx, ey_f_vec);
            let denom = vaddq_f32(gx, hy_1_vec);

            // Division using reciprocal approximation with Newton-Raphson refinement
            let denom_recip = vrecpeq_f32(denom);
            let denom_recip = vmulq_f32(vrecpsq_f32(denom, denom_recip), denom_recip);
            let denom_recip = vmulq_f32(vrecpsq_f32(denom, denom_recip), denom_recip);

            let src_x = vmulq_f32(num_x, denom_recip);
            let src_y = vmulq_f32(num_y, denom_recip);

            // Compute integer coordinates (floor)
            let x0 = vrndmq_f32(src_x);
            let y0 = vrndmq_f32(src_y);

            // Compute fractional parts
            let fx = vsubq_f32(src_x, x0);
            let fy = vsubq_f32(src_y, y0);

            // Sample the four corners (scalar fallback for gather)
            let mut p00 = [0.0f32; 4];
            let mut p10 = [0.0f32; 4];
            let mut p01 = [0.0f32; 4];
            let mut p11 = [0.0f32; 4];

            let mut x0_arr = [0.0f32; 4];
            let mut y0_arr = [0.0f32; 4];

            vst1q_f32(x0_arr.as_mut_ptr(), x0);
            vst1q_f32(y0_arr.as_mut_ptr(), y0);

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

            let p00_vec = vld1q_f32(p00.as_ptr());
            let p10_vec = vld1q_f32(p10.as_ptr());
            let p01_vec = vld1q_f32(p01.as_ptr());
            let p11_vec = vld1q_f32(p11.as_ptr());

            // Bilinear interpolation
            // top = p00 + fx * (p10 - p00)
            // bottom = p01 + fx * (p11 - p01)
            // result = top + fy * (bottom - top)
            let top = vmlaq_f32(p00_vec, fx, vsubq_f32(p10_vec, p00_vec));
            let bottom = vmlaq_f32(p01_vec, fx, vsubq_f32(p11_vec, p01_vec));
            let result = vmlaq_f32(top, fy, vsubq_f32(bottom, top));

            // Store result
            vst1q_f32(output_row.as_mut_ptr().add(base_x), result);
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

    fn create_test_image(width: usize, height: usize) -> Vec<f32> {
        (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| (x as f32 + y as f32 * 0.5) / (width + height) as f32)
            })
            .collect()
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn test_neon_warp_row() {
        let width = 64;
        let height = 64;
        let input = create_test_image(width, height);
        let transform = TransformMatrix::translation(2.5, 1.5);
        let inverse = transform.inverse();

        let mut output_neon = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];
        let y = 30;

        unsafe {
            warp_row_bilinear_neon(&input, width, height, &mut output_neon, y, &inverse, -1.0);
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
                (output_neon[x] - output_scalar[x]).abs() < 1e-4,
                "x={}: NEON {} vs Scalar {}",
                x,
                output_neon[x],
                output_scalar[x]
            );
        }
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn test_neon_warp_row_rotation() {
        let width = 64;
        let height = 64;
        let input = create_test_image(width, height);
        let transform = TransformMatrix::similarity(1.0, 2.0, 0.1, 1.02);
        let inverse = transform.inverse();

        let mut output_neon = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];
        let y = 25;

        unsafe {
            warp_row_bilinear_neon(&input, width, height, &mut output_neon, y, &inverse, 0.0);
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
                (output_neon[x] - output_scalar[x]).abs() < 1e-4,
                "x={}: NEON {} vs Scalar {}",
                x,
                output_neon[x],
                output_scalar[x]
            );
        }
    }
}
