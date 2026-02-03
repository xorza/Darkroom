//! 2D Gaussian fitting for high-precision centroid computation.
//!
//! Implements Levenberg-Marquardt optimization to fit a 2D Gaussian model:
//! f(x,y) = A × exp(-((x-x₀)²/2σ_x² + (y-y₀)²/2σ_y²)) + B
//!
//! This achieves ~0.01 pixel centroid accuracy compared to ~0.05 for weighted centroid.

#![allow(dead_code)]

#[cfg(test)]
mod bench;
pub(crate) mod simd;
#[cfg(test)]
mod tests;

use super::linear_solver::solve_6x6;
use super::lm_optimizer::{LMConfig, LMModel, LMResult, optimize_6, optimize_6_weighted};
use super::{compute_pixel_weights, estimate_sigma_from_moments, extract_stamp};
use crate::common::Buffer2;
use glam::{DVec2, Vec2};

/// Configuration for Gaussian fitting.
pub type GaussianFitConfig = LMConfig;

/// Result of 2D Gaussian fitting.
#[derive(Debug, Clone, Copy)]
pub struct GaussianFitResult {
    /// Position of Gaussian center (sub-pixel).
    pub pos: Vec2,
    /// Amplitude of Gaussian.
    pub amplitude: f32,
    /// Sigma in X and Y directions.
    pub sigma: Vec2,
    /// Background level.
    pub background: f32,
    /// RMS residual of fit.
    pub rms_residual: f32,
    /// Whether the fit converged.
    pub converged: bool,
    /// Number of iterations used.
    pub iterations: usize,
}

/// 2D Gaussian model for L-M fitting.
/// Parameters: [x0, y0, amplitude, sigma_x, sigma_y, background]
pub(crate) struct Gaussian2D {
    pub stamp_radius: f32,
}

impl LMModel<6> for Gaussian2D {
    #[inline]
    fn evaluate(&self, x: f32, y: f32, params: &[f32; 6]) -> f32 {
        let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
        let dx = x - x0;
        let dy = y - y0;
        let exponent = -0.5 * (dx * dx / (sigma_x * sigma_x) + dy * dy / (sigma_y * sigma_y));
        amp * exponent.exp() + bg
    }

    #[inline]
    fn jacobian_row(&self, x: f32, y: f32, params: &[f32; 6]) -> [f32; 6] {
        let [x0, y0, amp, sigma_x, sigma_y, _bg] = *params;
        let sigma_x2 = sigma_x * sigma_x;
        let sigma_y2 = sigma_y * sigma_y;

        let dx = x - x0;
        let dy = y - y0;
        let dx2 = dx * dx;
        let dy2 = dy * dy;
        let exponent = -0.5 * (dx2 / sigma_x2 + dy2 / sigma_y2);
        let exp_val = exponent.exp();
        let amp_exp = amp * exp_val;

        [
            amp_exp * dx / sigma_x2,              // df/dx0
            amp_exp * dy / sigma_y2,              // df/dy0
            exp_val,                              // df/damp
            amp_exp * dx2 / (sigma_x2 * sigma_x), // df/dsigma_x
            amp_exp * dy2 / (sigma_y2 * sigma_y), // df/dsigma_y
            1.0,                                  // df/dbg
        ]
    }

    #[inline]
    fn constrain(&self, params: &mut [f32; 6]) {
        params[2] = params[2].max(0.01); // Amplitude > 0
        params[3] = params[3].clamp(0.5, self.stamp_radius); // Sigma_x
        params[4] = params[4].clamp(0.5, self.stamp_radius); // Sigma_y
    }
}

/// Fit a 2D Gaussian to a star stamp.
///
/// Uses Levenberg-Marquardt optimization to find the best-fit Gaussian
/// parameters, achieving ~0.01 pixel centroid accuracy.
pub fn fit_gaussian_2d(
    pixels: &Buffer2<f32>,
    pos: Vec2,
    stamp_radius: usize,
    background: f32,
    config: &GaussianFitConfig,
) -> Option<GaussianFitResult> {
    let (data_x, data_y, data_z, peak_value) = extract_stamp(pixels, pos, stamp_radius)?;

    let n = data_x.len();
    if n < 7 {
        return None;
    }

    // Estimate sigma from moments for better initial guess
    let sigma_est = estimate_sigma_from_moments(&data_x, &data_y, &data_z, pos, background);

    let initial_params = [
        pos.x,
        pos.y,
        (peak_value - background).max(0.01),
        sigma_est,
        sigma_est,
        background,
    ];

    // Use SIMD-optimized optimizer
    let result = optimize_gaussian_simd(
        &data_x,
        &data_y,
        &data_z,
        initial_params,
        stamp_radius as f32,
        config,
    );

    validate_result(&result, pos, stamp_radius, n)
}

/// Fit a 2D Gaussian to a star stamp with inverse-variance weighting.
///
/// Uses weighted Levenberg-Marquardt optimization for optimal estimation
/// when noise characteristics are known.
#[allow(clippy::too_many_arguments)]
pub fn fit_gaussian_2d_weighted(
    pixels: &Buffer2<f32>,
    pos: Vec2,
    stamp_radius: usize,
    background: f32,
    noise: f32,
    gain: Option<f32>,
    read_noise: Option<f32>,
    config: &GaussianFitConfig,
) -> Option<GaussianFitResult> {
    let (data_x, data_y, data_z, peak_value) = extract_stamp(pixels, pos, stamp_radius)?;

    let n = data_x.len();
    if n < 7 {
        return None;
    }

    // Compute inverse-variance weights
    let weights = compute_pixel_weights(&data_z, background, noise, gain, read_noise);

    // Estimate sigma from moments for better initial guess
    let sigma_est = estimate_sigma_from_moments(&data_x, &data_y, &data_z, pos, background);

    let initial_params = [
        pos.x,
        pos.y,
        (peak_value - background).max(0.01),
        sigma_est,
        sigma_est,
        background,
    ];

    let model = Gaussian2D {
        stamp_radius: stamp_radius as f32,
    };
    let result = optimize_6_weighted(
        &model,
        &data_x,
        &data_y,
        &data_z,
        &weights,
        initial_params,
        config,
    );

    validate_result(&result, pos, stamp_radius, n)
}

fn validate_result(
    result: &LMResult<6>,
    pos: Vec2,
    stamp_radius: usize,
    n: usize,
) -> Option<GaussianFitResult> {
    let [x0, y0, amplitude, sigma_x, sigma_y, bg] = result.params;

    // Check if center is within stamp
    let result_pos = Vec2::new(x0, y0);
    if (result_pos - pos).abs().max_element() > stamp_radius as f32 {
        return None;
    }

    // Check for reasonable sigma values
    if sigma_x < 0.5
        || sigma_y < 0.5
        || sigma_x > stamp_radius as f32 * 2.0
        || sigma_y > stamp_radius as f32 * 2.0
    {
        return None;
    }

    let rms = (result.chi2 / n as f32).sqrt();

    Some(GaussianFitResult {
        pos: Vec2::new(x0, y0),
        amplitude,
        sigma: Vec2::new(sigma_x, sigma_y),
        background: bg,
        rms_residual: rms,
        converged: result.converged,
        iterations: result.iterations,
    })
}

/// SIMD-optimized L-M optimizer for Gaussian fitting.
pub(crate) fn optimize_gaussian_simd(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    initial_params: [f32; 6],
    stamp_radius: f32,
    config: &LMConfig,
) -> LMResult<6> {
    #[cfg(target_arch = "x86_64")]
    if common::cpu_features::has_avx2_fma() {
        return unsafe {
            optimize_gaussian_avx2(data_x, data_y, data_z, initial_params, stamp_radius, config)
        };
    }

    // Fallback to generic optimizer
    let model = Gaussian2D { stamp_radius };
    optimize_6(&model, data_x, data_y, data_z, initial_params, config)
}

/// AVX2-accelerated L-M optimization for Gaussian fitting.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2", enable = "fma")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn optimize_gaussian_avx2(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    initial_params: [f32; 6],
    stamp_radius: f32,
    config: &LMConfig,
) -> LMResult<6> {
    let mut params = initial_params;
    let mut lambda = config.initial_lambda;
    let mut prev_chi2 = simd::compute_chi2_simd(data_x, data_y, data_z, &params);
    let mut converged = false;
    let mut iterations = 0;

    let n = data_x.len();
    let mut jacobian = Vec::with_capacity(n);
    let mut residuals = Vec::with_capacity(n);

    for iter in 0..config.max_iterations {
        iterations = iter + 1;

        simd::fill_jacobian_residuals_simd(
            data_x,
            data_y,
            data_z,
            &params,
            &mut jacobian,
            &mut residuals,
        );

        let (hessian, gradient) = compute_hessian_gradient_6(&jacobian, &residuals);

        let mut damped_hessian = hessian;
        for (i, row) in damped_hessian.iter_mut().enumerate() {
            row[i] *= 1.0 + lambda;
        }

        let Some(delta) = solve_6x6(&damped_hessian, &gradient) else {
            break;
        };

        let mut new_params = params;
        for (p, d) in new_params.iter_mut().zip(delta.iter()) {
            *p += d;
        }
        constrain_gaussian_params(&mut new_params, stamp_radius);

        let new_chi2 = simd::compute_chi2_simd(data_x, data_y, data_z, &new_params);

        if new_chi2 < prev_chi2 {
            params = new_params;
            lambda *= config.lambda_down;

            let chi2_rel_change = (prev_chi2 - new_chi2) / prev_chi2.max(1e-30);
            prev_chi2 = new_chi2;

            let max_delta = delta.iter().copied().fold(0.0f32, |a, d| a.max(d.abs()));
            if max_delta < config.convergence_threshold || chi2_rel_change < 1e-6 {
                converged = true;
                break;
            }
        } else {
            let chi2_rel_diff = (new_chi2 - prev_chi2) / prev_chi2.max(1e-30);
            if chi2_rel_diff < 1e-6 {
                converged = true;
                break;
            }

            lambda *= config.lambda_up;
            if lambda > 1e10 {
                break;
            }
        }
    }

    LMResult {
        params,
        chi2: prev_chi2,
        converged,
        iterations,
    }
}

#[inline]
fn constrain_gaussian_params(params: &mut [f32; 6], stamp_radius: f32) {
    params[2] = params[2].max(0.01); // Amplitude > 0
    params[3] = params[3].clamp(0.5, stamp_radius); // Sigma_x
    params[4] = params[4].clamp(0.5, stamp_radius); // Sigma_y
}

/// Compute Hessian (J^T J) and gradient (J^T r) for 6-parameter Gaussian model.
///
/// AVX2 implementation: processes 8 Jacobian rows at a time using gathered loads
/// and FMA. Exploits symmetry — only computes 21 upper-triangle hessian elements.
///
/// # Safety
/// Requires AVX2 + FMA. Called from `optimize_gaussian_avx2` which has the
/// appropriate `#[target_feature]` attribute.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2", enable = "fma")]
#[allow(unsafe_op_in_unsafe_fn, clippy::needless_range_loop)]
unsafe fn compute_hessian_gradient_6(
    jacobian: &[[f32; 6]],
    residuals: &[f32],
) -> ([[f32; 6]; 6], [f32; 6]) {
    use std::arch::x86_64::*;

    let n = jacobian.len();
    let simd_end = (n / 8) * 8;

    // 21 upper-triangle hessian accumulators + 6 gradient accumulators
    let mut h00 = _mm256_setzero_ps();
    let mut h01 = _mm256_setzero_ps();
    let mut h02 = _mm256_setzero_ps();
    let mut h03 = _mm256_setzero_ps();
    let mut h04 = _mm256_setzero_ps();
    let mut h05 = _mm256_setzero_ps();
    let mut h11 = _mm256_setzero_ps();
    let mut h12 = _mm256_setzero_ps();
    let mut h13 = _mm256_setzero_ps();
    let mut h14 = _mm256_setzero_ps();
    let mut h15 = _mm256_setzero_ps();
    let mut h22 = _mm256_setzero_ps();
    let mut h23 = _mm256_setzero_ps();
    let mut h24 = _mm256_setzero_ps();
    let mut h25 = _mm256_setzero_ps();
    let mut h33 = _mm256_setzero_ps();
    let mut h34 = _mm256_setzero_ps();
    let mut h35 = _mm256_setzero_ps();
    let mut h44 = _mm256_setzero_ps();
    let mut h45 = _mm256_setzero_ps();
    let mut h55 = _mm256_setzero_ps();

    let mut g0 = _mm256_setzero_ps();
    let mut g1 = _mm256_setzero_ps();
    let mut g2 = _mm256_setzero_ps();
    let mut g3 = _mm256_setzero_ps();
    let mut g4 = _mm256_setzero_ps();
    let mut g5 = _mm256_setzero_ps();

    // Process 8 rows at a time
    let jac_ptr = jacobian.as_ptr() as *const f32;
    let res_ptr = residuals.as_ptr();

    for base in (0..simd_end).step_by(8) {
        let p = jac_ptr.add(base * 6);

        // Load 8 rows of 6 columns each. Each row is contiguous [c0,c1,c2,c3,c4,c5].
        // We need column vectors: all c0 values, all c1 values, etc.
        // Manual set is faster than gather on most CPUs.
        let c0 = _mm256_setr_ps(
            *p.add(0),
            *p.add(6),
            *p.add(12),
            *p.add(18),
            *p.add(24),
            *p.add(30),
            *p.add(36),
            *p.add(42),
        );
        let c1 = _mm256_setr_ps(
            *p.add(1),
            *p.add(7),
            *p.add(13),
            *p.add(19),
            *p.add(25),
            *p.add(31),
            *p.add(37),
            *p.add(43),
        );
        let c2 = _mm256_setr_ps(
            *p.add(2),
            *p.add(8),
            *p.add(14),
            *p.add(20),
            *p.add(26),
            *p.add(32),
            *p.add(38),
            *p.add(44),
        );
        let c3 = _mm256_setr_ps(
            *p.add(3),
            *p.add(9),
            *p.add(15),
            *p.add(21),
            *p.add(27),
            *p.add(33),
            *p.add(39),
            *p.add(45),
        );
        let c4 = _mm256_setr_ps(
            *p.add(4),
            *p.add(10),
            *p.add(16),
            *p.add(22),
            *p.add(28),
            *p.add(34),
            *p.add(40),
            *p.add(46),
        );
        let c5 = _mm256_setr_ps(
            *p.add(5),
            *p.add(11),
            *p.add(17),
            *p.add(23),
            *p.add(29),
            *p.add(35),
            *p.add(41),
            *p.add(47),
        );

        let r = _mm256_loadu_ps(res_ptr.add(base));

        // Gradient: g[i] += c[i] * r
        g0 = _mm256_fmadd_ps(c0, r, g0);
        g1 = _mm256_fmadd_ps(c1, r, g1);
        g2 = _mm256_fmadd_ps(c2, r, g2);
        g3 = _mm256_fmadd_ps(c3, r, g3);
        g4 = _mm256_fmadd_ps(c4, r, g4);
        g5 = _mm256_fmadd_ps(c5, r, g5);

        // Upper triangle of hessian: h[i][j] += c[i] * c[j]
        h00 = _mm256_fmadd_ps(c0, c0, h00);
        h01 = _mm256_fmadd_ps(c0, c1, h01);
        h02 = _mm256_fmadd_ps(c0, c2, h02);
        h03 = _mm256_fmadd_ps(c0, c3, h03);
        h04 = _mm256_fmadd_ps(c0, c4, h04);
        h05 = _mm256_fmadd_ps(c0, c5, h05);
        h11 = _mm256_fmadd_ps(c1, c1, h11);
        h12 = _mm256_fmadd_ps(c1, c2, h12);
        h13 = _mm256_fmadd_ps(c1, c3, h13);
        h14 = _mm256_fmadd_ps(c1, c4, h14);
        h15 = _mm256_fmadd_ps(c1, c5, h15);
        h22 = _mm256_fmadd_ps(c2, c2, h22);
        h23 = _mm256_fmadd_ps(c2, c3, h23);
        h24 = _mm256_fmadd_ps(c2, c4, h24);
        h25 = _mm256_fmadd_ps(c2, c5, h25);
        h33 = _mm256_fmadd_ps(c3, c3, h33);
        h34 = _mm256_fmadd_ps(c3, c4, h34);
        h35 = _mm256_fmadd_ps(c3, c5, h35);
        h44 = _mm256_fmadd_ps(c4, c4, h44);
        h45 = _mm256_fmadd_ps(c4, c5, h45);
        h55 = _mm256_fmadd_ps(c5, c5, h55);
    }

    // Horizontal sum helper
    #[inline]
    #[target_feature(enable = "avx2")]
    unsafe fn hsum(v: __m256) -> f32 {
        // Sum 8 floats: high128 + low128, then hadd twice
        let hi = _mm256_extractf128_ps(v, 1);
        let lo = _mm256_castps256_ps128(v);
        let sum128 = _mm_add_ps(lo, hi);
        let shuf = _mm_movehdup_ps(sum128); // [1,1,3,3]
        let sums = _mm_add_ps(sum128, shuf); // [0+1,_,2+3,_]
        let shuf2 = _mm_movehl_ps(sums, sums); // [2+3,_,_,_]
        let result = _mm_add_ss(sums, shuf2);
        _mm_cvtss_f32(result)
    }

    // Reduce SIMD accumulators to scalars
    let mut hessian = [[0.0f32; 6]; 6];
    hessian[0][0] = hsum(h00);
    hessian[0][1] = hsum(h01);
    hessian[0][2] = hsum(h02);
    hessian[0][3] = hsum(h03);
    hessian[0][4] = hsum(h04);
    hessian[0][5] = hsum(h05);
    hessian[1][1] = hsum(h11);
    hessian[1][2] = hsum(h12);
    hessian[1][3] = hsum(h13);
    hessian[1][4] = hsum(h14);
    hessian[1][5] = hsum(h15);
    hessian[2][2] = hsum(h22);
    hessian[2][3] = hsum(h23);
    hessian[2][4] = hsum(h24);
    hessian[2][5] = hsum(h25);
    hessian[3][3] = hsum(h33);
    hessian[3][4] = hsum(h34);
    hessian[3][5] = hsum(h35);
    hessian[4][4] = hsum(h44);
    hessian[4][5] = hsum(h45);
    hessian[5][5] = hsum(h55);

    let mut gradient = [0.0f32; 6];
    gradient[0] = hsum(g0);
    gradient[1] = hsum(g1);
    gradient[2] = hsum(g2);
    gradient[3] = hsum(g3);
    gradient[4] = hsum(g4);
    gradient[5] = hsum(g5);

    // Scalar tail for remaining rows
    for i in simd_end..n {
        let row = &jacobian[i];
        let r = residuals[i];
        for col in 0..6 {
            gradient[col] += row[col] * r;
            for j in col..6 {
                hessian[col][j] += row[col] * row[j];
            }
        }
    }

    // Mirror upper triangle to lower
    for i in 1..6 {
        for j in 0..i {
            hessian[i][j] = hessian[j][i];
        }
    }

    (hessian, gradient)
}

/// Scalar fallback for non-x86_64 platforms.
#[cfg(not(target_arch = "x86_64"))]
fn compute_hessian_gradient_6(
    jacobian: &[[f32; 6]],
    residuals: &[f32],
) -> ([[f32; 6]; 6], [f32; 6]) {
    let mut hessian = [[0.0f32; 6]; 6];
    let mut gradient = [0.0f32; 6];

    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..6 {
            gradient[i] += row[i] * r;
            for j in i..6 {
                hessian[i][j] += row[i] * row[j];
            }
        }
    }

    // Mirror upper triangle to lower
    for i in 1..6 {
        for j in 0..i {
            hessian[i][j] = hessian[j][i];
        }
    }

    (hessian, gradient)
}
