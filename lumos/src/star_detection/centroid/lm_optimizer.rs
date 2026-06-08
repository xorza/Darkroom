//! Levenberg-Marquardt optimizer for profile fitting.
//!
//! Generic implementation that can be used for both Gaussian and Moffat fitting.
//! Uses f64 throughout for numerical stability.

use crate::star_detection::centroid::linear_solver::solve;

/// Configuration for Levenberg-Marquardt optimization.
#[derive(Debug, Clone)]
pub struct LMConfig {
    /// Maximum iterations.
    pub max_iterations: usize,
    /// Convergence threshold for parameter changes.
    pub convergence_threshold: f64,
    /// Initial damping parameter.
    pub initial_lambda: f64,
    /// Factor to increase lambda on failed step.
    pub lambda_up: f64,
    /// Factor to decrease lambda on successful step.
    pub lambda_down: f64,
    /// Early termination when position parameters (first 2) converge to this threshold.
    /// The optimizer stops once both x0 and y0 deltas are below this value,
    /// even if other parameters (amplitude, sigma, background) are still changing.
    /// Default is 0 (disabled). Set to 0.0001 for sub-pixel astrometric precision
    /// in centroid-only use cases where non-position parameters don't matter.
    pub position_convergence_threshold: f64,
}

impl Default for LMConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            convergence_threshold: 1e-8,
            initial_lambda: 0.001,
            lambda_up: 10.0,
            lambda_down: 0.1,
            position_convergence_threshold: 0.0,
        }
    }
}

/// Result of L-M optimization.
#[derive(Debug, Clone, Copy)]
pub struct LMResult<const N: usize> {
    pub params: [f64; N],
    pub chi2: f64,
    pub converged: bool,
    pub iterations: usize,
}

/// Trait for models that can be fit with L-M optimization.
pub trait LMModel<const N: usize> {
    /// Evaluate the model at a point.
    fn evaluate(&self, x: f64, y: f64, params: &[f64; N]) -> f64;

    /// Compute partial derivatives at a point.
    fn jacobian_row(&self, x: f64, y: f64, params: &[f64; N]) -> [f64; N];

    /// Evaluate model and compute Jacobian row in a single pass.
    /// Default implementation calls evaluate + jacobian_row separately.
    /// Override to share expensive intermediate computations (e.g., powf).
    #[inline]
    fn evaluate_and_jacobian(&self, x: f64, y: f64, params: &[f64; N]) -> (f64, [f64; N]) {
        (self.evaluate(x, y, params), self.jacobian_row(x, y, params))
    }

    /// Apply parameter constraints after an update.
    fn constrain(&self, params: &mut [f64; N]);

    /// Build normal equations (J^T J, J^T r) and chi² in a single pass.
    /// Fuses model evaluation, Jacobian computation, and Hessian/gradient
    /// accumulation to avoid storing intermediate jacobian/residuals arrays.
    /// Default implementation calls `evaluate_and_jacobian` per pixel.
    /// Override with SIMD to process multiple pixels at once.
    #[allow(clippy::needless_range_loop)]
    fn batch_build_normal_equations(
        &self,
        data_x: &[f64],
        data_y: &[f64],
        data_z: &[f64],
        params: &[f64; N],
    ) -> ([[f64; N]; N], [f64; N], f64) {
        let mut hessian = [[0.0f64; N]; N];
        let mut gradient = [0.0f64; N];
        let mut chi2 = 0.0f64;

        for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
            let (model_val, row) = self.evaluate_and_jacobian(x, y, params);
            let r = z - model_val;
            chi2 += r * r;
            for i in 0..N {
                gradient[i] += row[i] * r;
                for j in i..N {
                    hessian[i][j] += row[i] * row[j];
                }
            }
        }

        // Mirror upper triangle to lower
        for i in 1..N {
            for j in 0..i {
                hessian[i][j] = hessian[j][i];
            }
        }

        (hessian, gradient, chi2)
    }

    /// Batch compute chi² (sum of squared residuals).
    /// Default implementation calls `evaluate` per pixel.
    /// Override with SIMD to process multiple pixels at once.
    fn batch_compute_chi2(
        &self,
        data_x: &[f64],
        data_y: &[f64],
        data_z: &[f64],
        params: &[f64; N],
    ) -> f64 {
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

    /// Weighted `batch_build_normal_equations`: each pixel contributes its inverse-variance
    /// weight `w_i` to chi²/gradient/Hessian. Scalar default — the weighted fit is opt-in
    /// (set a `NoiseModel`), so the unweighted SIMD overrides stay untouched.
    #[allow(clippy::needless_range_loop)]
    fn batch_build_normal_equations_weighted(
        &self,
        data_x: &[f64],
        data_y: &[f64],
        data_z: &[f64],
        weights: &[f64],
        params: &[f64; N],
    ) -> ([[f64; N]; N], [f64; N], f64) {
        let mut hessian = [[0.0f64; N]; N];
        let mut gradient = [0.0f64; N];
        let mut chi2 = 0.0f64;

        for (((&x, &y), &z), &w) in data_x
            .iter()
            .zip(data_y.iter())
            .zip(data_z.iter())
            .zip(weights.iter())
        {
            let (model_val, row) = self.evaluate_and_jacobian(x, y, params);
            let r = z - model_val;
            chi2 += w * r * r;
            for i in 0..N {
                gradient[i] += w * row[i] * r;
                for j in i..N {
                    hessian[i][j] += w * row[i] * row[j];
                }
            }
        }

        for i in 1..N {
            for j in 0..i {
                hessian[i][j] = hessian[j][i];
            }
        }

        (hessian, gradient, chi2)
    }

    /// Weighted `batch_compute_chi2` (inverse-variance).
    fn batch_compute_chi2_weighted(
        &self,
        data_x: &[f64],
        data_y: &[f64],
        data_z: &[f64],
        weights: &[f64],
        params: &[f64; N],
    ) -> f64 {
        data_x
            .iter()
            .zip(data_y.iter())
            .zip(data_z.iter())
            .zip(weights.iter())
            .map(|(((&x, &y), &z), &w)| {
                let r = z - self.evaluate(x, y, params);
                w * r * r
            })
            .sum()
    }
}

/// Run L-M optimization for N-parameter model (generic implementation).
pub fn optimize<const N: usize, M: LMModel<N>>(
    model: &M,
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    weights: Option<&[f64]>,
    initial_params: [f64; N],
    config: &LMConfig,
) -> LMResult<N> {
    // Build normal equations (J^T J, J^T r) + χ² in one fused pass, or its weighted form.
    let build = |p: &[f64; N]| match weights {
        Some(w) => model.batch_build_normal_equations_weighted(data_x, data_y, data_z, w, p),
        None => model.batch_build_normal_equations(data_x, data_y, data_z, p),
    };
    let chi2_at = |p: &[f64; N]| match weights {
        Some(w) => model.batch_compute_chi2_weighted(data_x, data_y, data_z, w, p),
        None => model.batch_compute_chi2(data_x, data_y, data_z, p),
    };

    let mut params = initial_params;
    let mut lambda = config.initial_lambda;
    let mut converged = false;
    let mut iterations = 0;

    // Normal equations at the current `params`. Rebuilt only when `params` actually moves — a
    // rejected step changes only `lambda`, so the cached (SIMD-accelerated) Jacobian pass is reused
    // across damping retries instead of being recomputed identically every iteration.
    let (mut hessian, mut gradient, mut prev_chi2) = build(&params);

    for iter in 0..config.max_iterations {
        iterations = iter + 1;

        let mut damped_hessian = hessian;
        for (i, row) in damped_hessian.iter_mut().enumerate() {
            row[i] *= 1.0 + lambda;
        }

        let Some(delta) = solve(&damped_hessian, &gradient) else {
            break;
        };

        let mut new_params = params;
        for (p, d) in new_params.iter_mut().zip(delta.iter()) {
            *p += d;
        }
        model.constrain(&mut new_params);

        let new_chi2 = chi2_at(&new_params);

        if new_chi2.is_finite() && new_chi2 < prev_chi2 {
            let chi2_rel_change = (prev_chi2 - new_chi2) / prev_chi2.max(1e-30);
            params = new_params;
            lambda *= config.lambda_down;
            prev_chi2 = new_chi2;

            let max_delta = delta.iter().copied().fold(0.0f64, |a, d| a.max(d.abs()));
            if max_delta < config.convergence_threshold || chi2_rel_change < 1e-10 {
                converged = true;
                break;
            }
            // Early exit when only position accuracy matters
            if delta[0].abs() < config.position_convergence_threshold
                && delta[1].abs() < config.position_convergence_threshold
            {
                converged = true;
                break;
            }

            // `params` moved → refresh the normal equations for the next iteration.
            let (h, g, _) = build(&params);
            hessian = h;
            gradient = g;
        } else {
            // Rejected step (worse fit, or a non-finite χ² from a bad trial point): keep `params`
            // and the cached normal equations, just increase damping. A non-finite χ² skips the
            // relative-change test (it would be NaN) and falls straight through to the lambda ramp.
            if new_chi2.is_finite() {
                let chi2_rel_diff = (new_chi2 - prev_chi2) / prev_chi2.max(1e-30);
                if chi2_rel_diff < 1e-10 {
                    converged = true;
                    break;
                }
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
