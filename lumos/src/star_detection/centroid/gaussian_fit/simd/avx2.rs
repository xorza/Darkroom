//! AVX2 SIMD implementation for Gaussian profile fitting.
//!
//! Uses fully vectorized exp() via fast polynomial approximation (Cephes-style),
//! keeping all computation in SIMD registers without scalar round-trips.

#![allow(unsafe_op_in_unsafe_fn)]

use std::arch::x86_64::*;

/// Compute Gaussian Jacobian and residuals for 8 pixels at once.
///
/// Uses vectorized exp (Cephes polynomial in AVX2) for all 8 pixels simultaneously.
#[inline]
#[target_feature(enable = "avx2", enable = "fma")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn compute_jacobian_residuals_8(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    offset: usize,
    params: &[f32; 6],
    jacobian: &mut [[f32; 6]],
    residuals: &mut [f32],
) {
    debug_assert!(offset + 8 <= data_x.len());
    debug_assert!(offset + 8 <= data_y.len());
    debug_assert!(offset + 8 <= data_z.len());

    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;

    // Precompute all reciprocals to avoid repeated divisions
    let inv_sigma_x2 = 1.0 / sigma_x2;
    let inv_sigma_y2 = 1.0 / sigma_y2;
    let inv_sigma_x3 = inv_sigma_x2 / sigma_x;
    let inv_sigma_y3 = inv_sigma_y2 / sigma_y;

    // Load 8 pixel coordinates and values
    let vx = _mm256_loadu_ps(data_x.as_ptr().add(offset));
    let vy = _mm256_loadu_ps(data_y.as_ptr().add(offset));
    let vz = _mm256_loadu_ps(data_z.as_ptr().add(offset));

    let vx0 = _mm256_set1_ps(x0);
    let vy0 = _mm256_set1_ps(y0);

    // Compute dx, dy using SIMD
    let vdx = _mm256_sub_ps(vx, vx0);
    let vdy = _mm256_sub_ps(vy, vy0);

    // Compute dx^2 and dy^2
    let vdx2 = _mm256_mul_ps(vdx, vdx);
    let vdy2 = _mm256_mul_ps(vdy, vdy);

    // Compute exponent = -0.5 * (dx^2/sigma_x^2 + dy^2/sigma_y^2) entirely in SIMD
    let vinv_sx2 = _mm256_set1_ps(inv_sigma_x2);
    let vinv_sy2 = _mm256_set1_ps(inv_sigma_y2);
    let vneg_half = _mm256_set1_ps(-0.5);
    // exponent = -0.5 * (dx2 * inv_sx2 + dy2 * inv_sy2)
    let vexponent = _mm256_mul_ps(
        vneg_half,
        _mm256_fmadd_ps(vdx2, vinv_sx2, _mm256_mul_ps(vdy2, vinv_sy2)),
    );

    // Vectorized exp — all 8 values computed in SIMD registers
    let vexp = crate::math::fast_exp::avx2::fast_exp_8_avx2_m256(vexponent);

    let vamp = _mm256_set1_ps(amp);
    let vbg = _mm256_set1_ps(bg);

    // amp * exp
    let vamp_exp = _mm256_mul_ps(vamp, vexp);

    // Model value: amp * exp + bg
    let vmodel = _mm256_fmadd_ps(vamp, vexp, vbg);

    // Residual: z - model
    let vresidual = _mm256_sub_ps(vz, vmodel);

    // Jacobian components using SIMD (use precomputed reciprocals)
    let vinv_sigma_x2 = _mm256_set1_ps(inv_sigma_x2);
    let vinv_sigma_y2 = _mm256_set1_ps(inv_sigma_y2);
    let vinv_sigma_x3 = _mm256_set1_ps(inv_sigma_x3);
    let vinv_sigma_y3 = _mm256_set1_ps(inv_sigma_y3);

    let vj0 = _mm256_mul_ps(_mm256_mul_ps(vamp_exp, vdx), vinv_sigma_x2); // df/dx0
    let vj1 = _mm256_mul_ps(_mm256_mul_ps(vamp_exp, vdy), vinv_sigma_y2); // df/dy0
    let vj2 = vexp; // df/damp
    let vj3 = _mm256_mul_ps(_mm256_mul_ps(vamp_exp, vdx2), vinv_sigma_x3); // df/dsigma_x
    let vj4 = _mm256_mul_ps(_mm256_mul_ps(vamp_exp, vdy2), vinv_sigma_y3); // df/dsigma_y

    // Store results
    let mut j0_arr = [0.0f32; 8];
    let mut j1_arr = [0.0f32; 8];
    let mut j2_arr = [0.0f32; 8];
    let mut j3_arr = [0.0f32; 8];
    let mut j4_arr = [0.0f32; 8];
    let mut res_arr = [0.0f32; 8];

    _mm256_storeu_ps(j0_arr.as_mut_ptr(), vj0);
    _mm256_storeu_ps(j1_arr.as_mut_ptr(), vj1);
    _mm256_storeu_ps(j2_arr.as_mut_ptr(), vj2);
    _mm256_storeu_ps(j3_arr.as_mut_ptr(), vj3);
    _mm256_storeu_ps(j4_arr.as_mut_ptr(), vj4);
    _mm256_storeu_ps(res_arr.as_mut_ptr(), vresidual);

    // Copy to output (AoS format)
    for i in 0..8 {
        jacobian[offset + i] = [j0_arr[i], j1_arr[i], j2_arr[i], j3_arr[i], j4_arr[i], 1.0];
        residuals[offset + i] = res_arr[i];
    }
}

/// Compute Gaussian model values for 8 pixels.
/// Returns the sum of squared residuals for these 8 pixels.
#[inline]
#[target_feature(enable = "avx2", enable = "fma")]
pub unsafe fn compute_chi2_8(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    offset: usize,
    params: &[f32; 6],
) -> f32 {
    debug_assert!(offset + 8 <= data_x.len());

    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;
    let inv_sigma_x2 = 1.0 / sigma_x2;
    let inv_sigma_y2 = 1.0 / sigma_y2;

    // Load coordinates using SIMD
    let vx = _mm256_loadu_ps(data_x.as_ptr().add(offset));
    let vy = _mm256_loadu_ps(data_y.as_ptr().add(offset));
    let vz = _mm256_loadu_ps(data_z.as_ptr().add(offset));

    let vx0 = _mm256_set1_ps(x0);
    let vy0 = _mm256_set1_ps(y0);

    let vdx = _mm256_sub_ps(vx, vx0);
    let vdy = _mm256_sub_ps(vy, vy0);
    let vdx2 = _mm256_mul_ps(vdx, vdx);
    let vdy2 = _mm256_mul_ps(vdy, vdy);

    // Compute exponent entirely in SIMD
    let vinv_sx2 = _mm256_set1_ps(inv_sigma_x2);
    let vinv_sy2 = _mm256_set1_ps(inv_sigma_y2);
    let vneg_half = _mm256_set1_ps(-0.5);
    let vexponent = _mm256_mul_ps(
        vneg_half,
        _mm256_fmadd_ps(vdx2, vinv_sx2, _mm256_mul_ps(vdy2, vinv_sy2)),
    );

    // Vectorized exp
    let vexp = crate::math::fast_exp::avx2::fast_exp_8_avx2_m256(vexponent);

    let vamp = _mm256_set1_ps(amp);
    let vbg = _mm256_set1_ps(bg);

    let vmodel = _mm256_fmadd_ps(vamp, vexp, vbg);
    let vresidual = _mm256_sub_ps(vz, vmodel);
    let vresidual_sq = _mm256_mul_ps(vresidual, vresidual);

    // Horizontal sum of 8 floats using store+scalar sum
    // This is faster than multiple hadd operations
    let mut res_arr = [0.0f32; 8];
    _mm256_storeu_ps(res_arr.as_mut_ptr(), vresidual_sq);
    res_arr[0]
        + res_arr[1]
        + res_arr[2]
        + res_arr[3]
        + res_arr[4]
        + res_arr[5]
        + res_arr[6]
        + res_arr[7]
}

/// Fill Jacobian and residuals using AVX2 where possible.
/// Falls back to scalar for remaining pixels.
#[target_feature(enable = "avx2", enable = "fma")]
pub unsafe fn fill_jacobian_residuals_avx2(
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

    let simd_end = (n / 8) * 8;

    // Process 8 pixels at a time with SIMD
    for offset in (0..simd_end).step_by(8) {
        compute_jacobian_residuals_8(data_x, data_y, data_z, offset, params, jacobian, residuals);
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

/// Compute chi^2 using AVX2 where possible.
#[target_feature(enable = "avx2", enable = "fma")]
pub unsafe fn compute_chi2_avx2(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 6],
) -> f32 {
    let n = data_x.len();
    let simd_end = (n / 8) * 8;
    let mut chi2 = 0.0f32;

    // Process 8 pixels at a time with SIMD
    for offset in (0..simd_end).step_by(8) {
        chi2 += compute_chi2_8(data_x, data_y, data_z, offset, params);
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
