//! SSE SIMD implementation for Moffat profile fitting.
//!
//! Uses SSE to process 4 pixels in parallel. Falls back from AVX2 when not available.

#![allow(dead_code)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::arch::x86_64::*;

use super::fast_pow_neg_beta;

/// Compute u^(neg_beta) for 4 values using fast specialized power function.
#[inline]
fn compute_pow_neg_beta_4(u: &[f32; 4], neg_beta: f32) -> [f32; 4] {
    [
        fast_pow_neg_beta(u[0], neg_beta),
        fast_pow_neg_beta(u[1], neg_beta),
        fast_pow_neg_beta(u[2], neg_beta),
        fast_pow_neg_beta(u[3], neg_beta),
    ]
}

/// Compute Moffat Jacobian and residuals for 4 pixels at once (fixed beta).
#[inline]
#[target_feature(enable = "sse4.1")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn compute_jacobian_residuals_4_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    offset: usize,
    params: &[f32; 5],
    beta: f32,
    jacobian: &mut [[f32; 5]],
    residuals: &mut [f32],
) {
    debug_assert!(offset + 4 <= data_x.len());
    debug_assert!(offset + 4 <= data_y.len());
    debug_assert!(offset + 4 <= data_z.len());

    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;
    let neg_beta = -beta;

    // Precompute all reciprocals to avoid repeated divisions
    let inv_alpha2 = 1.0 / alpha2;
    let inv_alpha = 1.0 / alpha;

    // Load 4 pixel coordinates and values
    let vx = _mm_loadu_ps(data_x.as_ptr().add(offset));
    let vy = _mm_loadu_ps(data_y.as_ptr().add(offset));
    let vz = _mm_loadu_ps(data_z.as_ptr().add(offset));

    let vx0 = _mm_set1_ps(x0);
    let vy0 = _mm_set1_ps(y0);

    // Compute dx, dy using SIMD (keep in registers for later use)
    let vdx = _mm_sub_ps(vx, vx0);
    let vdy = _mm_sub_ps(vy, vy0);

    // Compute r^2 = dx^2 + dy^2 using SIMD
    let vdx2 = _mm_mul_ps(vdx, vdx);
    let vdy2 = _mm_mul_ps(vdy, vdy);
    let vr2 = _mm_add_ps(vdx2, vdy2);

    // Store only r^2 to compute u (dx/dy stay in registers)
    let mut r2_arr = [0.0f32; 4];
    _mm_storeu_ps(r2_arr.as_mut_ptr(), vr2);

    // Compute u = 1 + r^2/alpha^2 for each pixel
    let u_arr: [f32; 4] = [
        1.0 + r2_arr[0] * inv_alpha2,
        1.0 + r2_arr[1] * inv_alpha2,
        1.0 + r2_arr[2] * inv_alpha2,
        1.0 + r2_arr[3] * inv_alpha2,
    ];

    // Compute u^(-beta) using accurate scalar powf
    let u_neg_beta_arr = compute_pow_neg_beta_4(&u_arr, neg_beta);

    // Load results back into SIMD registers for remaining arithmetic
    let vu = _mm_loadu_ps(u_arr.as_ptr());
    let vu_neg_beta = _mm_loadu_ps(u_neg_beta_arr.as_ptr());
    let vamp = _mm_set1_ps(amp);
    let vbg = _mm_set1_ps(bg);

    // u^(-beta-1) = u^(-beta) / u
    let vu_neg_beta_m1 = _mm_div_ps(vu_neg_beta, vu);

    // Model value: amp * u^(-beta) + bg
    let vmodel = _mm_add_ps(_mm_mul_ps(vamp, vu_neg_beta), vbg);

    // Residual: z - model
    let vresidual = _mm_sub_ps(vz, vmodel);

    // Common factor: 2 * amp * beta / alpha^2 * u^(-beta-1)
    let common_scalar = 2.0 * amp * beta * inv_alpha2;
    let vcommon_base = _mm_set1_ps(common_scalar);
    let vcommon = _mm_mul_ps(vcommon_base, vu_neg_beta_m1);

    // Jacobian components using SIMD (use precomputed inv_alpha)
    let vj0 = _mm_mul_ps(vcommon, vdx); // df/dx0
    let vj1 = _mm_mul_ps(vcommon, vdy); // df/dy0
    let vj2 = vu_neg_beta; // df/damp
    let vinv_alpha = _mm_set1_ps(inv_alpha);
    let vj3 = _mm_mul_ps(_mm_mul_ps(vcommon, vr2), vinv_alpha); // df/dalpha

    // Store results
    let mut j0_arr = [0.0f32; 4];
    let mut j1_arr = [0.0f32; 4];
    let mut j2_arr = [0.0f32; 4];
    let mut j3_arr = [0.0f32; 4];
    let mut res_arr = [0.0f32; 4];

    _mm_storeu_ps(j0_arr.as_mut_ptr(), vj0);
    _mm_storeu_ps(j1_arr.as_mut_ptr(), vj1);
    _mm_storeu_ps(j2_arr.as_mut_ptr(), vj2);
    _mm_storeu_ps(j3_arr.as_mut_ptr(), vj3);
    _mm_storeu_ps(res_arr.as_mut_ptr(), vresidual);

    // Copy to output (AoS format)
    for i in 0..4 {
        jacobian[offset + i] = [j0_arr[i], j1_arr[i], j2_arr[i], j3_arr[i], 1.0];
        residuals[offset + i] = res_arr[i];
    }
}

/// Compute Moffat model values for 4 pixels (fixed beta).
/// Returns the sum of squared residuals for these 4 pixels.
#[inline]
#[target_feature(enable = "sse4.1")]
pub unsafe fn compute_chi2_4_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    offset: usize,
    params: &[f32; 5],
    beta: f32,
) -> f32 {
    debug_assert!(offset + 4 <= data_x.len());

    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;
    let inv_alpha2 = 1.0 / alpha2;
    let neg_beta = -beta;

    // Load coordinates using SIMD
    let vx = _mm_loadu_ps(data_x.as_ptr().add(offset));
    let vy = _mm_loadu_ps(data_y.as_ptr().add(offset));
    let vz = _mm_loadu_ps(data_z.as_ptr().add(offset));

    let vx0 = _mm_set1_ps(x0);
    let vy0 = _mm_set1_ps(y0);

    let vdx = _mm_sub_ps(vx, vx0);
    let vdy = _mm_sub_ps(vy, vy0);
    let vdx2 = _mm_mul_ps(vdx, vdx);
    let vdy2 = _mm_mul_ps(vdy, vdy);
    let vr2 = _mm_add_ps(vdx2, vdy2);

    // Store r^2 to compute u and powf
    let mut r2_arr = [0.0f32; 4];
    _mm_storeu_ps(r2_arr.as_mut_ptr(), vr2);

    // Compute u and u^(-beta) with scalar powf
    let u_arr: [f32; 4] = [
        1.0 + r2_arr[0] * inv_alpha2,
        1.0 + r2_arr[1] * inv_alpha2,
        1.0 + r2_arr[2] * inv_alpha2,
        1.0 + r2_arr[3] * inv_alpha2,
    ];
    let u_neg_beta_arr = compute_pow_neg_beta_4(&u_arr, neg_beta);

    // Continue with SIMD for model and residual computation
    let vu_neg_beta = _mm_loadu_ps(u_neg_beta_arr.as_ptr());
    let vamp = _mm_set1_ps(amp);
    let vbg = _mm_set1_ps(bg);

    let vmodel = _mm_add_ps(_mm_mul_ps(vamp, vu_neg_beta), vbg);
    let vresidual = _mm_sub_ps(vz, vmodel);
    let vresidual_sq = _mm_mul_ps(vresidual, vresidual);

    // Horizontal sum of 4 floats using store+scalar sum
    // This is faster than multiple hadd operations
    let mut res_arr = [0.0f32; 4];
    _mm_storeu_ps(res_arr.as_mut_ptr(), vresidual_sq);
    res_arr[0] + res_arr[1] + res_arr[2] + res_arr[3]
}

/// Fill Jacobian and residuals using SSE where possible.
#[target_feature(enable = "sse4.1")]
pub unsafe fn fill_jacobian_residuals_sse_fixed_beta(
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

    let simd_end = (n / 4) * 4;

    // Process 4 pixels at a time with SIMD
    for offset in (0..simd_end).step_by(4) {
        compute_jacobian_residuals_4_fixed_beta(
            data_x, data_y, data_z, offset, params, beta, jacobian, residuals,
        );
    }

    // Scalar fallback for remaining pixels
    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;
    let neg_beta = -beta;

    for i in simd_end..n {
        let x = data_x[i];
        let y = data_y[i];
        let z = data_z[i];

        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let u_neg_beta = fast_pow_neg_beta(u, neg_beta);
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

/// Compute chi^2 using SSE where possible.
#[target_feature(enable = "sse4.1")]
pub unsafe fn compute_chi2_sse_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 5],
    beta: f32,
) -> f32 {
    let n = data_x.len();
    let simd_end = (n / 4) * 4;
    let mut chi2 = 0.0f32;

    // Process 4 pixels at a time with SIMD
    for offset in (0..simd_end).step_by(4) {
        chi2 += compute_chi2_4_fixed_beta(data_x, data_y, data_z, offset, params, beta);
    }

    // Scalar fallback for remaining pixels
    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;
    let neg_beta = -beta;

    for i in simd_end..n {
        let x = data_x[i];
        let y = data_y[i];
        let z = data_z[i];

        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let model = amp * fast_pow_neg_beta(u, neg_beta) + bg;
        let residual = z - model;
        chi2 += residual * residual;
    }

    chi2
}
