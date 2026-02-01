//! AVX2 SIMD implementation for Moffat profile fitting.
//!
//! Uses a hybrid approach: compute the expensive powf() calls with scalar code,
//! then use SIMD for the remaining arithmetic (which is most of the work).

#![allow(dead_code)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::arch::x86_64::*;

/// Compute u^(-beta) for 8 values using scalar powf (accurate).
/// The powf call is the expensive part, but we need accuracy for L-M.
#[inline]
fn compute_pow_neg_beta_8(u: &[f32; 8], neg_beta: f32) -> [f32; 8] {
    [
        u[0].powf(neg_beta),
        u[1].powf(neg_beta),
        u[2].powf(neg_beta),
        u[3].powf(neg_beta),
        u[4].powf(neg_beta),
        u[5].powf(neg_beta),
        u[6].powf(neg_beta),
        u[7].powf(neg_beta),
    ]
}

/// Compute Moffat Jacobian and residuals for 8 pixels at once (fixed beta).
///
/// Uses scalar powf for accuracy, SIMD for all other arithmetic.
#[inline]
#[target_feature(enable = "avx2", enable = "fma")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn compute_jacobian_residuals_8_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    offset: usize,
    params: &[f32; 5],
    beta: f32,
    jacobian: &mut [[f32; 5]],
    residuals: &mut [f32],
) {
    debug_assert!(offset + 8 <= data_x.len());
    debug_assert!(offset + 8 <= data_y.len());
    debug_assert!(offset + 8 <= data_z.len());

    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;
    let neg_beta = -beta;

    // Load 8 pixel coordinates and values
    let vx = _mm256_loadu_ps(data_x.as_ptr().add(offset));
    let vy = _mm256_loadu_ps(data_y.as_ptr().add(offset));
    let vz = _mm256_loadu_ps(data_z.as_ptr().add(offset));

    let vx0 = _mm256_set1_ps(x0);
    let vy0 = _mm256_set1_ps(y0);

    // Compute dx, dy using SIMD
    let vdx = _mm256_sub_ps(vx, vx0);
    let vdy = _mm256_sub_ps(vy, vy0);

    // Compute r^2 = dx^2 + dy^2 using SIMD
    let vr2 = _mm256_fmadd_ps(vdx, vdx, _mm256_mul_ps(vdy, vdy));

    // Store dx, dy, r^2 to compute u and call scalar powf
    let mut dx_arr = [0.0f32; 8];
    let mut dy_arr = [0.0f32; 8];
    let mut r2_arr = [0.0f32; 8];
    _mm256_storeu_ps(dx_arr.as_mut_ptr(), vdx);
    _mm256_storeu_ps(dy_arr.as_mut_ptr(), vdy);
    _mm256_storeu_ps(r2_arr.as_mut_ptr(), vr2);

    // Compute u = 1 + r^2/alpha^2 for each pixel
    let inv_alpha2 = 1.0 / alpha2;
    let u_arr: [f32; 8] = [
        1.0 + r2_arr[0] * inv_alpha2,
        1.0 + r2_arr[1] * inv_alpha2,
        1.0 + r2_arr[2] * inv_alpha2,
        1.0 + r2_arr[3] * inv_alpha2,
        1.0 + r2_arr[4] * inv_alpha2,
        1.0 + r2_arr[5] * inv_alpha2,
        1.0 + r2_arr[6] * inv_alpha2,
        1.0 + r2_arr[7] * inv_alpha2,
    ];

    // Compute u^(-beta) using accurate scalar powf
    let u_neg_beta_arr = compute_pow_neg_beta_8(&u_arr, neg_beta);

    // Load results back into SIMD registers for remaining arithmetic
    let vu = _mm256_loadu_ps(u_arr.as_ptr());
    let vu_neg_beta = _mm256_loadu_ps(u_neg_beta_arr.as_ptr());
    let vamp = _mm256_set1_ps(amp);
    let vbg = _mm256_set1_ps(bg);

    // u^(-beta-1) = u^(-beta) / u
    let vu_neg_beta_m1 = _mm256_div_ps(vu_neg_beta, vu);

    // Model value: amp * u^(-beta) + bg
    let vmodel = _mm256_fmadd_ps(vamp, vu_neg_beta, vbg);

    // Residual: z - model
    let vresidual = _mm256_sub_ps(vz, vmodel);

    // Common factor: 2 * amp * beta / alpha^2 * u^(-beta-1)
    let common_scalar = 2.0 * amp * beta * inv_alpha2;
    let vcommon_base = _mm256_set1_ps(common_scalar);
    let vcommon = _mm256_mul_ps(vcommon_base, vu_neg_beta_m1);

    // Jacobian components using SIMD
    let vj0 = _mm256_mul_ps(vcommon, vdx); // df/dx0
    let vj1 = _mm256_mul_ps(vcommon, vdy); // df/dy0
    let vj2 = vu_neg_beta; // df/damp
    let inv_alpha = 1.0 / alpha;
    let vinv_alpha = _mm256_set1_ps(inv_alpha);
    let vj3 = _mm256_mul_ps(_mm256_mul_ps(vcommon, vr2), vinv_alpha); // df/dalpha

    // Store results
    let mut j0_arr = [0.0f32; 8];
    let mut j1_arr = [0.0f32; 8];
    let mut j2_arr = [0.0f32; 8];
    let mut j3_arr = [0.0f32; 8];
    let mut res_arr = [0.0f32; 8];

    _mm256_storeu_ps(j0_arr.as_mut_ptr(), vj0);
    _mm256_storeu_ps(j1_arr.as_mut_ptr(), vj1);
    _mm256_storeu_ps(j2_arr.as_mut_ptr(), vj2);
    _mm256_storeu_ps(j3_arr.as_mut_ptr(), vj3);
    _mm256_storeu_ps(res_arr.as_mut_ptr(), vresidual);

    // Copy to output (AoS format)
    for i in 0..8 {
        jacobian[offset + i] = [j0_arr[i], j1_arr[i], j2_arr[i], j3_arr[i], 1.0];
        residuals[offset + i] = res_arr[i];
    }
}

/// Compute Moffat model values for 8 pixels (fixed beta).
/// Returns the sum of squared residuals for these 8 pixels.
#[inline]
#[target_feature(enable = "avx2", enable = "fma")]
pub unsafe fn compute_chi2_8_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    offset: usize,
    params: &[f32; 5],
    beta: f32,
) -> f32 {
    debug_assert!(offset + 8 <= data_x.len());

    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;
    let inv_alpha2 = 1.0 / alpha2;
    let neg_beta = -beta;

    // Load coordinates using SIMD
    let vx = _mm256_loadu_ps(data_x.as_ptr().add(offset));
    let vy = _mm256_loadu_ps(data_y.as_ptr().add(offset));
    let vz = _mm256_loadu_ps(data_z.as_ptr().add(offset));

    let vx0 = _mm256_set1_ps(x0);
    let vy0 = _mm256_set1_ps(y0);

    let vdx = _mm256_sub_ps(vx, vx0);
    let vdy = _mm256_sub_ps(vy, vy0);
    let vr2 = _mm256_fmadd_ps(vdx, vdx, _mm256_mul_ps(vdy, vdy));

    // Store r^2 to compute u and powf
    let mut r2_arr = [0.0f32; 8];
    _mm256_storeu_ps(r2_arr.as_mut_ptr(), vr2);

    // Compute u and u^(-beta) with scalar powf
    let u_arr: [f32; 8] = [
        1.0 + r2_arr[0] * inv_alpha2,
        1.0 + r2_arr[1] * inv_alpha2,
        1.0 + r2_arr[2] * inv_alpha2,
        1.0 + r2_arr[3] * inv_alpha2,
        1.0 + r2_arr[4] * inv_alpha2,
        1.0 + r2_arr[5] * inv_alpha2,
        1.0 + r2_arr[6] * inv_alpha2,
        1.0 + r2_arr[7] * inv_alpha2,
    ];
    let u_neg_beta_arr = compute_pow_neg_beta_8(&u_arr, neg_beta);

    // Continue with SIMD for model and residual computation
    let vu_neg_beta = _mm256_loadu_ps(u_neg_beta_arr.as_ptr());
    let vamp = _mm256_set1_ps(amp);
    let vbg = _mm256_set1_ps(bg);

    let vmodel = _mm256_fmadd_ps(vamp, vu_neg_beta, vbg);
    let vresidual = _mm256_sub_ps(vz, vmodel);
    let vresidual_sq = _mm256_mul_ps(vresidual, vresidual);

    // Horizontal sum of 8 floats
    let sum1 = _mm256_hadd_ps(vresidual_sq, vresidual_sq);
    let sum2 = _mm256_hadd_ps(sum1, sum1);
    let low = _mm256_extractf128_ps(sum2, 0);
    let high = _mm256_extractf128_ps(sum2, 1);
    let sum = _mm_add_ss(low, high);
    _mm_cvtss_f32(sum)
}

/// Fill Jacobian and residuals using SIMD where possible.
/// Falls back to scalar for remaining pixels.
#[target_feature(enable = "avx2", enable = "fma")]
pub unsafe fn fill_jacobian_residuals_simd_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 5],
    beta: f32,
    jacobian: &mut Vec<[f32; 5]>,
    residuals: &mut Vec<f32>,
) {
    let n = data_x.len();

    // Resize buffers, reusing capacity when possible
    jacobian.clear();
    residuals.clear();
    if jacobian.capacity() >= n {
        // SAFETY: We're about to fill all n elements
        jacobian.set_len(n);
        residuals.set_len(n);
    } else {
        jacobian.resize(n, [0.0; 5]);
        residuals.resize(n, 0.0);
    }

    let simd_end = (n / 8) * 8;

    // Process 8 pixels at a time with SIMD
    for offset in (0..simd_end).step_by(8) {
        compute_jacobian_residuals_8_fixed_beta(
            data_x, data_y, data_z, offset, params, beta, jacobian, residuals,
        );
    }

    // Scalar fallback for remaining pixels
    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;

    for i in simd_end..n {
        let x = data_x[i];
        let y = data_y[i];
        let z = data_z[i];

        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let u_neg_beta = u.powf(-beta);
        let u_neg_beta_m1 = u_neg_beta / u;
        let model = amp * u_neg_beta + bg;

        residuals[i] = z - model;

        let common = 2.0 * amp * beta / alpha2 * u_neg_beta_m1;
        jacobian[i] = [
            common * dx,
            common * dy,
            u_neg_beta,
            common * r2 / alpha,
            1.0,
        ];
    }
}

/// Compute chi^2 using SIMD where possible.
#[target_feature(enable = "avx2", enable = "fma")]
pub unsafe fn compute_chi2_simd_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 5],
    beta: f32,
) -> f32 {
    let n = data_x.len();
    let simd_end = (n / 8) * 8;
    let mut chi2 = 0.0f32;

    // Process 8 pixels at a time with SIMD
    for offset in (0..simd_end).step_by(8) {
        chi2 += compute_chi2_8_fixed_beta(data_x, data_y, data_z, offset, params, beta);
    }

    // Scalar fallback for remaining pixels
    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;

    for i in simd_end..n {
        let x = data_x[i];
        let y = data_y[i];
        let z = data_z[i];

        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let model = amp * u.powf(-beta) + bg;
        let residual = z - model;
        chi2 += residual * residual;
    }

    chi2
}
