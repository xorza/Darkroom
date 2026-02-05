//! AVX2+FMA SIMD implementation for Gaussian2D batch operations.
//!
//! Processes 4 f64 pixels per AVX2 iteration for `batch_build_normal_equations`
//! and `batch_compute_chi2`. Uses a fast polynomial `exp()` approximation
//! (Cephes-derived, ~1e-13 relative accuracy) fully vectorized in AVX2.

use super::super::lm_optimizer::LMModel;
use super::Gaussian2D;
use std::arch::x86_64::*;

// ============================================================================
// Fast SIMD exp() for f64 — Cephes-derived polynomial approximation
// ============================================================================
//
// Algorithm:
//   1. Clamp x to [-708, 709] to avoid overflow/underflow
//   2. Range reduction: n = round(x / ln2), r = x - n*ln2  (|r| ≤ ln2/2)
//   3. Polynomial: exp(r) ≈ 1 + 2r * P(r²) / (Q(r²) - P(r²))
//      where P and Q are degree-3 and degree-4 polynomials (Cephes coefficients)
//   4. Reconstruction: exp(x) = 2^n * exp(r), done via IEEE 754 bit manipulation
//
// Accuracy: max relative error < 2e-13, sufficient for L-M fitting.
// Coefficients from Cephes library (Stephen Moshier), public domain.

// Cephes P coefficients for exp (numerator)
const EXP_P0: f64 = 1.261_771_930_748_105_8e-4;
const EXP_P1: f64 = 3.029_944_077_074_419_5e-2;
const EXP_P2: f64 = 1.0;

// Cephes Q coefficients for exp (denominator)
const EXP_Q0: f64 = 3.001_985_051_386_644_6e-6;
const EXP_Q1: f64 = 2.524_483_403_496_841e-3;
const EXP_Q2: f64 = 2.272_655_482_081_550_3e-1;
const EXP_Q3: f64 = 2.0;

const LOG2E: f64 = std::f64::consts::LOG2_E;

// ln(2) split into high and low parts for exact range reduction
const LN2_HI: f64 = 6.931_457_519_531_25e-1;
const LN2_LO: f64 = 1.428_606_820_309_417_3e-6;

/// Fast vectorized exp() for 4 f64 lanes using Cephes polynomial approximation.
///
/// Achieves ~1e-13 relative accuracy, which is more than sufficient for
/// Levenberg-Marquardt fitting where the solver converges to ~1e-8.
#[target_feature(enable = "avx2,fma")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
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

/// Horizontal sum of 4 f64 lanes.
#[target_feature(enable = "avx2,fma")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn hsum(v: __m256d) -> f64 {
    let mut arr = [0.0f64; 4];
    _mm256_storeu_pd(arr.as_mut_ptr(), v);
    arr[0] + arr[1] + arr[2] + arr[3]
}

/// Batch build normal equations (J^T J, J^T r, chi²) using AVX2+FMA.
///
/// For N=6 (Gaussian2D), accumulates 21 upper-triangle hessian elements,
/// 6 gradient elements, and chi² directly in AVX2 registers (28 total).
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available on the current CPU.
#[allow(clippy::needless_range_loop)]
#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn batch_build_normal_equations_avx2(
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
        let v_x0 = _mm256_set1_pd(x0);
        let v_y0 = _mm256_set1_pd(y0);
        let v_amp = _mm256_set1_pd(amp);
        let v_bg = _mm256_set1_pd(bg);
        let v_inv_sx2 = _mm256_set1_pd(inv_sigma_x2);
        let v_inv_sy2 = _mm256_set1_pd(inv_sigma_y2);
        let v_neg_half = _mm256_set1_pd(-0.5);
        let v_inv_sx3 = _mm256_set1_pd(1.0 / (sigma_x2 * sigma_x));
        let v_inv_sy3 = _mm256_set1_pd(1.0 / (sigma_y2 * sigma_y));
        let v_one = _mm256_set1_pd(1.0);
        let zero = _mm256_setzero_pd();

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

        let chunks = n / 4;

        for chunk in 0..chunks {
            let base = chunk * 4;
            let vx = _mm256_loadu_pd(data_x.as_ptr().add(base));
            let vy = _mm256_loadu_pd(data_y.as_ptr().add(base));
            let vz = _mm256_loadu_pd(data_z.as_ptr().add(base));

            // dx, dy
            let dx = _mm256_sub_pd(vx, v_x0);
            let dy = _mm256_sub_pd(vy, v_y0);

            // dx², dy²
            let dx2 = _mm256_mul_pd(dx, dx);
            let dy2 = _mm256_mul_pd(dy, dy);

            // exponent = -0.5 * (dx²/σx² + dy²/σy²)
            let term_x = _mm256_mul_pd(dx2, v_inv_sx2);
            let term_y = _mm256_fmadd_pd(dy2, v_inv_sy2, term_x);
            let exponent = _mm256_mul_pd(v_neg_half, term_y);

            // exp_val = fast_exp(exponent)
            let exp_val = simd_exp_fast(exponent);

            // amp_exp = amp * exp_val
            let amp_exp = _mm256_mul_pd(v_amp, exp_val);

            // model_val = amp_exp + bg
            let model_val = _mm256_add_pd(amp_exp, v_bg);
            let residual = _mm256_sub_pd(vz, model_val);

            // chi²
            v_chi2 = _mm256_fmadd_pd(residual, residual, v_chi2);

            // Jacobian rows:
            let j0 = _mm256_mul_pd(amp_exp, _mm256_mul_pd(dx, v_inv_sx2));
            let j1 = _mm256_mul_pd(amp_exp, _mm256_mul_pd(dy, v_inv_sy2));
            let j2 = exp_val;
            let j3 = _mm256_mul_pd(amp_exp, _mm256_mul_pd(dx2, v_inv_sx3));
            let j4 = _mm256_mul_pd(amp_exp, _mm256_mul_pd(dy2, v_inv_sy3));
            // j5 = 1.0 (implicit)

            // Gradient: g[i] += j[i] * residual
            v_g0 = _mm256_fmadd_pd(j0, residual, v_g0);
            v_g1 = _mm256_fmadd_pd(j1, residual, v_g1);
            v_g2 = _mm256_fmadd_pd(j2, residual, v_g2);
            v_g3 = _mm256_fmadd_pd(j3, residual, v_g3);
            v_g4 = _mm256_fmadd_pd(j4, residual, v_g4);
            v_g5 = _mm256_add_pd(v_g5, residual); // j5=1

            // Hessian upper triangle: h[i][j] += j[i] * j[j]
            // Row 0
            v_h00 = _mm256_fmadd_pd(j0, j0, v_h00);
            v_h01 = _mm256_fmadd_pd(j0, j1, v_h01);
            v_h02 = _mm256_fmadd_pd(j0, j2, v_h02);
            v_h03 = _mm256_fmadd_pd(j0, j3, v_h03);
            v_h04 = _mm256_fmadd_pd(j0, j4, v_h04);
            v_h05 = _mm256_add_pd(v_h05, j0); // j5=1

            // Row 1
            v_h11 = _mm256_fmadd_pd(j1, j1, v_h11);
            v_h12 = _mm256_fmadd_pd(j1, j2, v_h12);
            v_h13 = _mm256_fmadd_pd(j1, j3, v_h13);
            v_h14 = _mm256_fmadd_pd(j1, j4, v_h14);
            v_h15 = _mm256_add_pd(v_h15, j1); // j5=1

            // Row 2
            v_h22 = _mm256_fmadd_pd(j2, j2, v_h22);
            v_h23 = _mm256_fmadd_pd(j2, j3, v_h23);
            v_h24 = _mm256_fmadd_pd(j2, j4, v_h24);
            v_h25 = _mm256_add_pd(v_h25, j2); // j5=1

            // Row 3
            v_h33 = _mm256_fmadd_pd(j3, j3, v_h33);
            v_h34 = _mm256_fmadd_pd(j3, j4, v_h34);
            v_h35 = _mm256_add_pd(v_h35, j3); // j5=1

            // Row 4
            v_h44 = _mm256_fmadd_pd(j4, j4, v_h44);
            v_h45 = _mm256_add_pd(v_h45, j4); // j5=1

            // Row 5: h55 += j5*j5 = 1
            v_h55 = _mm256_add_pd(v_h55, v_one);
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

        // Scalar tail
        let model = Gaussian2D {
            stamp_radius: _model.stamp_radius,
        };
        let tail_start = chunks * 4;
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

/// Batch compute chi² using AVX2+FMA.
///
/// # Safety
/// Caller must ensure AVX2 and FMA are available on the current CPU.
#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn batch_compute_chi2_avx2(
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
        let v_x0 = _mm256_set1_pd(x0);
        let v_y0 = _mm256_set1_pd(y0);
        let v_amp = _mm256_set1_pd(amp);
        let v_bg = _mm256_set1_pd(bg);
        let v_inv_sx2 = _mm256_set1_pd(1.0 / sigma_x2);
        let v_inv_sy2 = _mm256_set1_pd(1.0 / sigma_y2);
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
            let dx2 = _mm256_mul_pd(dx, dx);
            let dy2 = _mm256_mul_pd(dy, dy);
            let term_x = _mm256_mul_pd(dx2, v_inv_sx2);
            let term_y = _mm256_fmadd_pd(dy2, v_inv_sy2, term_x);
            let exponent = _mm256_mul_pd(v_neg_half, term_y);
            let exp_val = simd_exp_fast(exponent);
            let model_val = _mm256_fmadd_pd(v_amp, exp_val, v_bg);
            let residual = _mm256_sub_pd(vz, model_val);
            v_chi2 = _mm256_fmadd_pd(residual, residual, v_chi2);
        }

        let mut chi2 = hsum(v_chi2);

        // Scalar tail
        let model = Gaussian2D {
            stamp_radius: _model.stamp_radius,
        };
        let tail_start = chunks * 4;
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
        if !common::cpu_features::has_avx2_fma() {
            return;
        }

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

    /// Test simd_exp_fast with the typical Gaussian exponent range.
    #[test]
    fn test_simd_exp_fast_gaussian_range() {
        if !common::cpu_features::has_avx2_fma() {
            return;
        }

        // Gaussian fitting exponents are always ≤ 0: -0.5 * r²/σ²
        // For stamp_radius=15, sigma=1.0: max |exponent| = 0.5 * 15² = 112.5
        for i in 0..1000 {
            let x = -(i as f64) * 0.5; // Range: 0 to -500
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
