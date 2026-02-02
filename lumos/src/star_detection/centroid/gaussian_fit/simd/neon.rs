//! NEON SIMD implementation for Gaussian profile fitting (aarch64).
//!
//! Uses NEON to process 4 pixels in parallel.

#![allow(dead_code)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::arch::aarch64::*;

/// Compute exp(x) for 4 values using scalar exp (accurate).
#[inline]
fn compute_exp_4(x: &[f32; 4]) -> [f32; 4] {
    [x[0].exp(), x[1].exp(), x[2].exp(), x[3].exp()]
}

/// Compute Gaussian Jacobian and residuals for 4 pixels at once.
#[inline]
#[allow(clippy::too_many_arguments)]
pub unsafe fn compute_jacobian_residuals_4(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    offset: usize,
    params: &[f32; 6],
    jacobian: &mut [[f32; 6]],
    residuals: &mut [f32],
) {
    debug_assert!(offset + 4 <= data_x.len());
    debug_assert!(offset + 4 <= data_y.len());
    debug_assert!(offset + 4 <= data_z.len());

    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;

    // Precompute all reciprocals to avoid repeated divisions
    let inv_sigma_x2 = 1.0 / sigma_x2;
    let inv_sigma_y2 = 1.0 / sigma_y2;
    let inv_sigma_x3 = inv_sigma_x2 / sigma_x; // Reuse reciprocal
    let inv_sigma_y3 = inv_sigma_y2 / sigma_y; // Reuse reciprocal

    // Load 4 pixel coordinates and values
    let vx = vld1q_f32(data_x.as_ptr().add(offset));
    let vy = vld1q_f32(data_y.as_ptr().add(offset));
    let vz = vld1q_f32(data_z.as_ptr().add(offset));

    let vx0 = vdupq_n_f32(x0);
    let vy0 = vdupq_n_f32(y0);

    // Compute dx, dy using SIMD (keep in registers for later use)
    let vdx = vsubq_f32(vx, vx0);
    let vdy = vsubq_f32(vy, vy0);

    // Compute dx^2 and dy^2
    let vdx2 = vmulq_f32(vdx, vdx);
    let vdy2 = vmulq_f32(vdy, vdy);

    // Store only dx2/dy2 to compute exponent (dx/dy stay in registers)
    let mut dx2_arr = [0.0f32; 4];
    let mut dy2_arr = [0.0f32; 4];
    vst1q_f32(dx2_arr.as_mut_ptr(), vdx2);
    vst1q_f32(dy2_arr.as_mut_ptr(), vdy2);

    // Compute exponent = -0.5 * (dx^2/sigma_x^2 + dy^2/sigma_y^2)
    let exponent_arr: [f32; 4] = [
        -0.5 * (dx2_arr[0] * inv_sigma_x2 + dy2_arr[0] * inv_sigma_y2),
        -0.5 * (dx2_arr[1] * inv_sigma_x2 + dy2_arr[1] * inv_sigma_y2),
        -0.5 * (dx2_arr[2] * inv_sigma_x2 + dy2_arr[2] * inv_sigma_y2),
        -0.5 * (dx2_arr[3] * inv_sigma_x2 + dy2_arr[3] * inv_sigma_y2),
    ];

    // Compute exp using accurate scalar exp
    let exp_arr = compute_exp_4(&exponent_arr);

    // Load results back into SIMD registers for remaining arithmetic
    let vexp = vld1q_f32(exp_arr.as_ptr());
    let vamp = vdupq_n_f32(amp);
    let vbg = vdupq_n_f32(bg);

    // amp * exp
    let vamp_exp = vmulq_f32(vamp, vexp);

    // Model value: amp * exp + bg
    let vmodel = vfmaq_f32(vbg, vamp, vexp);

    // Residual: z - model
    let vresidual = vsubq_f32(vz, vmodel);

    // Jacobian components using SIMD (use precomputed reciprocals)
    let vinv_sigma_x2 = vdupq_n_f32(inv_sigma_x2);
    let vinv_sigma_y2 = vdupq_n_f32(inv_sigma_y2);
    let vinv_sigma_x3 = vdupq_n_f32(inv_sigma_x3);
    let vinv_sigma_y3 = vdupq_n_f32(inv_sigma_y3);

    let vj0 = vmulq_f32(vmulq_f32(vamp_exp, vdx), vinv_sigma_x2); // df/dx0
    let vj1 = vmulq_f32(vmulq_f32(vamp_exp, vdy), vinv_sigma_y2); // df/dy0
    let vj2 = vexp; // df/damp
    let vj3 = vmulq_f32(vmulq_f32(vamp_exp, vdx2), vinv_sigma_x3); // df/dsigma_x
    let vj4 = vmulq_f32(vmulq_f32(vamp_exp, vdy2), vinv_sigma_y3); // df/dsigma_y

    // Store results
    let mut j0_arr = [0.0f32; 4];
    let mut j1_arr = [0.0f32; 4];
    let mut j2_arr = [0.0f32; 4];
    let mut j3_arr = [0.0f32; 4];
    let mut j4_arr = [0.0f32; 4];
    let mut res_arr = [0.0f32; 4];

    vst1q_f32(j0_arr.as_mut_ptr(), vj0);
    vst1q_f32(j1_arr.as_mut_ptr(), vj1);
    vst1q_f32(j2_arr.as_mut_ptr(), vj2);
    vst1q_f32(j3_arr.as_mut_ptr(), vj3);
    vst1q_f32(j4_arr.as_mut_ptr(), vj4);
    vst1q_f32(res_arr.as_mut_ptr(), vresidual);

    // Copy to output (AoS format)
    for i in 0..4 {
        jacobian[offset + i] = [j0_arr[i], j1_arr[i], j2_arr[i], j3_arr[i], j4_arr[i], 1.0];
        residuals[offset + i] = res_arr[i];
    }
}

/// Compute Gaussian model values for 4 pixels.
/// Returns the sum of squared residuals for these 4 pixels.
#[inline]
pub unsafe fn compute_chi2_4(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    offset: usize,
    params: &[f32; 6],
) -> f32 {
    debug_assert!(offset + 4 <= data_x.len());

    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;
    let inv_sigma_x2 = 1.0 / sigma_x2;
    let inv_sigma_y2 = 1.0 / sigma_y2;

    // Load coordinates using SIMD
    let vx = vld1q_f32(data_x.as_ptr().add(offset));
    let vy = vld1q_f32(data_y.as_ptr().add(offset));
    let vz = vld1q_f32(data_z.as_ptr().add(offset));

    let vx0 = vdupq_n_f32(x0);
    let vy0 = vdupq_n_f32(y0);

    let vdx = vsubq_f32(vx, vx0);
    let vdy = vsubq_f32(vy, vy0);
    let vdx2 = vmulq_f32(vdx, vdx);
    let vdy2 = vmulq_f32(vdy, vdy);

    // Store dx2, dy2 to compute exponent
    let mut dx2_arr = [0.0f32; 4];
    let mut dy2_arr = [0.0f32; 4];
    vst1q_f32(dx2_arr.as_mut_ptr(), vdx2);
    vst1q_f32(dy2_arr.as_mut_ptr(), vdy2);

    // Compute exponent and exp with scalar
    let exponent_arr: [f32; 4] = [
        -0.5 * (dx2_arr[0] * inv_sigma_x2 + dy2_arr[0] * inv_sigma_y2),
        -0.5 * (dx2_arr[1] * inv_sigma_x2 + dy2_arr[1] * inv_sigma_y2),
        -0.5 * (dx2_arr[2] * inv_sigma_x2 + dy2_arr[2] * inv_sigma_y2),
        -0.5 * (dx2_arr[3] * inv_sigma_x2 + dy2_arr[3] * inv_sigma_y2),
    ];
    let exp_arr = compute_exp_4(&exponent_arr);

    // Continue with SIMD for model and residual computation
    let vexp = vld1q_f32(exp_arr.as_ptr());
    let vamp = vdupq_n_f32(amp);
    let vbg = vdupq_n_f32(bg);

    let vmodel = vfmaq_f32(vbg, vamp, vexp);
    let vresidual = vsubq_f32(vz, vmodel);
    let vresidual_sq = vmulq_f32(vresidual, vresidual);

    // Horizontal sum of 4 floats
    vaddvq_f32(vresidual_sq)
}

/// Fill Jacobian and residuals using NEON where possible.
pub unsafe fn fill_jacobian_residuals_neon(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 6],
    jacobian: &mut Vec<[f32; 6]>,
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
        jacobian.resize(n, [0.0; 6]);
        residuals.resize(n, 0.0);
    }

    let simd_end = (n / 4) * 4;

    // Process 4 pixels at a time with SIMD
    for offset in (0..simd_end).step_by(4) {
        compute_jacobian_residuals_4(data_x, data_y, data_z, offset, params, jacobian, residuals);
    }

    // Scalar fallback for remaining pixels
    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;

    for i in simd_end..n {
        let x = data_x[i];
        let y = data_y[i];
        let z = data_z[i];

        let dx = x - x0;
        let dy = y - y0;
        let dx2 = dx * dx;
        let dy2 = dy * dy;
        let exponent = -0.5 * (dx2 / sigma_x2 + dy2 / sigma_y2);
        let exp_val = exponent.exp();
        let amp_exp = amp * exp_val;
        let model = amp_exp + bg;

        residuals[i] = z - model;

        jacobian[i] = [
            amp_exp * dx / sigma_x2,
            amp_exp * dy / sigma_y2,
            exp_val,
            amp_exp * dx2 / (sigma_x2 * sigma_x),
            amp_exp * dy2 / (sigma_y2 * sigma_y),
            1.0,
        ];
    }
}

/// Compute chi^2 using NEON where possible.
pub unsafe fn compute_chi2_neon(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 6],
) -> f32 {
    let n = data_x.len();
    let simd_end = (n / 4) * 4;
    let mut chi2 = 0.0f32;

    // Process 4 pixels at a time with SIMD
    for offset in (0..simd_end).step_by(4) {
        chi2 += compute_chi2_4(data_x, data_y, data_z, offset, params);
    }

    // Scalar fallback for remaining pixels
    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;

    for i in simd_end..n {
        let x = data_x[i];
        let y = data_y[i];
        let z = data_z[i];

        let dx = x - x0;
        let dy = y - y0;
        let exponent = -0.5 * (dx * dx / sigma_x2 + dy * dy / sigma_y2);
        let model = amp * exponent.exp() + bg;
        let residual = z - model;
        chi2 += residual * residual;
    }

    chi2
}
