//! SSE SIMD implementation for Gaussian profile fitting.
//!
//! Uses SSE to process 4 pixels in parallel. Falls back from AVX2 when not available.

#![allow(dead_code)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::arch::x86_64::*;

/// Compute exp(x) for 4 values using scalar exp (accurate).
#[inline]
fn compute_exp_4(x: &[f32; 4]) -> [f32; 4] {
    [x[0].exp(), x[1].exp(), x[2].exp(), x[3].exp()]
}

/// Compute Gaussian Jacobian and residuals for 4 pixels at once.
#[inline]
#[target_feature(enable = "sse4.1")]
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

    // Load 4 pixel coordinates and values
    let vx = _mm_loadu_ps(data_x.as_ptr().add(offset));
    let vy = _mm_loadu_ps(data_y.as_ptr().add(offset));
    let vz = _mm_loadu_ps(data_z.as_ptr().add(offset));

    let vx0 = _mm_set1_ps(x0);
    let vy0 = _mm_set1_ps(y0);

    // Compute dx, dy using SIMD
    let vdx = _mm_sub_ps(vx, vx0);
    let vdy = _mm_sub_ps(vy, vy0);

    // Compute dx^2 and dy^2
    let vdx2 = _mm_mul_ps(vdx, vdx);
    let vdy2 = _mm_mul_ps(vdy, vdy);

    // Store to compute exponent
    let mut dx_arr = [0.0f32; 4];
    let mut dy_arr = [0.0f32; 4];
    let mut dx2_arr = [0.0f32; 4];
    let mut dy2_arr = [0.0f32; 4];
    _mm_storeu_ps(dx_arr.as_mut_ptr(), vdx);
    _mm_storeu_ps(dy_arr.as_mut_ptr(), vdy);
    _mm_storeu_ps(dx2_arr.as_mut_ptr(), vdx2);
    _mm_storeu_ps(dy2_arr.as_mut_ptr(), vdy2);

    // Compute exponent = -0.5 * (dx^2/sigma_x^2 + dy^2/sigma_y^2)
    let inv_sigma_x2 = 1.0 / sigma_x2;
    let inv_sigma_y2 = 1.0 / sigma_y2;
    let exponent_arr: [f32; 4] = [
        -0.5 * (dx2_arr[0] * inv_sigma_x2 + dy2_arr[0] * inv_sigma_y2),
        -0.5 * (dx2_arr[1] * inv_sigma_x2 + dy2_arr[1] * inv_sigma_y2),
        -0.5 * (dx2_arr[2] * inv_sigma_x2 + dy2_arr[2] * inv_sigma_y2),
        -0.5 * (dx2_arr[3] * inv_sigma_x2 + dy2_arr[3] * inv_sigma_y2),
    ];

    // Compute exp using accurate scalar exp
    let exp_arr = compute_exp_4(&exponent_arr);

    // Load results back into SIMD registers for remaining arithmetic
    let vexp = _mm_loadu_ps(exp_arr.as_ptr());
    let vamp = _mm_set1_ps(amp);
    let vbg = _mm_set1_ps(bg);

    // amp * exp
    let vamp_exp = _mm_mul_ps(vamp, vexp);

    // Model value: amp * exp + bg
    let vmodel = _mm_add_ps(_mm_mul_ps(vamp, vexp), vbg);

    // Residual: z - model
    let vresidual = _mm_sub_ps(vz, vmodel);

    // Jacobian components using SIMD
    let vinv_sigma_x2 = _mm_set1_ps(inv_sigma_x2);
    let vinv_sigma_y2 = _mm_set1_ps(inv_sigma_y2);
    let vinv_sigma_x3 = _mm_set1_ps(1.0 / (sigma_x2 * sigma_x));
    let vinv_sigma_y3 = _mm_set1_ps(1.0 / (sigma_y2 * sigma_y));

    let vj0 = _mm_mul_ps(_mm_mul_ps(vamp_exp, vdx), vinv_sigma_x2); // df/dx0
    let vj1 = _mm_mul_ps(_mm_mul_ps(vamp_exp, vdy), vinv_sigma_y2); // df/dy0
    let vj2 = vexp; // df/damp
    let vj3 = _mm_mul_ps(_mm_mul_ps(vamp_exp, vdx2), vinv_sigma_x3); // df/dsigma_x
    let vj4 = _mm_mul_ps(_mm_mul_ps(vamp_exp, vdy2), vinv_sigma_y3); // df/dsigma_y

    // Store results
    let mut j0_arr = [0.0f32; 4];
    let mut j1_arr = [0.0f32; 4];
    let mut j2_arr = [0.0f32; 4];
    let mut j3_arr = [0.0f32; 4];
    let mut j4_arr = [0.0f32; 4];
    let mut res_arr = [0.0f32; 4];

    _mm_storeu_ps(j0_arr.as_mut_ptr(), vj0);
    _mm_storeu_ps(j1_arr.as_mut_ptr(), vj1);
    _mm_storeu_ps(j2_arr.as_mut_ptr(), vj2);
    _mm_storeu_ps(j3_arr.as_mut_ptr(), vj3);
    _mm_storeu_ps(j4_arr.as_mut_ptr(), vj4);
    _mm_storeu_ps(res_arr.as_mut_ptr(), vresidual);

    // Copy to output (AoS format)
    for i in 0..4 {
        jacobian[offset + i] = [j0_arr[i], j1_arr[i], j2_arr[i], j3_arr[i], j4_arr[i], 1.0];
        residuals[offset + i] = res_arr[i];
    }
}

/// Compute Gaussian model values for 4 pixels.
/// Returns the sum of squared residuals for these 4 pixels.
#[inline]
#[target_feature(enable = "sse4.1")]
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
    let vx = _mm_loadu_ps(data_x.as_ptr().add(offset));
    let vy = _mm_loadu_ps(data_y.as_ptr().add(offset));
    let vz = _mm_loadu_ps(data_z.as_ptr().add(offset));

    let vx0 = _mm_set1_ps(x0);
    let vy0 = _mm_set1_ps(y0);

    let vdx = _mm_sub_ps(vx, vx0);
    let vdy = _mm_sub_ps(vy, vy0);
    let vdx2 = _mm_mul_ps(vdx, vdx);
    let vdy2 = _mm_mul_ps(vdy, vdy);

    // Store dx2, dy2 to compute exponent
    let mut dx2_arr = [0.0f32; 4];
    let mut dy2_arr = [0.0f32; 4];
    _mm_storeu_ps(dx2_arr.as_mut_ptr(), vdx2);
    _mm_storeu_ps(dy2_arr.as_mut_ptr(), vdy2);

    // Compute exponent and exp with scalar
    let exponent_arr: [f32; 4] = [
        -0.5 * (dx2_arr[0] * inv_sigma_x2 + dy2_arr[0] * inv_sigma_y2),
        -0.5 * (dx2_arr[1] * inv_sigma_x2 + dy2_arr[1] * inv_sigma_y2),
        -0.5 * (dx2_arr[2] * inv_sigma_x2 + dy2_arr[2] * inv_sigma_y2),
        -0.5 * (dx2_arr[3] * inv_sigma_x2 + dy2_arr[3] * inv_sigma_y2),
    ];
    let exp_arr = compute_exp_4(&exponent_arr);

    // Continue with SIMD for model and residual computation
    let vexp = _mm_loadu_ps(exp_arr.as_ptr());
    let vamp = _mm_set1_ps(amp);
    let vbg = _mm_set1_ps(bg);

    let vmodel = _mm_add_ps(_mm_mul_ps(vamp, vexp), vbg);
    let vresidual = _mm_sub_ps(vz, vmodel);
    let vresidual_sq = _mm_mul_ps(vresidual, vresidual);

    // Horizontal sum of 4 floats
    let sum1 = _mm_hadd_ps(vresidual_sq, vresidual_sq);
    let sum2 = _mm_hadd_ps(sum1, sum1);
    _mm_cvtss_f32(sum2)
}

/// Fill Jacobian and residuals using SSE where possible.
#[target_feature(enable = "sse4.1")]
pub unsafe fn fill_jacobian_residuals_sse(
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

/// Compute chi^2 using SSE where possible.
#[target_feature(enable = "sse4.1")]
pub unsafe fn compute_chi2_sse(
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
