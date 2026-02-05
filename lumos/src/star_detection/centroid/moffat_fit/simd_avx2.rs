//! AVX2+FMA SIMD implementation for MoffatFixedBeta batch operations.
//!
//! Processes 4 f64 pixels per AVX2 iteration for `evaluate_and_jacobian`
//! and `compute_chi2`. Supports HalfInt, Int, and General PowStrategy variants.

use super::super::lm_optimizer::LMModel;
use super::{MoffatFixedBeta, PowStrategy};
use std::arch::x86_64::*;

/// SIMD `int_pow`: compute u^n for each lane using repeated squaring.
#[target_feature(enable = "avx2,fma")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn simd_int_pow(u: __m256d, n: u32) -> __m256d {
    match n {
        0 => _mm256_set1_pd(1.0),
        1 => u,
        2 => _mm256_mul_pd(u, u),
        3 => _mm256_mul_pd(_mm256_mul_pd(u, u), u),
        4 => {
            let u2 = _mm256_mul_pd(u, u);
            _mm256_mul_pd(u2, u2)
        }
        5 => {
            let u2 = _mm256_mul_pd(u, u);
            _mm256_mul_pd(_mm256_mul_pd(u2, u2), u)
        }
        _ => {
            let mut result = _mm256_set1_pd(1.0);
            let mut base = u;
            let mut exp = n;
            while exp > 0 {
                if exp & 1 == 1 {
                    result = _mm256_mul_pd(result, base);
                }
                base = _mm256_mul_pd(base, base);
                exp >>= 1;
            }
            result
        }
    }
}

/// SIMD `fast_pow_neg`: compute u^(-beta) for 4 lanes at once.
#[target_feature(enable = "avx2,fma")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn simd_fast_pow_neg(u: __m256d, strategy: PowStrategy) -> __m256d {
    match strategy {
        PowStrategy::HalfInt { int_part } => {
            // u^(-(n+0.5)) = 1 / (u^n * sqrt(u))
            let u_n = simd_int_pow(u, int_part);
            let sqrt_u = _mm256_sqrt_pd(u);
            let denom = _mm256_mul_pd(u_n, sqrt_u);
            _mm256_div_pd(_mm256_set1_pd(1.0), denom)
        }
        PowStrategy::Int { n } => _mm256_div_pd(_mm256_set1_pd(1.0), simd_int_pow(u, n)),
        PowStrategy::General { neg_beta } => {
            let mut u_arr = [0.0f64; 4];
            _mm256_storeu_pd(u_arr.as_mut_ptr(), u);
            let result = [
                u_arr[0].powf(neg_beta),
                u_arr[1].powf(neg_beta),
                u_arr[2].powf(neg_beta),
                u_arr[3].powf(neg_beta),
            ];
            _mm256_loadu_pd(result.as_ptr())
        }
    }
}

/// Batch fill jacobian rows and residuals using AVX2+FMA.
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available on the current CPU.
#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn batch_fill_jacobian_residuals_avx2(
    model: &MoffatFixedBeta,
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 5],
    jacobian: &mut Vec<[f64; 5]>,
    residuals: &mut Vec<f64>,
) -> f64 {
    let n = data_x.len();
    jacobian.clear();
    residuals.clear();
    jacobian.reserve(n);
    residuals.reserve(n);

    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;

    // SAFETY: AVX2+FMA guaranteed by #[target_feature] + caller check.
    unsafe {
        let v_x0 = _mm256_set1_pd(x0);
        let v_y0 = _mm256_set1_pd(y0);
        let v_amp = _mm256_set1_pd(amp);
        let v_inv_alpha2 = _mm256_set1_pd(1.0 / alpha2);
        let v_bg = _mm256_set1_pd(bg);
        let v_one = _mm256_set1_pd(1.0);
        let v_common_factor = _mm256_set1_pd(2.0 * amp * model.beta / alpha2);
        let v_inv_alpha = _mm256_set1_pd(1.0 / alpha);

        let mut v_chi2 = _mm256_setzero_pd();

        let chunks = n / 4;

        for chunk in 0..chunks {
            let base = chunk * 4;
            let vx = _mm256_loadu_pd(data_x.as_ptr().add(base));
            let vy = _mm256_loadu_pd(data_y.as_ptr().add(base));
            let vz = _mm256_loadu_pd(data_z.as_ptr().add(base));

            let dx = _mm256_sub_pd(vx, v_x0);
            let dy = _mm256_sub_pd(vy, v_y0);
            let r2 = _mm256_fmadd_pd(dx, dx, _mm256_mul_pd(dy, dy));
            let u = _mm256_fmadd_pd(r2, v_inv_alpha2, v_one);
            let u_neg_beta = simd_fast_pow_neg(u, model.pow_strategy);
            let model_val = _mm256_fmadd_pd(v_amp, u_neg_beta, v_bg);
            let residual = _mm256_sub_pd(vz, model_val);
            v_chi2 = _mm256_fmadd_pd(residual, residual, v_chi2);

            let u_neg_beta_m1 = _mm256_div_pd(u_neg_beta, u);
            let common = _mm256_mul_pd(v_common_factor, u_neg_beta_m1);
            let j0 = _mm256_mul_pd(common, dx);
            let j1 = _mm256_mul_pd(common, dy);
            let j2 = u_neg_beta;
            let j3 = _mm256_mul_pd(_mm256_mul_pd(common, r2), v_inv_alpha);

            let mut res_arr = [0.0f64; 4];
            let mut j0_arr = [0.0f64; 4];
            let mut j1_arr = [0.0f64; 4];
            let mut j2_arr = [0.0f64; 4];
            let mut j3_arr = [0.0f64; 4];
            _mm256_storeu_pd(res_arr.as_mut_ptr(), residual);
            _mm256_storeu_pd(j0_arr.as_mut_ptr(), j0);
            _mm256_storeu_pd(j1_arr.as_mut_ptr(), j1);
            _mm256_storeu_pd(j2_arr.as_mut_ptr(), j2);
            _mm256_storeu_pd(j3_arr.as_mut_ptr(), j3);

            for i in 0..4 {
                residuals.push(res_arr[i]);
                jacobian.push([j0_arr[i], j1_arr[i], j2_arr[i], j3_arr[i], 1.0]);
            }
        }

        let mut chi2_arr = [0.0f64; 4];
        _mm256_storeu_pd(chi2_arr.as_mut_ptr(), v_chi2);
        let mut chi2 = chi2_arr[0] + chi2_arr[1] + chi2_arr[2] + chi2_arr[3];

        // Scalar tail
        let tail_start = chunks * 4;
        for i in tail_start..n {
            let (model_val, jac_row) = model.evaluate_and_jacobian(data_x[i], data_y[i], params);
            let residual = data_z[i] - model_val;
            chi2 += residual * residual;
            jacobian.push(jac_row);
            residuals.push(residual);
        }

        chi2
    }
}

/// Batch compute chi² using AVX2+FMA.
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available on the current CPU.
#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn batch_compute_chi2_avx2(
    model: &MoffatFixedBeta,
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 5],
) -> f64 {
    let n = data_x.len();
    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;

    // SAFETY: AVX2+FMA guaranteed by #[target_feature] + caller check.
    unsafe {
        let v_x0 = _mm256_set1_pd(x0);
        let v_y0 = _mm256_set1_pd(y0);
        let v_amp = _mm256_set1_pd(amp);
        let v_inv_alpha2 = _mm256_set1_pd(1.0 / alpha2);
        let v_bg = _mm256_set1_pd(bg);
        let v_one = _mm256_set1_pd(1.0);

        let mut v_chi2 = _mm256_setzero_pd();
        let chunks = n / 4;

        for chunk in 0..chunks {
            let base = chunk * 4;
            let vx = _mm256_loadu_pd(data_x.as_ptr().add(base));
            let vy = _mm256_loadu_pd(data_y.as_ptr().add(base));
            let vz = _mm256_loadu_pd(data_z.as_ptr().add(base));

            let dx = _mm256_sub_pd(vx, v_x0);
            let dy = _mm256_sub_pd(vy, v_y0);
            let r2 = _mm256_fmadd_pd(dx, dx, _mm256_mul_pd(dy, dy));
            let u = _mm256_fmadd_pd(r2, v_inv_alpha2, v_one);
            let u_neg_beta = simd_fast_pow_neg(u, model.pow_strategy);
            let model_val = _mm256_fmadd_pd(v_amp, u_neg_beta, v_bg);
            let residual = _mm256_sub_pd(vz, model_val);
            v_chi2 = _mm256_fmadd_pd(residual, residual, v_chi2);
        }

        let mut chi2_arr = [0.0f64; 4];
        _mm256_storeu_pd(chi2_arr.as_mut_ptr(), v_chi2);
        let mut chi2 = chi2_arr[0] + chi2_arr[1] + chi2_arr[2] + chi2_arr[3];

        // Scalar tail
        let tail_start = chunks * 4;
        for i in tail_start..n {
            let residual = data_z[i] - model.evaluate(data_x[i], data_y[i], params);
            chi2 += residual * residual;
        }

        chi2
    }
}
