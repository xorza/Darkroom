//! SIMD-accelerated Moffat profile computations.
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
// Feature detection
// ============================================================================

/// Check if AVX2+FMA is available at runtime.
#[inline]
pub fn is_avx2_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        cpu_features::has_avx2_fma()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Check if SSE4.1 is available at runtime.
#[inline]
pub fn is_sse4_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        cpu_features::has_sse4_1()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

// ============================================================================
// Scalar fallback implementation
// ============================================================================

/// Resize vectors to n elements, reusing capacity when possible.
/// This avoids allocation if capacity >= n.
#[inline]
fn resize_buffers(jacobian: &mut Vec<[f32; 5]>, residuals: &mut Vec<f32>, n: usize) {
    // Clear and resize - Vec::resize only allocates if capacity < n
    jacobian.clear();
    residuals.clear();

    // Use resize_with to avoid unnecessary zeroing when we'll overwrite anyway
    if jacobian.capacity() >= n {
        // SAFETY: We're about to fill all n elements
        unsafe {
            jacobian.set_len(n);
            residuals.set_len(n);
        }
    } else {
        jacobian.resize(n, [0.0; 5]);
        residuals.resize(n, 0.0);
    }
}

/// Scalar implementation of Jacobian/residual computation.
#[inline]
pub fn fill_jacobian_residuals_scalar(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 5],
    beta: f32,
    jacobian: &mut Vec<[f32; 5]>,
    residuals: &mut Vec<f32>,
) {
    let n = data_x.len();
    resize_buffers(jacobian, residuals, n);

    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;

    for i in 0..n {
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

/// Scalar implementation of chi² computation.
#[inline]
pub fn compute_chi2_scalar(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 5],
    beta: f32,
) -> f32 {
    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;
    let mut chi2 = 0.0f32;

    for i in 0..data_x.len() {
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

// ============================================================================
// Dispatch functions
// ============================================================================

/// Fill Jacobian and residuals using the best available SIMD implementation.
///
/// Automatically dispatches to AVX2, SSE4.1, NEON, or scalar based on platform.
#[inline]
pub fn fill_jacobian_residuals_simd_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 5],
    beta: f32,
    jacobian: &mut Vec<[f32; 5]>,
    residuals: &mut Vec<f32>,
) {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64
        unsafe {
            neon::fill_jacobian_residuals_neon_fixed_beta(
                data_x, data_y, data_z, params, beta, jacobian, residuals,
            );
        }
        return;
    }

    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            // SAFETY: We've checked that AVX2+FMA is available
            unsafe {
                avx2::fill_jacobian_residuals_simd_fixed_beta(
                    data_x, data_y, data_z, params, beta, jacobian, residuals,
                );
            }
            return;
        }

        if cpu_features::has_sse4_1() {
            // SAFETY: We've checked that SSE4.1 is available
            unsafe {
                sse::fill_jacobian_residuals_sse_fixed_beta(
                    data_x, data_y, data_z, params, beta, jacobian, residuals,
                );
            }
            return;
        }

        fill_jacobian_residuals_scalar(data_x, data_y, data_z, params, beta, jacobian, residuals);
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        fill_jacobian_residuals_scalar(data_x, data_y, data_z, params, beta, jacobian, residuals);
    }
}

/// Compute chi² using the best available SIMD implementation.
///
/// Automatically dispatches to AVX2, SSE4.1, NEON, or scalar based on platform.
#[inline]
pub fn compute_chi2_simd_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 5],
    beta: f32,
) -> f32 {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64
        return unsafe { neon::compute_chi2_neon_fixed_beta(data_x, data_y, data_z, params, beta) };
    }

    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            // SAFETY: We've checked that AVX2+FMA is available
            return unsafe {
                avx2::compute_chi2_simd_fixed_beta(data_x, data_y, data_z, params, beta)
            };
        }

        if cpu_features::has_sse4_1() {
            // SAFETY: We've checked that SSE4.1 is available
            return unsafe {
                sse::compute_chi2_sse_fixed_beta(data_x, data_y, data_z, params, beta)
            };
        }

        compute_chi2_scalar(data_x, data_y, data_z, params, beta)
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        compute_chi2_scalar(data_x, data_y, data_z, params, beta)
    }
}
