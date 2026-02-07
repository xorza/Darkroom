//! NEON SIMD implementation for MoffatFixedBeta batch operations (aarch64).
//!
//! Processes 2 f64 pixels per NEON iteration for `batch_build_normal_equations`
//! and `batch_compute_chi2`. Supports HalfInt, Int, and General PowStrategy variants.

use super::super::lm_optimizer::LMModel;
use super::{MoffatFixedBeta, PowStrategy};
use std::arch::aarch64::*;

/// SIMD `int_pow`: compute u^n for each lane using repeated squaring.
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn simd_int_pow(u: float64x2_t, n: u32) -> float64x2_t {
    match n {
        0 => vdupq_n_f64(1.0),
        1 => u,
        2 => vmulq_f64(u, u),
        3 => vmulq_f64(vmulq_f64(u, u), u),
        4 => {
            let u2 = vmulq_f64(u, u);
            vmulq_f64(u2, u2)
        }
        5 => {
            let u2 = vmulq_f64(u, u);
            vmulq_f64(vmulq_f64(u2, u2), u)
        }
        _ => {
            let mut result = vdupq_n_f64(1.0);
            let mut base = u;
            let mut exp = n;
            while exp > 0 {
                if exp & 1 == 1 {
                    result = vmulq_f64(result, base);
                }
                base = vmulq_f64(base, base);
                exp >>= 1;
            }
            result
        }
    }
}

/// SIMD `fast_pow_neg`: compute u^(-beta) for 2 lanes at once.
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn simd_fast_pow_neg(u: float64x2_t, strategy: PowStrategy) -> float64x2_t {
    match strategy {
        PowStrategy::HalfInt { int_part } => {
            // u^(-(n+0.5)) = 1 / (u^n * sqrt(u))
            let u_n = simd_int_pow(u, int_part);
            let sqrt_u = vsqrtq_f64(u);
            let denom = vmulq_f64(u_n, sqrt_u);
            vdivq_f64(vdupq_n_f64(1.0), denom)
        }
        PowStrategy::Int { n } => vdivq_f64(vdupq_n_f64(1.0), simd_int_pow(u, n)),
        PowStrategy::General { neg_beta } => {
            let u0 = vgetq_lane_f64::<0>(u);
            let u1 = vgetq_lane_f64::<1>(u);
            let r0 = u0.powf(neg_beta);
            let r1 = u1.powf(neg_beta);
            let mut result = vdupq_n_f64(0.0);
            result = vsetq_lane_f64::<0>(r0, result);
            result = vsetq_lane_f64::<1>(r1, result);
            result
        }
    }
}

/// Batch build normal equations (J^T J, J^T r, chi²) using NEON.
///
/// For N=5 (MoffatFixedBeta), accumulates 15 upper-triangle hessian elements,
/// 5 gradient elements, and chi² directly in NEON registers.
#[allow(clippy::needless_range_loop)]
pub(super) unsafe fn batch_build_normal_equations_neon(
    model: &MoffatFixedBeta,
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 5],
) -> ([[f64; 5]; 5], [f64; 5], f64) {
    let n = data_x.len();
    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;

    unsafe {
        let v_x0 = vdupq_n_f64(x0);
        let v_y0 = vdupq_n_f64(y0);
        let v_amp = vdupq_n_f64(amp);
        let v_inv_alpha2 = vdupq_n_f64(1.0 / alpha2);
        let v_bg = vdupq_n_f64(bg);
        let v_one = vdupq_n_f64(1.0);
        let v_common_factor = vdupq_n_f64(2.0 * amp * model.beta / alpha2);
        let v_inv_alpha = vdupq_n_f64(1.0 / alpha);
        let zero = vdupq_n_f64(0.0);

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

        let chunks = n / 2;

        for chunk in 0..chunks {
            let base = chunk * 2;
            let vx = vld1q_f64(data_x.as_ptr().add(base));
            let vy = vld1q_f64(data_y.as_ptr().add(base));
            let vz = vld1q_f64(data_z.as_ptr().add(base));

            let dx = vsubq_f64(vx, v_x0);
            let dy = vsubq_f64(vy, v_y0);
            let r2 = vfmaq_f64(vmulq_f64(dy, dy), dx, dx);
            let u = vfmaq_f64(v_one, r2, v_inv_alpha2);
            let u_neg_beta = simd_fast_pow_neg(u, model.pow_strategy);
            let model_val = vfmaq_f64(v_bg, v_amp, u_neg_beta);
            let residual = vsubq_f64(vz, model_val);

            // Chi²
            v_chi2 = vfmaq_f64(v_chi2, residual, residual);

            // Jacobian
            let u_neg_beta_m1 = vdivq_f64(u_neg_beta, u);
            let common = vmulq_f64(v_common_factor, u_neg_beta_m1);
            let j0 = vmulq_f64(common, dx);
            let j1 = vmulq_f64(common, dy);
            let j2 = u_neg_beta;
            let j3 = vmulq_f64(vmulq_f64(common, r2), v_inv_alpha);
            // j4 = 1.0 (implicit)

            // Gradient
            v_g0 = vfmaq_f64(v_g0, j0, residual);
            v_g1 = vfmaq_f64(v_g1, j1, residual);
            v_g2 = vfmaq_f64(v_g2, j2, residual);
            v_g3 = vfmaq_f64(v_g3, j3, residual);
            v_g4 = vaddq_f64(v_g4, residual); // j4=1

            // Hessian upper triangle
            // Row 0
            v_h00 = vfmaq_f64(v_h00, j0, j0);
            v_h01 = vfmaq_f64(v_h01, j0, j1);
            v_h02 = vfmaq_f64(v_h02, j0, j2);
            v_h03 = vfmaq_f64(v_h03, j0, j3);
            v_h04 = vaddq_f64(v_h04, j0); // j4=1

            // Row 1
            v_h11 = vfmaq_f64(v_h11, j1, j1);
            v_h12 = vfmaq_f64(v_h12, j1, j2);
            v_h13 = vfmaq_f64(v_h13, j1, j3);
            v_h14 = vaddq_f64(v_h14, j1); // j4=1

            // Row 2
            v_h22 = vfmaq_f64(v_h22, j2, j2);
            v_h23 = vfmaq_f64(v_h23, j2, j3);
            v_h24 = vaddq_f64(v_h24, j2); // j4=1

            // Row 3
            v_h33 = vfmaq_f64(v_h33, j3, j3);
            v_h34 = vaddq_f64(v_h34, j3); // j4=1

            // Row 4
            v_h44 = vaddq_f64(v_h44, v_one); // j4=1
        }

        // Horizontal sums
        #[inline(always)]
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe fn hsum(v: float64x2_t) -> f64 {
            vaddvq_f64(v)
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
        let tail_start = chunks * 2;
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

/// Batch compute chi² using NEON.
pub(super) unsafe fn batch_compute_chi2_neon(
    model: &MoffatFixedBeta,
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 5],
) -> f64 {
    let n = data_x.len();
    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;

    unsafe {
        let v_x0 = vdupq_n_f64(x0);
        let v_y0 = vdupq_n_f64(y0);
        let v_amp = vdupq_n_f64(amp);
        let v_inv_alpha2 = vdupq_n_f64(1.0 / alpha2);
        let v_bg = vdupq_n_f64(bg);
        let v_one = vdupq_n_f64(1.0);

        let mut v_chi2 = vdupq_n_f64(0.0);
        let chunks = n / 2;

        for chunk in 0..chunks {
            let base = chunk * 2;
            let vx = vld1q_f64(data_x.as_ptr().add(base));
            let vy = vld1q_f64(data_y.as_ptr().add(base));
            let vz = vld1q_f64(data_z.as_ptr().add(base));

            let dx = vsubq_f64(vx, v_x0);
            let dy = vsubq_f64(vy, v_y0);
            let r2 = vfmaq_f64(vmulq_f64(dy, dy), dx, dx);
            let u = vfmaq_f64(v_one, r2, v_inv_alpha2);
            let u_neg_beta = simd_fast_pow_neg(u, model.pow_strategy);
            let model_val = vfmaq_f64(v_bg, v_amp, u_neg_beta);
            let residual = vsubq_f64(vz, model_val);
            v_chi2 = vfmaq_f64(v_chi2, residual, residual);
        }

        let mut chi2 = vaddvq_f64(v_chi2);

        // Scalar tail
        let tail_start = chunks * 2;
        for i in tail_start..n {
            let residual = data_z[i] - model.evaluate(data_x[i], data_y[i], params);
            chi2 += residual * residual;
        }

        chi2
    }
}
