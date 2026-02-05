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

/// Batch build normal equations (J^T J, J^T r, chi²) using AVX2+FMA.
/// Fuses model evaluation, Jacobian, and Hessian/gradient accumulation
/// to avoid storing intermediate jacobian/residuals arrays.
///
/// For N=5 (MoffatFixedBeta), accumulates 15 upper-triangle hessian elements,
/// 5 gradient elements, and chi² directly in AVX2 registers.
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available on the current CPU.
#[allow(clippy::needless_range_loop)]
#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn batch_build_normal_equations_avx2(
    model: &MoffatFixedBeta,
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 5],
) -> ([[f64; 5]; 5], [f64; 5], f64) {
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
        let v_common_factor = _mm256_set1_pd(2.0 * amp * model.beta / alpha2);
        let v_inv_alpha = _mm256_set1_pd(1.0 / alpha);
        let zero = _mm256_setzero_pd();

        // Accumulators: 15 upper-triangle hessian + 5 gradient + 1 chi²
        let mut v_chi2 = zero;
        let mut v_g0 = zero;
        let mut v_g1 = zero;
        let mut v_g2 = zero;
        let mut v_g3 = zero;
        let mut v_g4 = zero;
        let mut v_h00 = zero;
        let mut v_h01 = zero;
        let mut v_h02 = zero;
        let mut v_h03 = zero;
        let mut v_h04 = zero;
        let mut v_h11 = zero;
        let mut v_h12 = zero;
        let mut v_h13 = zero;
        let mut v_h14 = zero;
        let mut v_h22 = zero;
        let mut v_h23 = zero;
        let mut v_h24 = zero;
        let mut v_h33 = zero;
        let mut v_h34 = zero;
        let mut v_h44 = zero;

        let chunks = n / 4;

        for chunk in 0..chunks {
            let base = chunk * 4;
            let vx = _mm256_loadu_pd(data_x.as_ptr().add(base));
            let vy = _mm256_loadu_pd(data_y.as_ptr().add(base));
            let vz = _mm256_loadu_pd(data_z.as_ptr().add(base));

            // Model evaluation + Jacobian (same as before)
            let dx = _mm256_sub_pd(vx, v_x0);
            let dy = _mm256_sub_pd(vy, v_y0);
            let r2 = _mm256_fmadd_pd(dx, dx, _mm256_mul_pd(dy, dy));
            let u = _mm256_fmadd_pd(r2, v_inv_alpha2, v_one);
            let u_neg_beta = simd_fast_pow_neg(u, model.pow_strategy);
            let model_val = _mm256_fmadd_pd(v_amp, u_neg_beta, v_bg);
            let residual = _mm256_sub_pd(vz, model_val);

            // Chi²
            v_chi2 = _mm256_fmadd_pd(residual, residual, v_chi2);

            // Jacobian rows: j0=common*dx, j1=common*dy, j2=u_neg_beta, j3=common*r2/alpha, j4=1
            let u_neg_beta_m1 = _mm256_div_pd(u_neg_beta, u);
            let common = _mm256_mul_pd(v_common_factor, u_neg_beta_m1);
            let j0 = _mm256_mul_pd(common, dx);
            let j1 = _mm256_mul_pd(common, dy);
            let j2 = u_neg_beta;
            let j3 = _mm256_mul_pd(_mm256_mul_pd(common, r2), v_inv_alpha);
            // j4 = 1.0 (implicit)

            // Gradient: g[i] += j[i] * residual
            v_g0 = _mm256_fmadd_pd(j0, residual, v_g0);
            v_g1 = _mm256_fmadd_pd(j1, residual, v_g1);
            v_g2 = _mm256_fmadd_pd(j2, residual, v_g2);
            v_g3 = _mm256_fmadd_pd(j3, residual, v_g3);
            v_g4 = _mm256_add_pd(v_g4, residual); // j4=1, so g4 += residual

            // Hessian upper triangle: h[i][j] += j[i] * j[j]
            // Row 0: h00, h01, h02, h03, h04
            v_h00 = _mm256_fmadd_pd(j0, j0, v_h00);
            v_h01 = _mm256_fmadd_pd(j0, j1, v_h01);
            v_h02 = _mm256_fmadd_pd(j0, j2, v_h02);
            v_h03 = _mm256_fmadd_pd(j0, j3, v_h03);
            v_h04 = _mm256_add_pd(v_h04, j0); // j4=1, so h04 += j0

            // Row 1: h11, h12, h13, h14
            v_h11 = _mm256_fmadd_pd(j1, j1, v_h11);
            v_h12 = _mm256_fmadd_pd(j1, j2, v_h12);
            v_h13 = _mm256_fmadd_pd(j1, j3, v_h13);
            v_h14 = _mm256_add_pd(v_h14, j1); // j4=1

            // Row 2: h22, h23, h24
            v_h22 = _mm256_fmadd_pd(j2, j2, v_h22);
            v_h23 = _mm256_fmadd_pd(j2, j3, v_h23);
            v_h24 = _mm256_add_pd(v_h24, j2); // j4=1

            // Row 3: h33, h34
            v_h33 = _mm256_fmadd_pd(j3, j3, v_h33);
            v_h34 = _mm256_add_pd(v_h34, j3); // j4=1

            // Row 4: h44 = sum of j4*j4 = count of pixels (added after loop)
            v_h44 = _mm256_add_pd(v_h44, v_one); // j4=1, so h44 += 1
        }

        // Horizontal sum helper
        #[inline(always)]
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe fn hsum(v: __m256d) -> f64 {
            let mut arr = [0.0f64; 4];
            _mm256_storeu_pd(arr.as_mut_ptr(), v);
            arr[0] + arr[1] + arr[2] + arr[3]
        }

        let mut chi2 = hsum(v_chi2);
        let mut gradient = [hsum(v_g0), hsum(v_g1), hsum(v_g2), hsum(v_g3), hsum(v_g4)];
        let mut hessian = [[0.0f64; 5]; 5];
        hessian[0][0] = hsum(v_h00);
        hessian[0][1] = hsum(v_h01);
        hessian[0][2] = hsum(v_h02);
        hessian[0][3] = hsum(v_h03);
        hessian[0][4] = hsum(v_h04);
        hessian[1][1] = hsum(v_h11);
        hessian[1][2] = hsum(v_h12);
        hessian[1][3] = hsum(v_h13);
        hessian[1][4] = hsum(v_h14);
        hessian[2][2] = hsum(v_h22);
        hessian[2][3] = hsum(v_h23);
        hessian[2][4] = hsum(v_h24);
        hessian[3][3] = hsum(v_h33);
        hessian[3][4] = hsum(v_h34);
        hessian[4][4] = hsum(v_h44);

        // Scalar tail
        let tail_start = chunks * 4;
        for i in tail_start..n {
            let (model_val, row) = model.evaluate_and_jacobian(data_x[i], data_y[i], params);
            let r = data_z[i] - model_val;
            chi2 += r * r;
            for k in 0..5 {
                gradient[k] += row[k] * r;
                for j in k..5 {
                    hessian[k][j] += row[k] * row[j];
                }
            }
        }

        // Mirror upper triangle to lower
        for i in 1..5 {
            for j in 0..i {
                hessian[i][j] = hessian[j][i];
            }
        }

        (hessian, gradient, chi2)
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
