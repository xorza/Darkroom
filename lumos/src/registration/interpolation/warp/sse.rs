//! SSE4.1 and AVX2 SIMD implementations for interpolation.

#![allow(clippy::needless_range_loop)]

use crate::common::Buffer2;
use glam::DVec2;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use crate::registration::transform::Transform;

/// Warp a row using AVX2 SIMD with bilinear interpolation.
///
/// Processes 8 output pixels at a time.
///
/// # Safety
/// - Caller must ensure AVX2 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn warp_row_bilinear_avx2(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    transform: &Transform,
) {
    let pixels = input.pixels();
    let input_width = input.width();
    let input_height = input.height();

    unsafe {
        let output_width = output_row.len();
        let y = output_y as f64;

        // Extract transform coefficients
        let t = transform.matrix.as_array();
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

                p00[i] = sample_pixel(pixels, input_width, input_height, ix0, iy0, 0.0);
                p10[i] = sample_pixel(pixels, input_width, input_height, ix1, iy0, 0.0);
                p01[i] = sample_pixel(pixels, input_width, input_height, ix0, iy1, 0.0);
                p11[i] = sample_pixel(pixels, input_width, input_height, ix1, iy1, 0.0);
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
            let src = transform.apply(DVec2::new(x as f64, y));
            output_row[x] = super::bilinear_sample(input, src.x as f32, src.y as f32, 0.0);
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
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    transform: &Transform,
) {
    let pixels = input.pixels();
    let input_width = input.width();
    let input_height = input.height();

    unsafe {
        let output_width = output_row.len();
        let y = output_y as f64;

        // Extract transform coefficients
        let t = transform.matrix.as_array();
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

                p00[i] = sample_pixel(pixels, input_width, input_height, ix0, iy0, 0.0);
                p10[i] = sample_pixel(pixels, input_width, input_height, ix1, iy0, 0.0);
                p01[i] = sample_pixel(pixels, input_width, input_height, ix0, iy1, 0.0);
                p11[i] = sample_pixel(pixels, input_width, input_height, ix1, iy1, 0.0);
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
            let src = transform.apply(DVec2::new(x as f64, y));
            output_row[x] = super::bilinear_sample(input, src.x as f32, src.y as f32, 0.0);
        }
    }
}

use super::super::sample_pixel;

/// Compute the Lanczos3 6x6 weighted sum for a single pixel using FMA.
///
/// Pre-loads `wx` weights into SSE registers, then for each of the 6 source rows:
/// loads pixels, multiplies by `wx`, scales by `wy[j]` (broadcast), and accumulates.
///
/// Returns `(weighted_sum, min_pixel, max_pixel)`.
///
/// # Safety
/// - Caller must ensure FMA is available.
/// - The 6x6 pixel window at `(kx, ky)` must be fully in bounds,
///   plus 2 extra columns for the 128-bit load at offset +4: `kx + 7 < input_width`.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn lanczos3_kernel_fma<const DERINGING: bool>(
    pixels: &[f32],
    input_width: usize,
    kx: usize,
    ky: usize,
    wx: &[f32; 6],
    wy: &[f32; 6],
) -> (f32, f32, f32) {
    // Pre-load wx weights into SSE registers (constant across all 6 rows)
    let wx_lo = _mm_set_ps(wx[3], wx[2], wx[1], wx[0]);
    let wx_hi = _mm_setr_ps(wx[4], wx[5], 0.0, 0.0);

    let mut acc_lo = _mm_setzero_ps();
    let mut acc_hi = _mm_setzero_ps();
    let mut vmin_lo = _mm_set1_ps(f32::MAX);
    let mut vmax_lo = _mm_set1_ps(f32::MIN);
    let mut vmin_hi = _mm_set1_ps(f32::MAX);
    let mut vmax_hi = _mm_set1_ps(f32::MIN);

    for j in 0..6 {
        let row_ptr = pixels.as_ptr().add((ky + j) * input_width + kx);
        let src_lo = _mm_loadu_ps(row_ptr);
        let src_hi = _mm_loadu_ps(row_ptr.add(4));

        let wyj = _mm_set1_ps(wy[j]);
        let weighted_lo = _mm_mul_ps(src_lo, wx_lo);
        acc_lo = _mm_fmadd_ps(weighted_lo, wyj, acc_lo);
        let weighted_hi = _mm_mul_ps(src_hi, wx_hi);
        acc_hi = _mm_fmadd_ps(weighted_hi, wyj, acc_hi);

        if DERINGING {
            vmin_lo = _mm_min_ps(vmin_lo, src_lo);
            vmax_lo = _mm_max_ps(vmax_lo, src_lo);
            // src_hi lanes 2-3 are garbage neighbors; including them in min/max
            // only widens the clamp range slightly, which is harmless for deringing
            vmin_hi = _mm_min_ps(vmin_hi, src_hi);
            vmax_hi = _mm_max_ps(vmax_hi, src_hi);
        }
    }

    // Horizontal sum (4 floats -> 1)
    let combined = _mm_add_ps(acc_lo, acc_hi);
    let shuf = _mm_movehdup_ps(combined);
    let sums = _mm_add_ps(combined, shuf);
    let high = _mm_movehl_ps(sums, sums);
    let total = _mm_add_ss(sums, high);
    let sum = _mm_cvtss_f32(total);

    if DERINGING {
        let min4 = _mm_min_ps(vmin_lo, vmin_hi);
        let shuf = _mm_movehdup_ps(min4);
        let mins = _mm_min_ps(min4, shuf);
        let high = _mm_movehl_ps(mins, mins);
        let min_final = _mm_min_ss(mins, high);
        let smin = _mm_cvtss_f32(min_final);

        let max4 = _mm_max_ps(vmax_lo, vmax_hi);
        let shuf = _mm_movehdup_ps(max4);
        let maxs = _mm_max_ps(max4, shuf);
        let high = _mm_movehl_ps(maxs, maxs);
        let max_final = _mm_max_ss(maxs, high);
        let smax = _mm_cvtss_f32(max_final);

        (sum, smin, smax)
    } else {
        (sum, 0.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::synthetic::patterns;
    #[cfg(target_arch = "x86_64")]
    use common::cpu_features;

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx2_warp_row() {
        if !cpu_features::has_avx2() {
            eprintln!("Skipping AVX2 test - not available");
            return;
        }

        let width = 128;
        let height = 64;
        let input = patterns::diagonal_gradient(width, height);
        let transform = Transform::translation(DVec2::new(2.5, 1.5));
        let inverse = transform.inverse();

        let mut output_avx2 = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];
        let y = 30;

        unsafe {
            warp_row_bilinear_avx2(&input, &mut output_avx2, y, &inverse);
        }

        let inverse_wt = super::super::WarpTransform::new(inverse);
        super::super::warp_row_bilinear_scalar(&input, &mut output_scalar, y, &inverse_wt, 0.0);

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
        let input = patterns::diagonal_gradient(width, height);
        let transform = Transform::similarity(DVec2::new(1.0, 2.0), 0.05, 1.02);
        let inverse = transform.inverse();

        let mut output_sse = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];
        let y = 25;

        unsafe {
            warp_row_bilinear_sse(&input, &mut output_sse, y, &inverse);
        }

        let inverse_wt = super::super::WarpTransform::new(inverse);
        super::super::warp_row_bilinear_scalar(&input, &mut output_scalar, y, &inverse_wt, 0.0);

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
