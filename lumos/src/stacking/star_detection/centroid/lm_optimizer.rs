//! Levenberg-Marquardt optimizer for profile fitting.
//!
//! Generic implementation that can be used for both Gaussian and Moffat fitting.
//! Uses f64 throughout for numerical stability.

use crate::stacking::star_detection::centroid::linear_solver::solve;

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

/// Accumulate the normal equations (Hessian upper triangle, gradient) and chi² over
/// `range` into the given accumulators via `model.evaluate_and_jacobian`. `weights`
/// applies an optional per-pixel inverse-variance weight (`None` ≡ all 1).
///
/// This is the one scalar per-pixel accumulation loop shared by [`LMModel`]'s
/// default `batch_build_normal_equations` (and its weighted variant), every model's
/// scalar-fallback override (no SIMD available for this target), and every SIMD
/// backend's tail loop over the pixels left after the last full SIMD chunk — so a
/// numerical-stability fix only has to land once instead of in every copy. Does not
/// mirror the hessian's lower triangle; callers that need a full matrix do that once
/// after accumulating.
#[allow(clippy::needless_range_loop, clippy::too_many_arguments)]
pub(crate) fn accumulate_normal_equations<const N: usize>(
    model: &(impl LMModel<N> + ?Sized),
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    weights: Option<&[f64]>,
    params: &[f64; N],
    range: std::ops::Range<usize>,
    hessian: &mut [[f64; N]; N],
    gradient: &mut [f64; N],
    chi2: &mut f64,
) {
    for i in range {
        let w = weights.map_or(1.0, |ws| ws[i]);
        let (model_val, row) = model.evaluate_and_jacobian(data_x[i], data_y[i], params);
        let r = data_z[i] - model_val;
        *chi2 += w * r * r;
        for k in 0..N {
            gradient[k] += w * row[k] * r;
            for j in k..N {
                hessian[k][j] += w * row[k] * row[j];
            }
        }
    }
}

/// Sum chi² ((weighted) squared residuals) over `range` via `model.evaluate`.
/// Companion to [`accumulate_normal_equations`] for the gradient-free chi²-only
/// batch path — see that function's doc for why this is shared rather than
/// duplicated per model/backend.
pub(crate) fn accumulate_chi2<const N: usize>(
    model: &(impl LMModel<N> + ?Sized),
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    weights: Option<&[f64]>,
    params: &[f64; N],
    range: std::ops::Range<usize>,
) -> f64 {
    let mut chi2 = 0.0f64;
    for i in range {
        let w = weights.map_or(1.0, |ws| ws[i]);
        let residual = data_z[i] - model.evaluate(data_x[i], data_y[i], params);
        chi2 += w * residual * residual;
    }
    chi2
}

/// Build the full normal equations (mirrored Hessian, gradient) and chi² over the
/// whole data set with the scalar accumulation loop — the shared body of
/// [`LMModel::batch_build_normal_equations`] and its weighted variant, also called
/// directly by the fit models' scalar fallbacks when no SIMD backend applies.
#[allow(clippy::needless_range_loop)]
pub(crate) fn build_normal_equations_scalar<const N: usize>(
    model: &(impl LMModel<N> + ?Sized),
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    weights: Option<&[f64]>,
    params: &[f64; N],
) -> ([[f64; N]; N], [f64; N], f64) {
    let mut hessian = [[0.0f64; N]; N];
    let mut gradient = [0.0f64; N];
    let mut chi2 = 0.0f64;

    accumulate_normal_equations(
        model,
        data_x,
        data_y,
        data_z,
        weights,
        params,
        0..data_x.len(),
        &mut hessian,
        &mut gradient,
        &mut chi2,
    );

    // Mirror upper triangle to lower
    for i in 1..N {
        for j in 0..i {
            hessian[i][j] = hessian[j][i];
        }
    }

    (hessian, gradient, chi2)
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
    fn batch_build_normal_equations(
        &self,
        data_x: &[f64],
        data_y: &[f64],
        data_z: &[f64],
        params: &[f64; N],
    ) -> ([[f64; N]; N], [f64; N], f64) {
        build_normal_equations_scalar(self, data_x, data_y, data_z, None, params)
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
        accumulate_chi2(self, data_x, data_y, data_z, None, params, 0..data_x.len())
    }

    /// Weighted `batch_build_normal_equations`: each pixel contributes its inverse-variance
    /// weight `w_i` to chi²/gradient/Hessian. Scalar default — the weighted fit is opt-in
    /// (set a `NoiseModel`), so the unweighted SIMD overrides stay untouched.
    fn batch_build_normal_equations_weighted(
        &self,
        data_x: &[f64],
        data_y: &[f64],
        data_z: &[f64],
        weights: &[f64],
        params: &[f64; N],
    ) -> ([[f64; N]; N], [f64; N], f64) {
        build_normal_equations_scalar(self, data_x, data_y, data_z, Some(weights), params)
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
        accumulate_chi2(
            self,
            data_x,
            data_y,
            data_z,
            Some(weights),
            params,
            0..data_x.len(),
        )
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
