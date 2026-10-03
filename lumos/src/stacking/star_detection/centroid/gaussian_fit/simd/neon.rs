//! NEON SIMD implementation for the elliptical `Gaussian2D` batch operations (aarch64).
//!
//! Processes 2 f64 pixels per NEON iteration for `batch_build_normal_equations`
//! and `batch_compute_chi2`. Uses a fast polynomial `exp()` approximation
//! (Cephes-derived, ~1e-13 relative accuracy) fully vectorized in NEON.

use std::f64::consts::LOG2_E;

use crate::stacking::star_detection::centroid::gaussian_fit::Gaussian2D;
use crate::stacking::star_detection::centroid::gaussian_fit::simd::exp_poly::{
    EXP_P0, EXP_P1, EXP_P2, EXP_Q0, EXP_Q1, EXP_Q2, EXP_Q3, LN2_HI, LN2_LO,
};
use crate::stacking::star_detection::centroid::lm_optimizer::{FitData, LMModel, NormalEquations};
use crate::stacking::star_detection::centroid::simd::hsum;
use std::arch::aarch64::*;

const LOG2E: f64 = LOG2_E;

/// Fast vectorized `exp()` for 2 f64 lanes using Cephes polynomial approximation.
#[inline]
unsafe fn simd_exp_fast(x: float64x2_t) -> float64x2_t {
    unsafe {
        // Clamp to avoid overflow/underflow in IEEE 754
        let v_min = vdupq_n_f64(-708.0);
        let v_max = vdupq_n_f64(709.0);
        let x = vmaxq_f64(vminq_f64(x, v_max), v_min);

        // Range reduction: n = floor(x * log2(e) + 0.5)
        let v_log2e = vdupq_n_f64(LOG2E);
        let v_half = vdupq_n_f64(0.5);
        let n_real = vfmaq_f64(v_half, x, v_log2e);
        let n_real = vrndmq_f64(n_real); // floor

        // r = x - n * ln(2), using two-part ln(2) for precision
        let v_ln2_hi = vdupq_n_f64(LN2_HI);
        let v_ln2_lo = vdupq_n_f64(LN2_LO);
        let r = vsubq_f64(x, vmulq_f64(n_real, v_ln2_hi));
        let r = vsubq_f64(r, vmulq_f64(n_real, v_ln2_lo));

        // Polynomial evaluation: P(r²) and Q(r²)
        let r2 = vmulq_f64(r, r);

        // P(r) = r * ((P0 * r² + P1) * r² + P2)
        let px = vfmaq_f64(vdupq_n_f64(EXP_P1), vdupq_n_f64(EXP_P0), r2);
        let px = vfmaq_f64(vdupq_n_f64(EXP_P2), px, r2);
        let px = vmulq_f64(px, r);

        // Q(r) = ((Q0 * r² + Q1) * r² + Q2) * r² + Q3
        let qx = vfmaq_f64(vdupq_n_f64(EXP_Q1), vdupq_n_f64(EXP_Q0), r2);
        let qx = vfmaq_f64(vdupq_n_f64(EXP_Q2), qx, r2);
        let qx = vfmaq_f64(vdupq_n_f64(EXP_Q3), qx, r2);

        // exp(r) = 1 + 2*px / (qx - px)
        let v_one = vdupq_n_f64(1.0);
        let v_two = vdupq_n_f64(2.0);
        let denom = vsubq_f64(qx, px);
        let frac = vdivq_f64(px, denom);
        let exp_r = vfmaq_f64(v_one, v_two, frac);

        // Reconstruct: exp(x) = 2^n * exp(r)
        // Convert n to i64, add IEEE 754 exponent bias (1023), shift left by 52
        let n_i64 = vcvtq_s64_f64(n_real);
        let bias = vdupq_n_s64(1023);
        let n_biased = vaddq_s64(n_i64, bias);
        let pow2n = vshlq_n_s64::<52>(n_biased);
        let pow2n: float64x2_t = vreinterpretq_f64_s64(pow2n);

        vmulq_f64(exp_r, pow2n)
    }
}

/// Batch build normal equations (J^T J, J^T r, chi²) for the elliptical Gaussian.
///
/// For N=7, accumulates 28 upper-triangle hessian elements, 7 gradient elements, and chi² in
/// registers (36 total). The Jacobian row is the model's own (see
/// [`Gaussian2D::evaluate_and_jacobian`]), with the background's `∂f/∂bg = 1` folded into plain
/// additions.
///
/// # Safety
/// Caller must run on aarch64, where NEON is baseline.
pub(super) unsafe fn batch_build_normal_equations_neon(
    model: &Gaussian2D,
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 7],
) -> NormalEquations<7> {
    let n = data_x.len();
    let [x0, y0, amp, a, b, c, bg] = *params;

    unsafe {
        let v_x0 = vdupq_n_f64(x0);
        let v_y0 = vdupq_n_f64(y0);
        let v_amp = vdupq_n_f64(amp);
        let v_bg = vdupq_n_f64(bg);
        let v_a = vdupq_n_f64(a);
        let v_b = vdupq_n_f64(b);
        let v_c = vdupq_n_f64(c);
        let v_neg_half = vdupq_n_f64(-0.5);
        let v_one = vdupq_n_f64(1.0);
        let zero = vdupq_n_f64(0.0);
        let mut v_chi2 = zero;
        let mut v_g0 = zero;
        let mut v_g1 = zero;
        let mut v_g2 = zero;
        let mut v_g3 = zero;
        let mut v_g4 = zero;
        let mut v_g5 = zero;
        let mut v_g6 = zero;
        let mut v_h00 = zero;
        let mut v_h01 = zero;
        let mut v_h02 = zero;
        let mut v_h03 = zero;
        let mut v_h04 = zero;
        let mut v_h05 = zero;
        let mut v_h06 = zero;
        let mut v_h11 = zero;
        let mut v_h12 = zero;
        let mut v_h13 = zero;
        let mut v_h14 = zero;
        let mut v_h15 = zero;
        let mut v_h16 = zero;
        let mut v_h22 = zero;
        let mut v_h23 = zero;
        let mut v_h24 = zero;
        let mut v_h25 = zero;
        let mut v_h26 = zero;
        let mut v_h33 = zero;
        let mut v_h34 = zero;
        let mut v_h35 = zero;
        let mut v_h36 = zero;
        let mut v_h44 = zero;
        let mut v_h45 = zero;
        let mut v_h46 = zero;
        let mut v_h55 = zero;
        let mut v_h56 = zero;
        let mut v_h66 = zero;

        let chunks = n / 2;

        for chunk in 0..chunks {
            let base = chunk * 2;
            let vx = vld1q_f64(data_x.as_ptr().add(base));
            let vy = vld1q_f64(data_y.as_ptr().add(base));
            let vz = vld1q_f64(data_z.as_ptr().add(base));

            let dx = vsubq_f64(vx, v_x0);
            let dy = vsubq_f64(vy, v_y0);
            // t = a·dx + b·dy and u = b·dx + c·dy, so the quadratic form is dx·t + dy·u.
            let t = vfmaq_f64(vmulq_f64(v_a, dx), v_b, dy);
            let u = vfmaq_f64(vmulq_f64(v_b, dx), v_c, dy);
            let q = vfmaq_f64(vmulq_f64(dx, t), dy, u);
            let exp_val = simd_exp_fast(vmulq_f64(v_neg_half, q));
            let amp_exp = vmulq_f64(v_amp, exp_val);
            let residual = vsubq_f64(vz, vaddq_f64(amp_exp, v_bg));
            v_chi2 = vfmaq_f64(v_chi2, residual, residual);

            let half_amp_exp = vmulq_f64(v_neg_half, amp_exp);
            let j0 = vmulq_f64(amp_exp, t);
            let j1 = vmulq_f64(amp_exp, u);
            let j2 = exp_val;
            let j3 = vmulq_f64(half_amp_exp, vmulq_f64(dx, dx));
            let j4 = vmulq_f64(vmulq_f64(half_amp_exp, dx), vaddq_f64(dy, dy));
            let j5 = vmulq_f64(half_amp_exp, vmulq_f64(dy, dy));
            v_g0 = vfmaq_f64(v_g0, j0, residual);
            v_g1 = vfmaq_f64(v_g1, j1, residual);
            v_g2 = vfmaq_f64(v_g2, j2, residual);
            v_g3 = vfmaq_f64(v_g3, j3, residual);
            v_g4 = vfmaq_f64(v_g4, j4, residual);
            v_g5 = vfmaq_f64(v_g5, j5, residual);
            v_g6 = vaddq_f64(v_g6, residual);
            v_h00 = vfmaq_f64(v_h00, j0, j0);
            v_h01 = vfmaq_f64(v_h01, j0, j1);
            v_h02 = vfmaq_f64(v_h02, j0, j2);
            v_h03 = vfmaq_f64(v_h03, j0, j3);
            v_h04 = vfmaq_f64(v_h04, j0, j4);
            v_h05 = vfmaq_f64(v_h05, j0, j5);
            v_h06 = vaddq_f64(v_h06, j0);
            v_h11 = vfmaq_f64(v_h11, j1, j1);
            v_h12 = vfmaq_f64(v_h12, j1, j2);
            v_h13 = vfmaq_f64(v_h13, j1, j3);
            v_h14 = vfmaq_f64(v_h14, j1, j4);
            v_h15 = vfmaq_f64(v_h15, j1, j5);
            v_h16 = vaddq_f64(v_h16, j1);
            v_h22 = vfmaq_f64(v_h22, j2, j2);
            v_h23 = vfmaq_f64(v_h23, j2, j3);
            v_h24 = vfmaq_f64(v_h24, j2, j4);
            v_h25 = vfmaq_f64(v_h25, j2, j5);
            v_h26 = vaddq_f64(v_h26, j2);
            v_h33 = vfmaq_f64(v_h33, j3, j3);
            v_h34 = vfmaq_f64(v_h34, j3, j4);
            v_h35 = vfmaq_f64(v_h35, j3, j5);
            v_h36 = vaddq_f64(v_h36, j3);
            v_h44 = vfmaq_f64(v_h44, j4, j4);
            v_h45 = vfmaq_f64(v_h45, j4, j5);
            v_h46 = vaddq_f64(v_h46, j4);
            v_h55 = vfmaq_f64(v_h55, j5, j5);
            v_h56 = vaddq_f64(v_h56, j5);
            v_h66 = vaddq_f64(v_h66, v_one);
        }

        let chi2 = hsum(v_chi2);
        let gradient = [
            hsum(v_g0),
            hsum(v_g1),
            hsum(v_g2),
            hsum(v_g3),
            hsum(v_g4),
            hsum(v_g5),
            hsum(v_g6),
        ];
        let mut hessian = [[0.0f64; 7]; 7];
        hessian[0][0] = hsum(v_h00);
        hessian[0][1] = hsum(v_h01);
        hessian[0][2] = hsum(v_h02);
        hessian[0][3] = hsum(v_h03);
        hessian[0][4] = hsum(v_h04);
        hessian[0][5] = hsum(v_h05);
        hessian[0][6] = hsum(v_h06);
        hessian[1][1] = hsum(v_h11);
        hessian[1][2] = hsum(v_h12);
        hessian[1][3] = hsum(v_h13);
        hessian[1][4] = hsum(v_h14);
        hessian[1][5] = hsum(v_h15);
        hessian[1][6] = hsum(v_h16);
        hessian[2][2] = hsum(v_h22);
        hessian[2][3] = hsum(v_h23);
        hessian[2][4] = hsum(v_h24);
        hessian[2][5] = hsum(v_h25);
        hessian[2][6] = hsum(v_h26);
        hessian[3][3] = hsum(v_h33);
        hessian[3][4] = hsum(v_h34);
        hessian[3][5] = hsum(v_h35);
        hessian[3][6] = hsum(v_h36);
        hessian[4][4] = hsum(v_h44);
        hessian[4][5] = hsum(v_h45);
        hessian[4][6] = hsum(v_h46);
        hessian[5][5] = hsum(v_h55);
        hessian[5][6] = hsum(v_h56);
        hessian[6][6] = hsum(v_h66);

        let mut equations = NormalEquations {
            hessian,
            gradient,
            chi2,
        };

        // Scalar tail (pixels past the last full 2-wide chunk)
        let tail_start = chunks * 2;
        equations.accumulate(
            model,
            FitData::unweighted(data_x, data_y, data_z),
            params,
            tail_start..n,
        );

        equations.mirror_lower_triangle();
        equations
    }
}

/// Batch compute chi² for the elliptical Gaussian.
///
/// # Safety
/// Caller must run on aarch64, where NEON is baseline.
pub(super) unsafe fn batch_compute_chi2_neon(
    model: &Gaussian2D,
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 7],
) -> f64 {
    let n = data_x.len();
    let [x0, y0, amp, a, b, c, bg] = *params;

    unsafe {
        let v_x0 = vdupq_n_f64(x0);
        let v_y0 = vdupq_n_f64(y0);
        let v_amp = vdupq_n_f64(amp);
        let v_bg = vdupq_n_f64(bg);
        let v_a = vdupq_n_f64(a);
        let v_b = vdupq_n_f64(b);
        let v_c = vdupq_n_f64(c);
        let v_neg_half = vdupq_n_f64(-0.5);

        let mut v_chi2 = vdupq_n_f64(0.0);
        let chunks = n / 2;

        for chunk in 0..chunks {
            let base = chunk * 2;
            let vx = vld1q_f64(data_x.as_ptr().add(base));
            let vy = vld1q_f64(data_y.as_ptr().add(base));
            let vz = vld1q_f64(data_z.as_ptr().add(base));

            let dx = vsubq_f64(vx, v_x0);
            let dy = vsubq_f64(vy, v_y0);
            let t = vfmaq_f64(vmulq_f64(v_a, dx), v_b, dy);
            let u = vfmaq_f64(vmulq_f64(v_b, dx), v_c, dy);
            let q = vfmaq_f64(vmulq_f64(dx, t), dy, u);
            let exp_val = simd_exp_fast(vmulq_f64(v_neg_half, q));
            let model_val = vfmaq_f64(v_bg, v_amp, exp_val);
            let residual = vsubq_f64(vz, model_val);
            v_chi2 = vfmaq_f64(v_chi2, residual, residual);
        }

        let mut chi2 = hsum(v_chi2);

        // Scalar tail (pixels past the last full 2-wide chunk)
        let tail_start = chunks * 2;
        chi2 += model.accumulate_chi2(
            FitData::unweighted(data_x, data_y, data_z),
            params,
            tail_start..n,
        );

        chi2
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use crate::stacking::star_detection::centroid::gaussian_fit::simd::neon::*;

    /// Test that `simd_exp_fast` produces results close to std `exp()`.
    #[test]
    fn simd_exp_fast_accuracy() {
        let test_values: &[f64] = &[
            0.0, 1.0, -1.0, 0.5, -0.5, 2.0, -2.0, 5.0, -5.0, 10.0, -10.0, -50.0, -100.0, -500.0,
            -700.0, 0.001, -0.001, 0.1, -0.1, PI, -PI, 100.0, 500.0, 700.0,
        ];

        for &x in test_values {
            let result = unsafe {
                let v = vdupq_n_f64(x);
                let r = simd_exp_fast(v);
                vgetq_lane_f64::<0>(r)
            };
            let expected = x.exp();

            if expected == 0.0 || !expected.is_finite() {
                continue;
            }

            let rel_err = (result - expected).abs() / expected.abs();
            assert!(
                rel_err < 1e-12,
                "exp({x}) = {expected}, got {result}, rel_err = {rel_err:.2e}"
            );
        }
    }

    /// Test `simd_exp_fast` with the typical Gaussian exponent range.
    #[test]
    fn simd_exp_fast_gaussian_range() {
        for i in 0..1000 {
            let x = -f64::from(i) * 0.5;
            let result = unsafe {
                let v = vdupq_n_f64(x);
                let r = simd_exp_fast(v);
                vgetq_lane_f64::<0>(r)
            };
            let expected = x.exp();

            if expected < 1e-300 {
                assert!(result < 1e-290, "exp({x}): expected ~0, got {result}");
                continue;
            }

            let rel_err = (result - expected).abs() / expected.abs();
            assert!(
                rel_err < 1e-12,
                "exp({x}) = {expected}, got {result}, rel_err = {rel_err:.2e}"
            );
        }
    }
}
