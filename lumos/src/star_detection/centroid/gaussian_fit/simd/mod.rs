//! SIMD-accelerated Gaussian profile computations.
//!
//! Provides runtime dispatch to the best available SIMD implementation:
//! - AVX2+FMA on x86_64 (8 pixels in parallel)
//! - SSE4.1 on x86_64 (4 pixels in parallel)
//! - NEON on aarch64 (4 pixels in parallel)
//! - Scalar fallback on other platforms

use common::{cfg_aarch64, cfg_x86_64};

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

cfg_x86_64! {
    pub mod avx2;
    pub mod sse;
}

cfg_aarch64! {
    pub mod neon;
}

#[cfg(test)]
mod tests;

// ============================================================================
// Scalar fallback implementation
// ============================================================================

/// Resize vectors to n elements, reusing capacity when possible.
/// This avoids allocation if capacity >= n.
#[inline]
fn resize_buffers(jacobian: &mut Vec<[f32; 6]>, residuals: &mut Vec<f32>, n: usize) {
    jacobian.clear();
    residuals.clear();

    if jacobian.capacity() >= n {
        // SAFETY: We're about to fill all n elements
        unsafe {
            jacobian.set_len(n);
            residuals.set_len(n);
        }
    } else {
        jacobian.resize(n, [0.0; 6]);
        residuals.resize(n, 0.0);
    }
}

/// Scalar implementation of Jacobian/residual computation.
#[inline]
pub fn fill_jacobian_residuals_scalar(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 6],
    jacobian: &mut Vec<[f32; 6]>,
    residuals: &mut Vec<f32>,
) {
    let n = data_x.len();
    resize_buffers(jacobian, residuals, n);

    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;

    for i in 0..n {
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
            amp_exp * dx / sigma_x2,              // df/dx0
            amp_exp * dy / sigma_y2,              // df/dy0
            exp_val,                              // df/damp
            amp_exp * dx2 / (sigma_x2 * sigma_x), // df/dsigma_x
            amp_exp * dy2 / (sigma_y2 * sigma_y), // df/dsigma_y
            1.0,                                  // df/dbg
        ];
    }
}

/// Scalar implementation of chi² computation.
#[inline]
pub fn compute_chi2_scalar(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 6],
) -> f32 {
    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;
    let mut chi2 = 0.0f32;

    for i in 0..data_x.len() {
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

// ============================================================================
// Dispatch functions
// ============================================================================

/// Fill Jacobian and residuals using the best available SIMD implementation.
///
/// Automatically dispatches to AVX2, SSE4.1, NEON, or scalar based on platform.
#[inline]
pub fn fill_jacobian_residuals_simd(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 6],
    jacobian: &mut Vec<[f32; 6]>,
    residuals: &mut Vec<f32>,
) {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64
        unsafe {
            neon::fill_jacobian_residuals_neon(data_x, data_y, data_z, params, jacobian, residuals);
        }
        return;
    }

    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            // SAFETY: We've checked that AVX2+FMA is available
            unsafe {
                avx2::fill_jacobian_residuals_avx2(
                    data_x, data_y, data_z, params, jacobian, residuals,
                );
            }
            return;
        }

        if cpu_features::has_sse4_1() {
            // SAFETY: We've checked that SSE4.1 is available
            unsafe {
                sse::fill_jacobian_residuals_sse(
                    data_x, data_y, data_z, params, jacobian, residuals,
                );
            }
            return;
        }

        fill_jacobian_residuals_scalar(data_x, data_y, data_z, params, jacobian, residuals);
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        fill_jacobian_residuals_scalar(data_x, data_y, data_z, params, jacobian, residuals);
    }
}

/// Compute chi² using the best available SIMD implementation.
///
/// Automatically dispatches to AVX2, SSE4.1, NEON, or scalar based on platform.
#[inline]
pub fn compute_chi2_simd(data_x: &[f32], data_y: &[f32], data_z: &[f32], params: &[f32; 6]) -> f32 {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64
        return unsafe { neon::compute_chi2_neon(data_x, data_y, data_z, params) };
    }

    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            // SAFETY: We've checked that AVX2+FMA is available
            return unsafe { avx2::compute_chi2_avx2(data_x, data_y, data_z, params) };
        }

        if cpu_features::has_sse4_1() {
            // SAFETY: We've checked that SSE4.1 is available
            return unsafe { sse::compute_chi2_sse(data_x, data_y, data_z, params) };
        }

        compute_chi2_scalar(data_x, data_y, data_z, params)
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        compute_chi2_scalar(data_x, data_y, data_z, params)
    }
}
