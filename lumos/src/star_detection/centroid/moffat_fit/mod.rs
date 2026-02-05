//! 2D Moffat profile fitting for high-precision centroid computation.
//!
//! The Moffat profile is a better model for stellar PSFs than Gaussian because
//! it has extended wings that match atmospheric seeing:
//!
//! f(x,y) = A × (1 + ((x-x₀)²+(y-y₀)²)/α²)^(-β) + B
//!
//! where α is the core width and β controls the wing slope (typically 2.5-4.5).
//!
//! Uses f64 throughout the fitting pipeline for numerical stability,
//! achieving ~0.01 pixel centroid accuracy.

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

#[cfg(target_arch = "x86_64")]
mod simd_avx2;

use super::lm_optimizer::{LMConfig, LMModel, LMResult, optimize};
use super::{estimate_sigma_from_moments, extract_stamp};
use crate::common::Buffer2;
use crate::math::FWHM_TO_SIGMA;
use glam::Vec2;

/// Configuration for Moffat profile fitting.
#[derive(Debug, Clone)]
pub struct MoffatFitConfig {
    /// L-M optimization parameters.
    pub lm: LMConfig,
    /// Whether to fit beta or fix it to a constant.
    pub fit_beta: bool,
    /// Fixed beta value when fit_beta is false.
    pub fixed_beta: f32,
}

impl Default for MoffatFitConfig {
    fn default() -> Self {
        Self {
            lm: LMConfig::default(),
            fit_beta: false,
            fixed_beta: 2.5,
        }
    }
}

/// Result of 2D Moffat profile fitting.
#[derive(Debug, Clone, Copy)]
pub struct MoffatFitResult {
    /// Position of profile center (sub-pixel).
    pub pos: Vec2,
    /// Amplitude of profile.
    pub amplitude: f32,
    /// Core width parameter (alpha).
    pub alpha: f32,
    /// Power law slope (beta).
    pub beta: f32,
    /// Background level.
    pub background: f32,
    /// FWHM computed from alpha and beta.
    pub fwhm: f32,
    /// RMS residual of fit.
    pub rms_residual: f32,
    /// Whether the fit converged.
    pub converged: bool,
    /// Number of iterations used.
    pub iterations: usize,
}

/// Strategy for computing `u^(-beta)` efficiently.
/// Pre-computed at model construction to avoid per-pixel branching.
#[derive(Debug, Clone, Copy)]
enum PowStrategy {
    /// beta is a half-integer (n + 0.5): use `1 / (u^n * sqrt(u))`
    HalfInt { int_part: u32 },
    /// beta is an integer: use `1 / u^n`
    Int { n: u32 },
    /// General case: use `u.powf(-beta)`
    General { neg_beta: f64 },
}

/// Compute u^(-beta) using the pre-selected strategy.
#[inline]
fn fast_pow_neg(u: f64, strategy: PowStrategy) -> f64 {
    match strategy {
        PowStrategy::HalfInt { int_part } => {
            // u^(-(n+0.5)) = 1 / (u^n * sqrt(u))
            let u_n = int_pow(u, int_part);
            1.0 / (u_n * u.sqrt())
        }
        PowStrategy::Int { n } => 1.0 / int_pow(u, n),
        PowStrategy::General { neg_beta } => u.powf(neg_beta),
    }
}

/// Compute u^n for small integer n using repeated squaring.
#[inline]
fn int_pow(u: f64, n: u32) -> f64 {
    match n {
        0 => 1.0,
        1 => u,
        2 => u * u,
        3 => u * u * u,
        4 => {
            let u2 = u * u;
            u2 * u2
        }
        5 => {
            let u2 = u * u;
            u2 * u2 * u
        }
        _ => u.powi(n as i32),
    }
}

/// Select optimal strategy for computing u^(-beta).
fn select_pow_strategy(beta: f64) -> PowStrategy {
    let rounded = (beta * 2.0).round();
    let is_half_int = (beta * 2.0 - rounded).abs() < 1e-10;

    if is_half_int {
        let doubled = rounded as i64;
        if doubled % 2 == 0 {
            // Integer beta
            PowStrategy::Int {
                n: (doubled / 2) as u32,
            }
        } else {
            // Half-integer beta (n + 0.5)
            PowStrategy::HalfInt {
                int_part: (doubled / 2) as u32,
            }
        }
    } else {
        PowStrategy::General { neg_beta: -beta }
    }
}

/// Moffat model with fixed beta (5 parameters).
/// Parameters: [x0, y0, amplitude, alpha, background]
#[derive(Debug)]
pub(crate) struct MoffatFixedBeta {
    pub stamp_radius: f64,
    pub beta: f64,
    pow_strategy: PowStrategy,
}

impl MoffatFixedBeta {
    pub fn new(stamp_radius: f64, beta: f64) -> Self {
        Self {
            stamp_radius,
            beta,
            pow_strategy: select_pow_strategy(beta),
        }
    }
}

impl LMModel<5> for MoffatFixedBeta {
    #[inline]
    fn evaluate(&self, x: f64, y: f64, params: &[f64; 5]) -> f64 {
        let [x0, y0, amp, alpha, bg] = *params;
        let r2 = (x - x0).powi(2) + (y - y0).powi(2);
        let u = 1.0 + r2 / (alpha * alpha);
        amp * fast_pow_neg(u, self.pow_strategy) + bg
    }

    #[inline]
    fn jacobian_row(&self, x: f64, y: f64, params: &[f64; 5]) -> [f64; 5] {
        let [x0, y0, amp, alpha, _bg] = *params;
        let alpha2 = alpha * alpha;
        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let u_neg_beta = fast_pow_neg(u, self.pow_strategy);
        let u_neg_beta_m1 = u_neg_beta / u;
        let common = 2.0 * amp * self.beta / alpha2 * u_neg_beta_m1;

        [
            common * dx,         // df/dx0
            common * dy,         // df/dy0
            u_neg_beta,          // df/damp
            common * r2 / alpha, // df/dalpha
            1.0,                 // df/dbg
        ]
    }

    #[inline]
    fn evaluate_and_jacobian(&self, x: f64, y: f64, params: &[f64; 5]) -> (f64, [f64; 5]) {
        let [x0, y0, amp, alpha, bg] = *params;
        let alpha2 = alpha * alpha;
        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let u_neg_beta = fast_pow_neg(u, self.pow_strategy);
        let model_val = amp * u_neg_beta + bg;
        let u_neg_beta_m1 = u_neg_beta / u;
        let common = 2.0 * amp * self.beta / alpha2 * u_neg_beta_m1;

        (
            model_val,
            [
                common * dx,         // df/dx0
                common * dy,         // df/dy0
                u_neg_beta,          // df/damp
                common * r2 / alpha, // df/dalpha
                1.0,                 // df/dbg
            ],
        )
    }

    #[inline]
    fn constrain(&self, params: &mut [f64; 5]) {
        params[2] = params[2].max(0.01); // Amplitude > 0
        params[3] = params[3].clamp(0.5, self.stamp_radius); // Alpha
    }

    #[allow(clippy::needless_range_loop)]
    fn batch_build_normal_equations(
        &self,
        data_x: &[f64],
        data_y: &[f64],
        data_z: &[f64],
        params: &[f64; 5],
    ) -> ([[f64; 5]; 5], [f64; 5], f64) {
        #[cfg(target_arch = "x86_64")]
        if common::cpu_features::has_avx2_fma() {
            // SAFETY: AVX2+FMA availability checked above
            return unsafe {
                simd_avx2::batch_build_normal_equations_avx2(self, data_x, data_y, data_z, params)
            };
        }
        // Scalar fallback
        let mut hessian = [[0.0f64; 5]; 5];
        let mut gradient = [0.0f64; 5];
        let mut chi2 = 0.0f64;
        for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
            let (model_val, row) = self.evaluate_and_jacobian(x, y, params);
            let r = z - model_val;
            chi2 += r * r;
            for i in 0..5 {
                gradient[i] += row[i] * r;
                for j in i..5 {
                    hessian[i][j] += row[i] * row[j];
                }
            }
        }
        for i in 1..5 {
            for j in 0..i {
                hessian[i][j] = hessian[j][i];
            }
        }
        (hessian, gradient, chi2)
    }

    fn batch_compute_chi2(
        &self,
        data_x: &[f64],
        data_y: &[f64],
        data_z: &[f64],
        params: &[f64; 5],
    ) -> f64 {
        #[cfg(target_arch = "x86_64")]
        if common::cpu_features::has_avx2_fma() {
            // SAFETY: AVX2+FMA availability checked above
            return unsafe {
                simd_avx2::batch_compute_chi2_avx2(self, data_x, data_y, data_z, params)
            };
        }
        // Scalar fallback
        data_x
            .iter()
            .zip(data_y.iter())
            .zip(data_z.iter())
            .map(|((&x, &y), &z)| {
                let residual = z - self.evaluate(x, y, params);
                residual * residual
            })
            .sum()
    }
}

/// Moffat model with variable beta (6 parameters).
/// Parameters: [x0, y0, amplitude, alpha, beta, background]
#[derive(Debug)]
pub(crate) struct MoffatVariableBeta {
    pub stamp_radius: f64,
}

impl LMModel<6> for MoffatVariableBeta {
    #[inline]
    fn evaluate(&self, x: f64, y: f64, params: &[f64; 6]) -> f64 {
        let [x0, y0, amp, alpha, beta, bg] = *params;
        let r2 = (x - x0).powi(2) + (y - y0).powi(2);
        amp * (1.0 + r2 / (alpha * alpha)).powf(-beta) + bg
    }

    #[inline]
    fn jacobian_row(&self, x: f64, y: f64, params: &[f64; 6]) -> [f64; 6] {
        let [x0, y0, amp, alpha, beta, _bg] = *params;
        let alpha2 = alpha * alpha;
        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let ln_u = u.ln();
        let u_neg_beta = (-beta * ln_u).exp();
        let u_neg_beta_m1 = u_neg_beta / u;
        let common = 2.0 * amp * beta / alpha2 * u_neg_beta_m1;

        [
            common * dx,              // df/dx0
            common * dy,              // df/dy0
            u_neg_beta,               // df/damp
            common * r2 / alpha,      // df/dalpha
            -amp * ln_u * u_neg_beta, // df/dbeta
            1.0,                      // df/dbg
        ]
    }

    #[inline]
    fn evaluate_and_jacobian(&self, x: f64, y: f64, params: &[f64; 6]) -> (f64, [f64; 6]) {
        let [x0, y0, amp, alpha, beta, bg] = *params;
        let alpha2 = alpha * alpha;
        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let ln_u = u.ln();
        let u_neg_beta = (-beta * ln_u).exp();
        let model_val = amp * u_neg_beta + bg;
        let u_neg_beta_m1 = u_neg_beta / u;
        let common = 2.0 * amp * beta / alpha2 * u_neg_beta_m1;

        (
            model_val,
            [
                common * dx,              // df/dx0
                common * dy,              // df/dy0
                u_neg_beta,               // df/damp
                common * r2 / alpha,      // df/dalpha
                -amp * ln_u * u_neg_beta, // df/dbeta
                1.0,                      // df/dbg
            ],
        )
    }

    #[inline]
    fn constrain(&self, params: &mut [f64; 6]) {
        params[2] = params[2].max(0.01); // Amplitude > 0
        params[3] = params[3].clamp(0.5, self.stamp_radius); // Alpha
        params[4] = params[4].clamp(1.5, 10.0); // Beta
    }
}

/// Fit a 2D Moffat profile to a star stamp.
/// All fitting is done in f64 for numerical stability.
pub fn fit_moffat_2d(
    pixels: &Buffer2<f32>,
    pos: Vec2,
    stamp_radius: usize,
    background: f32,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let (data_x_f32, data_y_f32, data_z_f32, peak_value) =
        extract_stamp(pixels, pos, stamp_radius)?;

    let n = data_x_f32.len();
    let n_params = if config.fit_beta { 6 } else { 5 };
    if n < n_params + 1 {
        return None;
    }

    // Convert stamp data to f64 for fitting
    let data_x: Vec<f64> = data_x_f32.iter().map(|&v| v as f64).collect();
    let data_y: Vec<f64> = data_y_f32.iter().map(|&v| v as f64).collect();
    let data_z: Vec<f64> = data_z_f32.iter().map(|&v| v as f64).collect();

    let initial_amplitude = (peak_value - background).max(0.01);

    // Estimate sigma from moments, then convert to alpha
    let sigma_est =
        estimate_sigma_from_moments(&data_x_f32, &data_y_f32, &data_z_f32, pos, background);
    let fwhm_est = sigma_est * FWHM_TO_SIGMA;
    let initial_alpha =
        fwhm_beta_to_alpha(fwhm_est, config.fixed_beta).clamp(0.5, stamp_radius as f32);

    if config.fit_beta {
        fit_with_variable_beta(
            &data_x,
            &data_y,
            &data_z,
            pos,
            initial_amplitude,
            initial_alpha,
            background,
            stamp_radius,
            n,
            config,
        )
    } else {
        fit_with_fixed_beta(
            &data_x,
            &data_y,
            &data_z,
            pos,
            initial_amplitude,
            initial_alpha,
            background,
            stamp_radius,
            n,
            config,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn fit_with_fixed_beta(
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    pos: Vec2,
    initial_amplitude: f32,
    initial_alpha: f32,
    background: f32,
    stamp_radius: usize,
    n: usize,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let initial_params: [f64; 5] = [
        pos.x as f64,
        pos.y as f64,
        initial_amplitude as f64,
        initial_alpha as f64,
        background as f64,
    ];

    let model = MoffatFixedBeta::new(stamp_radius as f64, config.fixed_beta as f64);

    let result = optimize(&model, data_x, data_y, data_z, initial_params, &config.lm);

    let [x0, y0, amplitude, alpha, bg] = result.params;
    let result_pos = Vec2::new(x0 as f32, y0 as f32);

    if !validate_position(result_pos, pos, alpha as f32, stamp_radius) {
        return None;
    }

    let rms = (result.chi2 / n as f64).sqrt() as f32;
    let fwhm = alpha_beta_to_fwhm(alpha as f32, config.fixed_beta);

    Some(MoffatFitResult {
        pos: result_pos,
        amplitude: amplitude as f32,
        alpha: alpha as f32,
        beta: config.fixed_beta,
        background: bg as f32,
        fwhm,
        rms_residual: rms,
        converged: result.converged,
        iterations: result.iterations,
    })
}

#[allow(clippy::too_many_arguments)]
fn fit_with_variable_beta(
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    pos: Vec2,
    initial_amplitude: f32,
    initial_alpha: f32,
    background: f32,
    stamp_radius: usize,
    n: usize,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let initial_params: [f64; 6] = [
        pos.x as f64,
        pos.y as f64,
        initial_amplitude as f64,
        initial_alpha as f64,
        config.fixed_beta as f64,
        background as f64,
    ];
    let model = MoffatVariableBeta {
        stamp_radius: stamp_radius as f64,
    };

    let result = optimize(&model, data_x, data_y, data_z, initial_params, &config.lm);

    let [x0, y0, amplitude, alpha, beta, bg] = result.params;
    let result_pos = Vec2::new(x0 as f32, y0 as f32);

    if !validate_position(result_pos, pos, alpha as f32, stamp_radius) {
        return None;
    }

    let rms = (result.chi2 / n as f64).sqrt() as f32;
    let fwhm = alpha_beta_to_fwhm(alpha as f32, beta as f32);

    Some(MoffatFitResult {
        pos: result_pos,
        amplitude: amplitude as f32,
        alpha: alpha as f32,
        beta: beta as f32,
        background: bg as f32,
        fwhm,
        rms_residual: rms,
        converged: result.converged,
        iterations: result.iterations,
    })
}

fn validate_position(result_pos: Vec2, input_pos: Vec2, alpha: f32, stamp_radius: usize) -> bool {
    if (result_pos - input_pos).abs().max_element() > stamp_radius as f32 {
        return false;
    }
    if alpha < 0.5 || alpha > stamp_radius as f32 * 2.0 {
        return false;
    }
    true
}

/// Convert Moffat alpha and beta to FWHM.
/// FWHM = 2 * alpha * sqrt(2^(1/beta) - 1)
#[inline]
pub fn alpha_beta_to_fwhm(alpha: f32, beta: f32) -> f32 {
    2.0 * alpha * (2.0f32.powf(1.0 / beta) - 1.0).sqrt()
}

/// Convert FWHM and beta to Moffat alpha.
/// alpha = FWHM / (2 * sqrt(2^(1/beta) - 1))
#[inline]
pub fn fwhm_beta_to_alpha(fwhm: f32, beta: f32) -> f32 {
    fwhm / (2.0 * (2.0f32.powf(1.0 / beta) - 1.0).sqrt())
}
