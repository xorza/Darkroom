//! 2D Gaussian fitting for high-precision centroid computation.
//!
//! Implements Levenberg-Marquardt optimization to fit a 2D Gaussian model:
//! f(x,y) = A × exp(-((x-x₀)²/2σ_x² + (y-y₀)²/2σ_y²)) + B
//!
//! Uses f64 throughout the fitting pipeline for numerical stability,
//! achieving ~0.01 pixel centroid accuracy.

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

#[cfg(target_arch = "x86_64")]
mod simd_avx2;

#[cfg(target_arch = "aarch64")]
mod simd_neon;

// Cephes exp() polynomial coefficients (shared by AVX2 and NEON SIMD).
//
// Algorithm: range reduction via x = n*ln2 + r, then rational approximation
//   exp(r) ≈ 1 + 2r * P(r²) / (Q(r²) - P(r²))
// Coefficients from Cephes library (Stephen Moshier), public domain.
// Max relative error < 2e-13.

pub(crate) const EXP_P0: f64 = 1.261_771_930_748_105_8e-4;
pub(crate) const EXP_P1: f64 = 3.029_944_077_074_419_5e-2;
pub(crate) const EXP_P2: f64 = 1.0;

pub(crate) const EXP_Q0: f64 = 3.001_985_051_386_644_6e-6;
pub(crate) const EXP_Q1: f64 = 2.524_483_403_496_841e-3;
pub(crate) const EXP_Q2: f64 = 2.272_655_482_081_550_3e-1;
pub(crate) const EXP_Q3: f64 = 2.0;

// ln(2) split into high and low parts for exact range reduction
pub(crate) const LN2_HI: f64 = 6.931_457_519_531_25e-1;
pub(crate) const LN2_LO: f64 = 1.428_606_820_309_417_3e-6;

use crate::stacking::star_detection::centroid::lm_optimizer::{
    LMConfig, LMModel, LMResult, accumulate_chi2, build_normal_equations_scalar, optimize,
};
use crate::stacking::star_detection::centroid::{
    FitNoise, MAX_STAMP_PIXELS, estimate_sigma_from_moments, extract_stamp, fit_weights,
};
use arrayvec::ArrayVec;
use glam::Vec2;
use imaginarium::Buffer2;

/// Configuration for Gaussian fitting.
pub(crate) type GaussianFitConfig = LMConfig;

/// Result of 2D Gaussian fitting.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GaussianFitResult {
    /// Position of Gaussian center (sub-pixel).
    pub pos: Vec2,
    /// Sigma in X and Y directions.
    pub sigma: Vec2,
    /// Whether the fit converged.
    pub converged: bool,
    /// Fit diagnostics (amplitude/background/RMS residual/iteration count) that no
    /// production caller reads — `measure_star` only uses `pos`/`sigma`/`converged` —
    /// but that tests need to verify LM convergence against synthetic ground truth.
    #[allow(dead_code)] // read only by tests
    pub debug: GaussianFitDebug,
}

/// Fit diagnostics kept for tests; see [`GaussianFitResult::debug`].
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // read only by tests
pub(crate) struct GaussianFitDebug {
    /// Amplitude of Gaussian.
    pub amplitude: f32,
    /// Background level.
    pub background: f32,
    /// RMS residual of fit.
    pub rms_residual: f32,
    /// Number of iterations used.
    pub iterations: usize,
}

/// 2D Gaussian model for L-M optimization (6 parameters).
/// Parameters: [x0, y0, amplitude, sigma_x, sigma_y, background]
#[derive(Debug)]
pub(crate) struct Gaussian2D {
    pub stamp_radius: f64,
}

impl LMModel<6> for Gaussian2D {
    #[inline]
    fn evaluate(&self, x: f64, y: f64, params: &[f64; 6]) -> f64 {
        let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
        let dx = x - x0;
        let dy = y - y0;
        let exponent = -0.5 * (dx * dx / (sigma_x * sigma_x) + dy * dy / (sigma_y * sigma_y));
        amp * exponent.exp() + bg
    }

    #[inline]
    fn jacobian_row(&self, x: f64, y: f64, params: &[f64; 6]) -> [f64; 6] {
        let [x0, y0, amp, sigma_x, sigma_y, _bg] = *params;
        let sigma_x2 = sigma_x * sigma_x;
        let sigma_y2 = sigma_y * sigma_y;
        let dx = x - x0;
        let dy = y - y0;
        let exponent = -0.5 * (dx * dx / sigma_x2 + dy * dy / sigma_y2);
        let exp_val = exponent.exp();
        let amp_exp = amp * exp_val;

        [
            amp_exp * dx / sigma_x2,                  // df/dx0
            amp_exp * dy / sigma_y2,                  // df/dy0
            exp_val,                                  // df/damp
            amp_exp * dx * dx / (sigma_x2 * sigma_x), // df/dsigma_x
            amp_exp * dy * dy / (sigma_y2 * sigma_y), // df/dsigma_y
            1.0,                                      // df/dbg
        ]
    }

    #[inline]
    fn evaluate_and_jacobian(&self, x: f64, y: f64, params: &[f64; 6]) -> (f64, [f64; 6]) {
        let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
        let sigma_x2 = sigma_x * sigma_x;
        let sigma_y2 = sigma_y * sigma_y;
        let dx = x - x0;
        let dy = y - y0;
        let exponent = -0.5 * (dx * dx / sigma_x2 + dy * dy / sigma_y2);
        let exp_val = exponent.exp();
        let amp_exp = amp * exp_val;
        let model_val = amp_exp + bg;

        (
            model_val,
            [
                amp_exp * dx / sigma_x2,                  // df/dx0
                amp_exp * dy / sigma_y2,                  // df/dy0
                exp_val,                                  // df/damp
                amp_exp * dx * dx / (sigma_x2 * sigma_x), // df/dsigma_x
                amp_exp * dy * dy / (sigma_y2 * sigma_y), // df/dsigma_y
                1.0,                                      // df/dbg
            ],
        )
    }

    #[inline]
    fn constrain(&self, params: &mut [f64; 6]) {
        params[2] = params[2].max(0.01); // Amplitude > 0
        params[3] = params[3].clamp(0.5, self.stamp_radius); // Sigma_x
        params[4] = params[4].clamp(0.5, self.stamp_radius); // Sigma_y
    }

    // Scalar fallback is dead code on aarch64, where the NEON path returns unconditionally.
    #[allow(unreachable_code)]
    fn batch_build_normal_equations(
        &self,
        data_x: &[f64],
        data_y: &[f64],
        data_z: &[f64],
        params: &[f64; 6],
    ) -> ([[f64; 6]; 6], [f64; 6], f64) {
        #[cfg(target_arch = "x86_64")]
        if common::cpu_features::has_avx2_fma() {
            return unsafe {
                simd_avx2::batch_build_normal_equations_avx2(self, data_x, data_y, data_z, params)
            };
        }
        #[cfg(target_arch = "aarch64")]
        {
            return unsafe {
                simd_neon::batch_build_normal_equations_neon(self, data_x, data_y, data_z, params)
            };
        }
        // Scalar fallback
        build_normal_equations_scalar(self, data_x, data_y, data_z, None, params)
    }

    // Scalar fallback is dead code on aarch64, where the NEON path returns unconditionally.
    #[allow(unreachable_code)]
    fn batch_compute_chi2(
        &self,
        data_x: &[f64],
        data_y: &[f64],
        data_z: &[f64],
        params: &[f64; 6],
    ) -> f64 {
        #[cfg(target_arch = "x86_64")]
        if common::cpu_features::has_avx2_fma() {
            return unsafe {
                simd_avx2::batch_compute_chi2_avx2(self, data_x, data_y, data_z, params)
            };
        }
        #[cfg(target_arch = "aarch64")]
        {
            return unsafe {
                simd_neon::batch_compute_chi2_neon(self, data_x, data_y, data_z, params)
            };
        }
        // Scalar fallback
        accumulate_chi2(self, data_x, data_y, data_z, None, params, 0..data_x.len())
    }
}

/// Fit a 2D Gaussian to a star stamp via Levenberg-Marquardt (f64 throughout, ~0.01 px
/// centroid accuracy). When `noise` is set, each pixel is weighted by `1/σ²` from the CCD
/// noise model so the shot-noisy bright core doesn't bias the fit (PR1); `None` is a plain
/// unweighted fit.
pub(crate) fn fit_gaussian_2d(
    pixels: &Buffer2<f32>,
    pos: Vec2,
    stamp_radius: usize,
    background: f32,
    noise: Option<FitNoise>,
    config: &GaussianFitConfig,
) -> Option<GaussianFitResult> {
    let stamp = extract_stamp(pixels, pos, stamp_radius)?;

    let n = stamp.x.len();
    if n < 7 {
        return None;
    }

    // Convert stamp data to f64 for fitting. Stack-allocated (stamp size is bounded
    // by MAX_STAMP_PIXELS), so the parallel per-star fit loop makes no heap allocations.
    let data_x: ArrayVec<f64, MAX_STAMP_PIXELS> = stamp.x.iter().map(|&v| v as f64).collect();
    let data_y: ArrayVec<f64, MAX_STAMP_PIXELS> = stamp.y.iter().map(|&v| v as f64).collect();
    let data_z: ArrayVec<f64, MAX_STAMP_PIXELS> = stamp.z.iter().map(|&v| v as f64).collect();

    let weights = fit_weights(&data_z, background, noise);

    // Estimate sigma from moments for better initial guess
    let sigma_est = estimate_sigma_from_moments(&stamp.x, &stamp.y, &stamp.z, pos, background);

    let initial_params: [f64; 6] = [
        pos.x as f64,
        pos.y as f64,
        (stamp.peak - background).max(0.01) as f64,
        sigma_est as f64,
        sigma_est as f64,
        background as f64,
    ];

    let model = Gaussian2D {
        stamp_radius: stamp_radius as f64,
    };

    let result = optimize(
        &model,
        &data_x,
        &data_y,
        &data_z,
        weights.as_deref(),
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
    let result_pos = Vec2::new(x0 as f32, y0 as f32);
    if (result_pos - pos).abs().max_element() > stamp_radius as f32 {
        return None;
    }

    // Check for reasonable sigma values
    if sigma_x < 0.5
        || sigma_y < 0.5
        || sigma_x > stamp_radius as f64 * 2.0
        || sigma_y > stamp_radius as f64 * 2.0
    {
        return None;
    }

    Some(GaussianFitResult {
        pos: Vec2::new(x0 as f32, y0 as f32),
        sigma: Vec2::new(sigma_x as f32, sigma_y as f32),
        converged: result.converged,
        debug: GaussianFitDebug {
            amplitude: amplitude as f32,
            background: bg as f32,
            rms_residual: (result.chi2 / n as f64).sqrt() as f32,
            iterations: result.iterations,
        },
    })
}
