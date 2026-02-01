//! NEON SIMD implementation for Moffat profile fitting (aarch64).
//!
//! Uses NEON to process 4 pixels in parallel.

#![allow(dead_code)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::arch::aarch64::*;

/// Compute u^(-beta) for 4 values using scalar powf (accurate).
#[inline]
fn compute_pow_neg_beta_4(u: &[f32; 4], neg_beta: f32) -> [f32; 4] {
    [
        u[0].powf(neg_beta),
        u[1].powf(neg_beta),
        u[2].powf(neg_beta),
        u[3].powf(neg_beta),
    ]
}

/// Compute Moffat Jacobian and residuals for 4 pixels at once (fixed beta).
#[inline]
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

    // Load 4 pixel coordinates and values
    let vx = vld1q_f32(data_x.as_ptr().add(offset));
    let vy = vld1q_f32(data_y.as_ptr().add(offset));
    let vz = vld1q_f32(data_z.as_ptr().add(offset));

    let vx0 = vdupq_n_f32(x0);
    let vy0 = vdupq_n_f32(y0);

    // Compute dx, dy using SIMD
    let vdx = vsubq_f32(vx, vx0);
    let vdy = vsubq_f32(vy, vy0);

    // Compute r^2 = dx^2 + dy^2 using SIMD with FMA
    let vr2 = vfmaq_f32(vmulq_f32(vdy, vdy), vdx, vdx);

    // Store dx, dy, r^2 to compute u and call scalar powf
    let mut dx_arr = [0.0f32; 4];
    let mut dy_arr = [0.0f32; 4];
    let mut r2_arr = [0.0f32; 4];
    vst1q_f32(dx_arr.as_mut_ptr(), vdx);
    vst1q_f32(dy_arr.as_mut_ptr(), vdy);
    vst1q_f32(r2_arr.as_mut_ptr(), vr2);

    // Compute u = 1 + r^2/alpha^2 for each pixel
    let inv_alpha2 = 1.0 / alpha2;
    let u_arr: [f32; 4] = [
        1.0 + r2_arr[0] * inv_alpha2,
        1.0 + r2_arr[1] * inv_alpha2,
        1.0 + r2_arr[2] * inv_alpha2,
        1.0 + r2_arr[3] * inv_alpha2,
    ];

    // Compute u^(-beta) using accurate scalar powf
    let u_neg_beta_arr = compute_pow_neg_beta_4(&u_arr, neg_beta);

    // Load results back into SIMD registers for remaining arithmetic
    let vu = vld1q_f32(u_arr.as_ptr());
    let vu_neg_beta = vld1q_f32(u_neg_beta_arr.as_ptr());
    let vamp = vdupq_n_f32(amp);
    let vbg = vdupq_n_f32(bg);

    // u^(-beta-1) = u^(-beta) / u
    let vu_neg_beta_m1 = vdivq_f32(vu_neg_beta, vu);

    // Model value: amp * u^(-beta) + bg
    let vmodel = vfmaq_f32(vbg, vamp, vu_neg_beta);

    // Residual: z - model
    let vresidual = vsubq_f32(vz, vmodel);

    // Common factor: 2 * amp * beta / alpha^2 * u^(-beta-1)
    let common_scalar = 2.0 * amp * beta * inv_alpha2;
    let vcommon_base = vdupq_n_f32(common_scalar);
    let vcommon = vmulq_f32(vcommon_base, vu_neg_beta_m1);

    // Jacobian components using SIMD
    let vj0 = vmulq_f32(vcommon, vdx); // df/dx0
    let vj1 = vmulq_f32(vcommon, vdy); // df/dy0
    let vj2 = vu_neg_beta; // df/damp
    let inv_alpha = 1.0 / alpha;
    let vinv_alpha = vdupq_n_f32(inv_alpha);
    let vj3 = vmulq_f32(vmulq_f32(vcommon, vr2), vinv_alpha); // df/dalpha

    // Store results
    let mut j0_arr = [0.0f32; 4];
    let mut j1_arr = [0.0f32; 4];
    let mut j2_arr = [0.0f32; 4];
    let mut j3_arr = [0.0f32; 4];
    let mut res_arr = [0.0f32; 4];

    vst1q_f32(j0_arr.as_mut_ptr(), vj0);
    vst1q_f32(j1_arr.as_mut_ptr(), vj1);
    vst1q_f32(j2_arr.as_mut_ptr(), vj2);
    vst1q_f32(j3_arr.as_mut_ptr(), vj3);
    vst1q_f32(res_arr.as_mut_ptr(), vresidual);

    // Copy to output (AoS format)
    for i in 0..4 {
        jacobian[offset + i] = [j0_arr[i], j1_arr[i], j2_arr[i], j3_arr[i], 1.0];
        residuals[offset + i] = res_arr[i];
    }
}

/// Compute Moffat model values for 4 pixels (fixed beta).
/// Returns the sum of squared residuals for these 4 pixels.
#[inline]
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
    let vx = vld1q_f32(data_x.as_ptr().add(offset));
    let vy = vld1q_f32(data_y.as_ptr().add(offset));
    let vz = vld1q_f32(data_z.as_ptr().add(offset));

    let vx0 = vdupq_n_f32(x0);
    let vy0 = vdupq_n_f32(y0);

    let vdx = vsubq_f32(vx, vx0);
    let vdy = vsubq_f32(vy, vy0);
    let vr2 = vfmaq_f32(vmulq_f32(vdy, vdy), vdx, vdx);

    // Store r^2 to compute u and powf
    let mut r2_arr = [0.0f32; 4];
    vst1q_f32(r2_arr.as_mut_ptr(), vr2);

    // Compute u and u^(-beta) with scalar powf
    let u_arr: [f32; 4] = [
        1.0 + r2_arr[0] * inv_alpha2,
        1.0 + r2_arr[1] * inv_alpha2,
        1.0 + r2_arr[2] * inv_alpha2,
        1.0 + r2_arr[3] * inv_alpha2,
    ];
    let u_neg_beta_arr = compute_pow_neg_beta_4(&u_arr, neg_beta);

    // Continue with SIMD for model and residual computation
    let vu_neg_beta = vld1q_f32(u_neg_beta_arr.as_ptr());
    let vamp = vdupq_n_f32(amp);
    let vbg = vdupq_n_f32(bg);

    let vmodel = vfmaq_f32(vbg, vamp, vu_neg_beta);
    let vresidual = vsubq_f32(vz, vmodel);
    let vresidual_sq = vmulq_f32(vresidual, vresidual);

    // Horizontal sum of 4 floats
    vaddvq_f32(vresidual_sq)
}

/// Fill Jacobian and residuals using NEON where possible.
pub unsafe fn fill_jacobian_residuals_neon_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 5],
    beta: f32,
    jacobian: &mut Vec<[f32; 5]>,
    residuals: &mut Vec<f32>,
) {
    let n = data_x.len();
    jacobian.clear();
    jacobian.resize(n, [0.0; 5]);
    residuals.clear();
    residuals.resize(n, 0.0);

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

/// Compute chi^2 using NEON where possible.
pub unsafe fn compute_chi2_neon_fixed_beta(
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
