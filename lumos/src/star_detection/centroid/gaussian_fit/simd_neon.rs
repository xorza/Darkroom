//! NEON SIMD implementation for Gaussian2D batch operations (aarch64).
//!
//! Processes 2 f64 pixels per NEON iteration for `batch_build_normal_equations`
//! and `batch_compute_chi2`. Uses a fast polynomial `exp()` approximation
//! (Cephes-derived, ~1e-13 relative accuracy) fully vectorized in NEON.

use super::super::lm_optimizer::LMModel;
use super::{EXP_P0, EXP_P1, EXP_P2, EXP_Q0, EXP_Q1, EXP_Q2, EXP_Q3, Gaussian2D, LN2_HI, LN2_LO};
use std::arch::aarch64::*;

const LOG2E: f64 = std::f64::consts::LOG2_E;

/// Fast vectorized exp() for 2 f64 lanes using Cephes polynomial approximation.
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn simd_exp_fast(x: float64x2_t) -> float64x2_t {
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

/// Horizontal sum of 2 f64 lanes.
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn hsum(v: float64x2_t) -> f64 {
    vaddvq_f64(v)
}

/// Batch build normal equations (J^T J, J^T r, chi²) using NEON.
///
/// For N=6 (Gaussian2D), accumulates 21 upper-triangle hessian elements,
/// 6 gradient elements, and chi² directly in NEON registers (28 total).
#[allow(clippy::needless_range_loop)]
pub(super) unsafe fn batch_build_normal_equations_neon(
    _model: &Gaussian2D,
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 6],
) -> ([[f64; 6]; 6], [f64; 6], f64) {
    let n = data_x.len();
    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;
    let inv_sigma_x2 = 1.0 / sigma_x2;
    let inv_sigma_y2 = 1.0 / sigma_y2;

    unsafe {
        let v_x0 = vdupq_n_f64(x0);
        let v_y0 = vdupq_n_f64(y0);
        let v_amp = vdupq_n_f64(amp);
        let v_bg = vdupq_n_f64(bg);
        let v_inv_sx2 = vdupq_n_f64(inv_sigma_x2);
        let v_inv_sy2 = vdupq_n_f64(inv_sigma_y2);
        let v_neg_half = vdupq_n_f64(-0.5);
        let v_inv_sx3 = vdupq_n_f64(1.0 / (sigma_x2 * sigma_x));
        let v_inv_sy3 = vdupq_n_f64(1.0 / (sigma_y2 * sigma_y));
        let v_one = vdupq_n_f64(1.0);
        let zero = vdupq_n_f64(0.0);

        // 21 upper-triangle hessian + 6 gradient + 1 chi² = 28 accumulators
        let mut v_chi2 = zero;
        let mut v_g0 = zero;
        let mut v_g1 = zero;
        let mut v_g2 = zero;
        let mut v_g3 = zero;
        let mut v_g4 = zero;
        let mut v_g5 = zero;
        let mut v_h00 = zero;
        let mut v_h01 = zero;
        let mut v_h02 = zero;
        let mut v_h03 = zero;
        let mut v_h04 = zero;
        let mut v_h05 = zero;
        let mut v_h11 = zero;
        let mut v_h12 = zero;
        let mut v_h13 = zero;
        let mut v_h14 = zero;
        let mut v_h15 = zero;
        let mut v_h22 = zero;
        let mut v_h23 = zero;
        let mut v_h24 = zero;
        let mut v_h25 = zero;
        let mut v_h33 = zero;
        let mut v_h34 = zero;
        let mut v_h35 = zero;
        let mut v_h44 = zero;
        let mut v_h45 = zero;
        let mut v_h55 = zero;

        let chunks = n / 2;

        for chunk in 0..chunks {
            let base = chunk * 2;
            let vx = vld1q_f64(data_x.as_ptr().add(base));
            let vy = vld1q_f64(data_y.as_ptr().add(base));
            let vz = vld1q_f64(data_z.as_ptr().add(base));

            // dx, dy
            let dx = vsubq_f64(vx, v_x0);
            let dy = vsubq_f64(vy, v_y0);

            // dx², dy²
            let dx2 = vmulq_f64(dx, dx);
            let dy2 = vmulq_f64(dy, dy);

            // exponent = -0.5 * (dx²/σx² + dy²/σy²)
            let term_x = vmulq_f64(dx2, v_inv_sx2);
            let term_y = vfmaq_f64(term_x, dy2, v_inv_sy2);
            let exponent = vmulq_f64(v_neg_half, term_y);

            // exp_val = fast_exp(exponent)
            let exp_val = simd_exp_fast(exponent);

            // amp_exp = amp * exp_val
            let amp_exp = vmulq_f64(v_amp, exp_val);

            // model_val = amp_exp + bg
            let model_val = vaddq_f64(amp_exp, v_bg);
            let residual = vsubq_f64(vz, model_val);

            // chi²
            v_chi2 = vfmaq_f64(v_chi2, residual, residual);

            // Jacobian rows:
            let j0 = vmulq_f64(amp_exp, vmulq_f64(dx, v_inv_sx2));
            let j1 = vmulq_f64(amp_exp, vmulq_f64(dy, v_inv_sy2));
            let j2 = exp_val;
            let j3 = vmulq_f64(amp_exp, vmulq_f64(dx2, v_inv_sx3));
            let j4 = vmulq_f64(amp_exp, vmulq_f64(dy2, v_inv_sy3));
            // j5 = 1.0 (implicit)

            // Gradient: g[i] += j[i] * residual
            v_g0 = vfmaq_f64(v_g0, j0, residual);
            v_g1 = vfmaq_f64(v_g1, j1, residual);
            v_g2 = vfmaq_f64(v_g2, j2, residual);
            v_g3 = vfmaq_f64(v_g3, j3, residual);
            v_g4 = vfmaq_f64(v_g4, j4, residual);
            v_g5 = vaddq_f64(v_g5, residual); // j5=1

            // Hessian upper triangle: h[i][j] += j[i] * j[j]
            // Row 0
            v_h00 = vfmaq_f64(v_h00, j0, j0);
            v_h01 = vfmaq_f64(v_h01, j0, j1);
            v_h02 = vfmaq_f64(v_h02, j0, j2);
            v_h03 = vfmaq_f64(v_h03, j0, j3);
            v_h04 = vfmaq_f64(v_h04, j0, j4);
            v_h05 = vaddq_f64(v_h05, j0); // j5=1

            // Row 1
            v_h11 = vfmaq_f64(v_h11, j1, j1);
            v_h12 = vfmaq_f64(v_h12, j1, j2);
            v_h13 = vfmaq_f64(v_h13, j1, j3);
            v_h14 = vfmaq_f64(v_h14, j1, j4);
            v_h15 = vaddq_f64(v_h15, j1); // j5=1

            // Row 2
            v_h22 = vfmaq_f64(v_h22, j2, j2);
            v_h23 = vfmaq_f64(v_h23, j2, j3);
            v_h24 = vfmaq_f64(v_h24, j2, j4);
            v_h25 = vaddq_f64(v_h25, j2); // j5=1

            // Row 3
            v_h33 = vfmaq_f64(v_h33, j3, j3);
            v_h34 = vfmaq_f64(v_h34, j3, j4);
            v_h35 = vaddq_f64(v_h35, j3); // j5=1

            // Row 4
            v_h44 = vfmaq_f64(v_h44, j4, j4);
            v_h45 = vaddq_f64(v_h45, j4); // j5=1

            // Row 5: h55 += j5*j5 = 1
            v_h55 = vaddq_f64(v_h55, v_one);
        }

        // Horizontal sums
        let mut chi2 = hsum(v_chi2);
        let mut gradient = [
            hsum(v_g0),
            hsum(v_g1),
            hsum(v_g2),
            hsum(v_g3),
            hsum(v_g4),
            hsum(v_g5),
        ];
        let mut hessian = [[0.0f64; 6]; 6];
        hessian[0][0] = hsum(v_h00);
        hessian[0][1] = hsum(v_h01);
        hessian[0][2] = hsum(v_h02);
        hessian[0][3] = hsum(v_h03);
        hessian[0][4] = hsum(v_h04);
        hessian[0][5] = hsum(v_h05);
        hessian[1][1] = hsum(v_h11);
        hessian[1][2] = hsum(v_h12);
        hessian[1][3] = hsum(v_h13);
        hessian[1][4] = hsum(v_h14);
        hessian[1][5] = hsum(v_h15);
        hessian[2][2] = hsum(v_h22);
        hessian[2][3] = hsum(v_h23);
        hessian[2][4] = hsum(v_h24);
        hessian[2][5] = hsum(v_h25);
        hessian[3][3] = hsum(v_h33);
        hessian[3][4] = hsum(v_h34);
        hessian[3][5] = hsum(v_h35);
        hessian[4][4] = hsum(v_h44);
        hessian[4][5] = hsum(v_h45);
        hessian[5][5] = hsum(v_h55);

        // Scalar tail (0 or 1 element)
        let model = Gaussian2D {
            stamp_radius: _model.stamp_radius,
        };
        let tail_start = chunks * 2;
        for i in tail_start..n {
            let (model_val, row) = model.evaluate_and_jacobian(data_x[i], data_y[i], params);
            let r = data_z[i] - model_val;
            chi2 += r * r;
            for k in 0..6 {
                gradient[k] += row[k] * r;
                for j in k..6 {
                    hessian[k][j] += row[k] * row[j];
                }
            }
        }

        // Mirror upper triangle to lower
        for i in 1..6 {
            for j in 0..i {
                hessian[i][j] = hessian[j][i];
            }
        }

        (hessian, gradient, chi2)
    }
}

/// Batch compute chi² using NEON.
pub(super) unsafe fn batch_compute_chi2_neon(
    _model: &Gaussian2D,
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 6],
) -> f64 {
    let n = data_x.len();
    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;

    unsafe {
        let v_x0 = vdupq_n_f64(x0);
        let v_y0 = vdupq_n_f64(y0);
        let v_amp = vdupq_n_f64(amp);
        let v_bg = vdupq_n_f64(bg);
        let v_inv_sx2 = vdupq_n_f64(1.0 / sigma_x2);
        let v_inv_sy2 = vdupq_n_f64(1.0 / sigma_y2);
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
            let dx2 = vmulq_f64(dx, dx);
            let dy2 = vmulq_f64(dy, dy);
            let term_x = vmulq_f64(dx2, v_inv_sx2);
            let term_y = vfmaq_f64(term_x, dy2, v_inv_sy2);
            let exponent = vmulq_f64(v_neg_half, term_y);
            let exp_val = simd_exp_fast(exponent);
            let model_val = vfmaq_f64(v_bg, v_amp, exp_val);
            let residual = vsubq_f64(vz, model_val);
            v_chi2 = vfmaq_f64(v_chi2, residual, residual);
        }

        let mut chi2 = hsum(v_chi2);

        // Scalar tail
        let model = Gaussian2D {
            stamp_radius: _model.stamp_radius,
        };
        let tail_start = chunks * 2;
        for i in tail_start..n {
            let residual = data_z[i] - model.evaluate(data_x[i], data_y[i], params);
            chi2 += residual * residual;
        }

        chi2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that simd_exp_fast produces results close to std exp().
    #[test]
    fn test_simd_exp_fast_accuracy() {
        let test_values: &[f64] = &[
            0.0,
            1.0,
            -1.0,
            0.5,
            -0.5,
            2.0,
            -2.0,
            5.0,
            -5.0,
            10.0,
            -10.0,
            -50.0,
            -100.0,
            -500.0,
            -700.0,
            0.001,
            -0.001,
            0.1,
            -0.1,
            std::f64::consts::PI,
            -std::f64::consts::PI,
            100.0,
            500.0,
            700.0,
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

    /// Test simd_exp_fast with the typical Gaussian exponent range.
    #[test]
    fn test_simd_exp_fast_gaussian_range() {
        for i in 0..1000 {
            let x = -(i as f64) * 0.5;
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
