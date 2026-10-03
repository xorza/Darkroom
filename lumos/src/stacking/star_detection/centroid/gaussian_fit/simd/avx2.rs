//! AVX2+FMA SIMD implementation for the elliptical `Gaussian2D` batch operations.
//!
//! Processes 4 f64 pixels per AVX2 iteration for `batch_build_normal_equations`
//! and `batch_compute_chi2`. Uses a fast polynomial `exp()` approximation
//! (Cephes-derived, ~1e-13 relative accuracy) fully vectorized in AVX2.

use std::f64::consts::LOG2_E;

use crate::stacking::star_detection::centroid::gaussian_fit::Gaussian2D;
use crate::stacking::star_detection::centroid::gaussian_fit::simd::exp_poly::{
    EXP_P0, EXP_P1, EXP_P2, EXP_Q0, EXP_Q1, EXP_Q2, EXP_Q3, LN2_HI, LN2_LO,
};
use crate::stacking::star_detection::centroid::lm_optimizer::{FitData, LMModel, NormalEquations};
use crate::stacking::star_detection::centroid::simd::hsum;
use std::arch::x86_64::*;

const LOG2E: f64 = LOG2_E;

/// Fast vectorized `exp()` for 4 f64 lanes using Cephes polynomial approximation.
///
/// Achieves ~1e-13 relative accuracy, which is more than sufficient for
/// Levenberg-Marquardt fitting where the solver converges to ~1e-8.
#[target_feature(enable = "avx2,fma")]
#[inline]
unsafe fn simd_exp_fast(x: __m256d) -> __m256d {
    // Clamp to avoid overflow/underflow in IEEE 754
    let v_min = _mm256_set1_pd(-708.0);
    let v_max = _mm256_set1_pd(709.0);
    let x = _mm256_max_pd(_mm256_min_pd(x, v_max), v_min);

    // Range reduction: n = round(x * log2(e))
    let v_log2e = _mm256_set1_pd(LOG2E);
    let v_half = _mm256_set1_pd(0.5);
    let n_real = _mm256_fmadd_pd(x, v_log2e, v_half);
    let n_real = _mm256_floor_pd(n_real);

    // r = x - n * ln(2), using two-part ln(2) for precision
    let v_ln2_hi = _mm256_set1_pd(LN2_HI);
    let v_ln2_lo = _mm256_set1_pd(LN2_LO);
    let r = _mm256_sub_pd(x, _mm256_mul_pd(n_real, v_ln2_hi));
    let r = _mm256_sub_pd(r, _mm256_mul_pd(n_real, v_ln2_lo));

    // Polynomial evaluation: P(r²) and Q(r²)
    // Cephes rational approximation:
    //   px = r * ((P0 * r² + P1) * r² + P2)
    //   qx = ((Q0 * r² + Q1) * r² + Q2) * r² + Q3
    //   exp(r) = 1 + 2*px / (qx - px)
    let r2 = _mm256_mul_pd(r, r);

    // P(r) = r * ((P0 * r² + P1) * r² + P2)
    let px = _mm256_fmadd_pd(_mm256_set1_pd(EXP_P0), r2, _mm256_set1_pd(EXP_P1));
    let px = _mm256_fmadd_pd(px, r2, _mm256_set1_pd(EXP_P2));
    let px = _mm256_mul_pd(px, r);

    // Q(r) = ((Q0 * r² + Q1) * r² + Q2) * r² + Q3
    let qx = _mm256_fmadd_pd(_mm256_set1_pd(EXP_Q0), r2, _mm256_set1_pd(EXP_Q1));
    let qx = _mm256_fmadd_pd(qx, r2, _mm256_set1_pd(EXP_Q2));
    let qx = _mm256_fmadd_pd(qx, r2, _mm256_set1_pd(EXP_Q3));

    // exp(r) = 1 + 2*px / (qx - px)
    let v_one = _mm256_set1_pd(1.0);
    let v_two = _mm256_set1_pd(2.0);
    let denom = _mm256_sub_pd(qx, px);
    let frac = _mm256_div_pd(px, denom);
    let exp_r = _mm256_fmadd_pd(v_two, frac, v_one);

    // Reconstruct: exp(x) = 2^n * exp(r)
    // Convert n to i64, add IEEE 754 exponent bias (1023), shift left by 52
    let n_i32 = _mm256_cvtpd_epi32(n_real); // 4 × i32 in __m128i
    let n_i64 = _mm256_cvtepi32_epi64(n_i32); // 4 × i64 in __m256i
    let bias = _mm256_set1_epi64x(1023);
    let n_biased = _mm256_add_epi64(n_i64, bias);
    let pow2n = _mm256_slli_epi64(n_biased, 52);
    let pow2n = _mm256_castsi256_pd(pow2n);

    _mm256_mul_pd(exp_r, pow2n)
}

/// Batch build normal equations (J^T J, J^T r, chi²) for the elliptical Gaussian.
///
/// For N=7, accumulates 28 upper-triangle hessian elements, 7 gradient elements, and chi² in
/// registers (36 total). The Jacobian row is the model's own (see
/// [`Gaussian2D::evaluate_and_jacobian`]), with the background's `∂f/∂bg = 1` folded into plain
/// additions.
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available on the current CPU.
#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn batch_build_normal_equations_avx2(
    model: &Gaussian2D,
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 7],
) -> NormalEquations<7> {
    let n = data_x.len();
    let [x0, y0, amp, a, b, c, bg] = *params;

    unsafe {
        let v_x0 = _mm256_set1_pd(x0);
        let v_y0 = _mm256_set1_pd(y0);
        let v_amp = _mm256_set1_pd(amp);
        let v_bg = _mm256_set1_pd(bg);
        let v_a = _mm256_set1_pd(a);
        let v_b = _mm256_set1_pd(b);
        let v_c = _mm256_set1_pd(c);
        let v_neg_half = _mm256_set1_pd(-0.5);
        let v_one = _mm256_set1_pd(1.0);
        let zero = _mm256_setzero_pd();
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

        let chunks = n / 4;

        for chunk in 0..chunks {
            let base = chunk * 4;
            let vx = _mm256_loadu_pd(data_x.as_ptr().add(base));
            let vy = _mm256_loadu_pd(data_y.as_ptr().add(base));
            let vz = _mm256_loadu_pd(data_z.as_ptr().add(base));

            let dx = _mm256_sub_pd(vx, v_x0);
            let dy = _mm256_sub_pd(vy, v_y0);
            // t = a·dx + b·dy and u = b·dx + c·dy, so the quadratic form is dx·t + dy·u.
            let t = _mm256_fmadd_pd(v_b, dy, _mm256_mul_pd(v_a, dx));
            let u = _mm256_fmadd_pd(v_c, dy, _mm256_mul_pd(v_b, dx));
            let q = _mm256_fmadd_pd(dy, u, _mm256_mul_pd(dx, t));
            let exp_val = simd_exp_fast(_mm256_mul_pd(v_neg_half, q));
            let amp_exp = _mm256_mul_pd(v_amp, exp_val);
            let residual = _mm256_sub_pd(vz, _mm256_add_pd(amp_exp, v_bg));
            v_chi2 = _mm256_fmadd_pd(residual, residual, v_chi2);

            let half_amp_exp = _mm256_mul_pd(v_neg_half, amp_exp);
            let j0 = _mm256_mul_pd(amp_exp, t);
            let j1 = _mm256_mul_pd(amp_exp, u);
            let j2 = exp_val;
            let j3 = _mm256_mul_pd(half_amp_exp, _mm256_mul_pd(dx, dx));
            let j4 = _mm256_mul_pd(_mm256_mul_pd(half_amp_exp, dx), _mm256_add_pd(dy, dy));
            let j5 = _mm256_mul_pd(half_amp_exp, _mm256_mul_pd(dy, dy));
            v_g0 = _mm256_fmadd_pd(j0, residual, v_g0);
            v_g1 = _mm256_fmadd_pd(j1, residual, v_g1);
            v_g2 = _mm256_fmadd_pd(j2, residual, v_g2);
            v_g3 = _mm256_fmadd_pd(j3, residual, v_g3);
            v_g4 = _mm256_fmadd_pd(j4, residual, v_g4);
            v_g5 = _mm256_fmadd_pd(j5, residual, v_g5);
            v_g6 = _mm256_add_pd(v_g6, residual);
            v_h00 = _mm256_fmadd_pd(j0, j0, v_h00);
            v_h01 = _mm256_fmadd_pd(j0, j1, v_h01);
            v_h02 = _mm256_fmadd_pd(j0, j2, v_h02);
            v_h03 = _mm256_fmadd_pd(j0, j3, v_h03);
            v_h04 = _mm256_fmadd_pd(j0, j4, v_h04);
            v_h05 = _mm256_fmadd_pd(j0, j5, v_h05);
            v_h06 = _mm256_add_pd(v_h06, j0);
            v_h11 = _mm256_fmadd_pd(j1, j1, v_h11);
            v_h12 = _mm256_fmadd_pd(j1, j2, v_h12);
            v_h13 = _mm256_fmadd_pd(j1, j3, v_h13);
            v_h14 = _mm256_fmadd_pd(j1, j4, v_h14);
            v_h15 = _mm256_fmadd_pd(j1, j5, v_h15);
            v_h16 = _mm256_add_pd(v_h16, j1);
            v_h22 = _mm256_fmadd_pd(j2, j2, v_h22);
            v_h23 = _mm256_fmadd_pd(j2, j3, v_h23);
            v_h24 = _mm256_fmadd_pd(j2, j4, v_h24);
            v_h25 = _mm256_fmadd_pd(j2, j5, v_h25);
            v_h26 = _mm256_add_pd(v_h26, j2);
            v_h33 = _mm256_fmadd_pd(j3, j3, v_h33);
            v_h34 = _mm256_fmadd_pd(j3, j4, v_h34);
            v_h35 = _mm256_fmadd_pd(j3, j5, v_h35);
            v_h36 = _mm256_add_pd(v_h36, j3);
            v_h44 = _mm256_fmadd_pd(j4, j4, v_h44);
            v_h45 = _mm256_fmadd_pd(j4, j5, v_h45);
            v_h46 = _mm256_add_pd(v_h46, j4);
            v_h55 = _mm256_fmadd_pd(j5, j5, v_h55);
            v_h56 = _mm256_add_pd(v_h56, j5);
            v_h66 = _mm256_add_pd(v_h66, v_one);
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

        // Scalar tail (pixels past the last full 4-wide chunk)
        let tail_start = chunks * 4;
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
/// Caller must ensure AVX2 and FMA are available on the current CPU.
#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn batch_compute_chi2_avx2(
    model: &Gaussian2D,
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 7],
) -> f64 {
    let n = data_x.len();
    let [x0, y0, amp, a, b, c, bg] = *params;

    unsafe {
        let v_x0 = _mm256_set1_pd(x0);
        let v_y0 = _mm256_set1_pd(y0);
        let v_amp = _mm256_set1_pd(amp);
        let v_bg = _mm256_set1_pd(bg);
        let v_a = _mm256_set1_pd(a);
        let v_b = _mm256_set1_pd(b);
        let v_c = _mm256_set1_pd(c);
        let v_neg_half = _mm256_set1_pd(-0.5);

        let mut v_chi2 = _mm256_setzero_pd();
        let chunks = n / 4;

        for chunk in 0..chunks {
            let base = chunk * 4;
            let vx = _mm256_loadu_pd(data_x.as_ptr().add(base));
            let vy = _mm256_loadu_pd(data_y.as_ptr().add(base));
            let vz = _mm256_loadu_pd(data_z.as_ptr().add(base));

            let dx = _mm256_sub_pd(vx, v_x0);
            let dy = _mm256_sub_pd(vy, v_y0);
            let t = _mm256_fmadd_pd(v_b, dy, _mm256_mul_pd(v_a, dx));
            let u = _mm256_fmadd_pd(v_c, dy, _mm256_mul_pd(v_b, dx));
            let q = _mm256_fmadd_pd(dy, u, _mm256_mul_pd(dx, t));
            let exp_val = simd_exp_fast(_mm256_mul_pd(v_neg_half, q));
            let model_val = _mm256_fmadd_pd(v_amp, exp_val, v_bg);
            let residual = _mm256_sub_pd(vz, model_val);
            v_chi2 = _mm256_fmadd_pd(residual, residual, v_chi2);
        }

        let mut chi2 = hsum(v_chi2);

        // Scalar tail (pixels past the last full 4-wide chunk)
        let tail_start = chunks * 4;
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

    use crate::stacking::star_detection::centroid::gaussian_fit::simd::avx2::*;
    use imaginarium::SimdTier;

    use crate::testing::simd_check;

    /// Test that `simd_exp_fast` produces results close to std `exp()`.
    #[test]
    fn simd_exp_fast_accuracy() {
        if !simd_check::runs_here(SimdTier::Avx2Fma) {
            return;
        }

        let test_values: &[f64] = &[
            0.0, 1.0, -1.0, 0.5, -0.5, 2.0, -2.0, 5.0, -5.0, 10.0, -10.0, -50.0, -100.0, -500.0,
            -700.0, 0.001, -0.001, 0.1, -0.1, PI, -PI, 100.0, 500.0, 700.0,
        ];

        for &x in test_values {
            let input = [x; 4];
            let result = unsafe {
                let v = _mm256_loadu_pd(input.as_ptr());
                let r = simd_exp_fast(v);
                let mut out = [0.0f64; 4];
                _mm256_storeu_pd(out.as_mut_ptr(), r);
                out[0]
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
        if !simd_check::runs_here(SimdTier::Avx2Fma) {
            return;
        }

        // Gaussian fitting exponents are always ≤ 0: -0.5 * r²/σ²
        // For stamp_radius=15, sigma=1.0: max |exponent| = 0.5 * 15² = 112.5
        for i in 0..1000 {
            let x = -f64::from(i) * 0.5; // Range: 0 to -500
            let input = [x; 4];
            let result = unsafe {
                let v = _mm256_loadu_pd(input.as_ptr());
                let r = simd_exp_fast(v);
                let mut out = [0.0f64; 4];
                _mm256_storeu_pd(out.as_mut_ptr(), r);
                out[0]
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
