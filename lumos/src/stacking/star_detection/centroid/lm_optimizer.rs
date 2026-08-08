//! Levenberg-Marquardt optimizer for profile fitting.
//!
//! Generic implementation that can be used for both Gaussian and Moffat fitting.
//! Uses f64 throughout for numerical stability.

use crate::stacking::star_detection::centroid::linear_solver::solve;

/// Configuration for Levenberg-Marquardt optimization.
#[derive(Debug, Clone)]
pub(super) struct LMConfig {
    /// Maximum iterations.
    pub(super) max_iterations: usize,
    /// Convergence threshold for parameter changes.
    pub(super) convergence_threshold: f64,
    /// Initial damping parameter.
    pub(super) initial_lambda: f64,
    /// Factor to increase lambda on failed step.
    pub(super) lambda_up: f64,
    /// Factor to decrease lambda on successful step.
    pub(super) lambda_down: f64,
    /// Early termination when position parameters (first 2) converge to this threshold.
    /// The optimizer stops once both x0 and y0 deltas are below this value,
    /// even if other parameters (amplitude, sigma, background) are still changing.
    /// Default is 0 (disabled). Set to 0.0001 for sub-pixel astrometric precision
    /// in centroid-only use cases where non-position parameters don't matter.
    pub(super) position_convergence_threshold: f64,
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
///
/// `chi2` and `iterations` are the run's report: the fit models read them only into their
/// `cfg(test)` diagnostics, so a release build never looks at either. They are kept ungated
/// anyway — the loop computes both regardless (χ² drives the accept test, the count is the loop
/// variable), so carrying them costs two moves, while gating them would push `#[cfg]` into the
/// optimizer's inner loop and save nothing.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct LMResult<const N: usize> {
    pub(super) params: [f64; N],
    pub(super) chi2: f64,
    pub(super) converged: bool,
    pub(super) iterations: usize,
}

/// The samples a model is fit against, with optional per-pixel inverse-variance
/// weights (`None` ≡ all 1). All three coordinate slices are indexed in lockstep.
#[derive(Debug, Clone, Copy)]
pub(super) struct FitData<'a> {
    pub(super) x: &'a [f64],
    pub(super) y: &'a [f64],
    pub(super) z: &'a [f64],
    pub(super) weights: Option<&'a [f64]>,
}

impl<'a> FitData<'a> {
    pub(super) fn new(
        x: &'a [f64],
        y: &'a [f64],
        z: &'a [f64],
        weights: Option<&'a [f64]>,
    ) -> Self {
        Self { x, y, z, weights }
    }

    pub(super) fn unweighted(x: &'a [f64], y: &'a [f64], z: &'a [f64]) -> Self {
        Self::new(x, y, z, None)
    }

    /// Sample count — the three coordinate slices are indexed in lockstep, so any of them.
    #[inline]
    pub(super) fn len(&self) -> usize {
        self.x.len()
    }

    #[inline]
    fn weight(&self, i: usize) -> f64 {
        self.weights.map_or(1.0, |ws| ws[i])
    }
}

/// One L-M step's normal equations: the Hessian `J^T·W·J`, the gradient `J^T·W·r`, and χ².
///
/// Accumulation fills the Hessian's upper triangle only; [`Self::mirror_lower_triangle`]
/// completes it once the last sample has been added.
#[derive(Debug, Clone, Copy)]
pub(super) struct NormalEquations<const N: usize> {
    pub(super) hessian: [[f64; N]; N],
    pub(super) gradient: [f64; N],
    pub(super) chi2: f64,
}

impl<const N: usize> NormalEquations<N> {
    pub(super) fn zeroed() -> Self {
        Self {
            hessian: [[0.0f64; N]; N],
            gradient: [0.0f64; N],
            chi2: 0.0,
        }
    }

    /// Copy the accumulated upper triangle into the lower one, making the Hessian symmetric.
    pub(super) fn mirror_lower_triangle(&mut self) {
        for i in 1..N {
            for j in 0..i {
                self.hessian[i][j] = self.hessian[j][i];
            }
        }
    }
}

/// Accumulate the normal equations and chi² over `range` into `out` via
/// `model.evaluate_and_jacobian`.
///
/// This is the one scalar per-pixel accumulation loop shared by [`LMModel`]'s
/// default `batch_build_normal_equations` (and its weighted variant), every model's
/// scalar-fallback override (no SIMD available for this target), and every SIMD
/// backend's tail loop over the pixels left after the last full SIMD chunk — so a
/// numerical-stability fix only has to land once instead of in every copy. Leaves the
/// Hessian's lower triangle untouched; callers mirror it once after accumulating.
pub(super) fn accumulate_normal_equations<const N: usize>(
    model: &(impl LMModel<N> + ?Sized),
    data: FitData,
    params: &[f64; N],
    range: std::ops::Range<usize>,
    out: &mut NormalEquations<N>,
) {
    for i in range {
        let w = data.weight(i);
        let (model_val, row) = model.evaluate_and_jacobian(data.x[i], data.y[i], params);
        let r = data.z[i] - model_val;
        out.chi2 += w * r * r;
        for k in 0..N {
            out.gradient[k] += w * row[k] * r;
            for j in k..N {
                out.hessian[k][j] += w * row[k] * row[j];
            }
        }
    }
}

/// Sum chi² ((weighted) squared residuals) over `range` via `model.evaluate`.
/// Companion to [`accumulate_normal_equations`] for the gradient-free chi²-only
/// batch path — see that function's doc for why this is shared rather than
/// duplicated per model/backend.
pub(super) fn accumulate_chi2<const N: usize>(
    model: &(impl LMModel<N> + ?Sized),
    data: FitData,
    params: &[f64; N],
    range: std::ops::Range<usize>,
) -> f64 {
    let mut chi2 = 0.0f64;
    for i in range {
        let w = data.weight(i);
        let residual = data.z[i] - model.evaluate(data.x[i], data.y[i], params);
        chi2 += w * residual * residual;
    }
    chi2
}

/// Build the full normal equations (mirrored Hessian, gradient, chi²) over the whole
/// data set with the scalar accumulation loop — the default body of
/// [`LMModel::batch_build_normal_equations`], also called directly by the fit models'
/// scalar fallbacks when no SIMD backend applies.
pub(super) fn build_normal_equations_scalar<const N: usize>(
    model: &(impl LMModel<N> + ?Sized),
    data: FitData,
    params: &[f64; N],
) -> NormalEquations<N> {
    let mut equations = NormalEquations::zeroed();
    accumulate_normal_equations(model, data, params, 0..data.len(), &mut equations);
    equations.mirror_lower_triangle();
    equations
}

/// Trait for models that can be fit with L-M optimization.
pub(super) trait LMModel<const N: usize> {
    /// Evaluate the model at a point.
    fn evaluate(&self, x: f64, y: f64, params: &[f64; N]) -> f64;

    /// Evaluate the model and its Jacobian row at a point, in one pass.
    ///
    /// Fused rather than split into evaluate + derivatives so the expensive shared intermediate
    /// (the `exp` for a Gaussian, the `powf` for a Moffat) is computed once. Every accumulation
    /// loop goes through this, so it is the only derivative path production code takes.
    fn evaluate_and_jacobian(&self, x: f64, y: f64, params: &[f64; N]) -> (f64, [f64; N]);

    /// Apply parameter constraints after an update.
    fn constrain(&self, params: &mut [f64; N]);

    /// Build normal equations (J^T·W·J, J^T·W·r) and chi² in a single pass.
    /// Fuses model evaluation, Jacobian computation, and Hessian/gradient
    /// accumulation to avoid storing intermediate jacobian/residuals arrays.
    /// Default implementation calls `evaluate_and_jacobian` per pixel, applying
    /// `data.weights` when they are present.
    ///
    /// Override with SIMD to process multiple pixels at once. The weighted fit is opt-in
    /// (set a `NoiseModel`), so an override whose kernel is unweighted-only must test
    /// `data.weights` and delegate to [`build_normal_equations_scalar`] when it is set.
    ///
    /// Both models' overrides spell that dispatch out identically, differing only in `N`. It is
    /// left duplicated: the backend is chosen by `cfg`, so the arms name functions that exist
    /// only under their own `target_arch`, and the sole way to share them is a macro — which
    /// buys ~15 lines at the cost of making an `unsafe` call site expand from something a reader
    /// cannot see.
    fn batch_build_normal_equations(&self, data: FitData, params: &[f64; N]) -> NormalEquations<N> {
        build_normal_equations_scalar(self, data, params)
    }

    /// Batch compute chi² — the (weighted) sum of squared residuals.
    /// Default implementation calls `evaluate` per pixel. Override with SIMD under the same
    /// weighted-delegation rule as [`Self::batch_build_normal_equations`].
    fn batch_compute_chi2(&self, data: FitData, params: &[f64; N]) -> f64 {
        accumulate_chi2(self, data, params, 0..data.len())
    }
}

/// Run L-M optimization for N-parameter model (generic implementation).
pub(super) fn optimize<const N: usize, M: LMModel<N>>(
    model: &M,
    data: FitData,
    initial_params: [f64; N],
    config: &LMConfig,
) -> LMResult<N> {
    let mut params = initial_params;
    let mut lambda = config.initial_lambda;
    let mut converged = false;
    let mut iterations = 0;

    // Normal equations at the current `params`. Rebuilt only when `params` actually moves — a
    // rejected step changes only `lambda`, so the cached (SIMD-accelerated) Jacobian pass is reused
    // across damping retries instead of being recomputed identically every iteration.
    let mut equations = model.batch_build_normal_equations(data, &params);
    // Tracked apart from `equations.chi2`: an accepted step keeps the χ² `batch_compute_chi2`
    // already computed for the new params rather than the rebuild's, so the accept test and the
    // recorded χ² can never disagree by a rounding difference between those two code paths.
    let mut prev_chi2 = equations.chi2;

    for iter in 0..config.max_iterations {
        iterations = iter + 1;

        let mut damped_hessian = equations.hessian;
        for (i, row) in damped_hessian.iter_mut().enumerate() {
            row[i] *= 1.0 + lambda;
        }

        let Some(delta) = solve(&damped_hessian, &equations.gradient) else {
            break;
        };

        let mut new_params = params;
        for (p, d) in new_params.iter_mut().zip(delta.iter()) {
            *p += d;
        }
        model.constrain(&mut new_params);

        let new_chi2 = model.batch_compute_chi2(data, &new_params);

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
            equations = model.batch_build_normal_equations(data, &params);
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
