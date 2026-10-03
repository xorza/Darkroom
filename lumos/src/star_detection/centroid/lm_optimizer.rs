//! Levenberg-Marquardt optimizer for profile fitting.
//!
//! Generic implementation that can be used for both Gaussian and Moffat fitting.
//! Uses f64 throughout for numerical stability.

use crate::math::linear_system;

/// Pivot magnitude below which the damped Hessian counts as singular and the step is abandoned.
///
/// Three orders tighter than the distortion fits' threshold because the two matrices are not on one
/// scale: this one is built from pixel fluxes over a stamp, theirs from coordinates normalized to
/// ~[-1, 1].
const SINGULAR_PIVOT: f64 = 1e-15;

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
}

impl Default for LMConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            convergence_threshold: 1e-8,
            initial_lambda: 0.001,
            lambda_up: 10.0,
            lambda_down: 0.1,
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
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "a release build reads neither `chi2` nor `iterations`"
    )
)]
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
    pub(super) const fn new(
        x: &'a [f64],
        y: &'a [f64],
        z: &'a [f64],
        weights: Option<&'a [f64]>,
    ) -> Self {
        debug_assert!(
            x.len() == y.len() && y.len() == z.len(),
            "x, y and z are indexed in lockstep"
        );
        if let Some(weights) = weights {
            debug_assert!(weights.len() == x.len(), "one weight per sample");
        }
        Self { x, y, z, weights }
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
    /// Copy the accumulated upper triangle into the lower one, making the Hessian symmetric.
    pub(super) fn mirror_lower_triangle(&mut self) {
        for i in 1..N {
            for j in 0..i {
                self.hessian[i][j] = self.hessian[j][i];
            }
        }
    }
}

/// Trait for models that can be fit with L-M optimization.
pub(super) trait LMModel<const N: usize> {
    /// Evaluate the model at a point.
    fn evaluate(&self, x: f64, y: f64, params: &[f64; N]) -> f64;

    /// Apply parameter constraints after an update.
    fn constrain(&self, params: &mut [f64; N]);

    /// The normal equations (`J^T·W·J`, `J^T·W·r`) and χ² over `data`, weighted when `data`
    /// carries weights, in a single pass: model evaluation, Jacobian and accumulation fused, so
    /// no Jacobian or residual array is stored. The models implement it with the shared vector
    /// kernel.
    fn batch_build_normal_equations(
        &self,
        data: FitData<'_>,
        params: &[f64; N],
    ) -> NormalEquations<N>;

    /// χ² over `data` — the (weighted) sum of squared residuals — from the same residuals
    /// [`Self::batch_build_normal_equations`] takes.
    fn batch_compute_chi2(&self, data: FitData<'_>, params: &[f64; N]) -> f64;

    /// Fit this model to `data` by Levenberg-Marquardt, starting from `initial_params`.
    fn fit(&self, data: FitData<'_>, initial_params: [f64; N], config: &LMConfig) -> LMResult<N> {
        let mut params = initial_params;
        let mut lambda = config.initial_lambda;
        let mut converged = false;
        let mut iterations = 0;

        // Normal equations at the current `params`. Rebuilt only when `params` actually moves — a
        // rejected step changes only `lambda`, so the cached (vectorized) Jacobian pass is
        // reused across damping retries instead of being recomputed identically every iteration.
        let mut equations = self.batch_build_normal_equations(data, &params);
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

            // The solve consumes both operands, so the damped copy above is the only one made —
            // the gradient is copied into `delta`, which the solution then overwrites.
            let mut delta = equations.gradient;
            let solved = linear_system::solve_in_place(
                damped_hessian.as_flattened_mut(),
                &mut delta,
                SINGULAR_PIVOT,
            );
            if solved.is_none() {
                break;
            }

            let mut new_params = params;
            for (p, d) in new_params.iter_mut().zip(delta.iter()) {
                *p += d;
            }
            self.constrain(&mut new_params);

            let new_chi2 = self.batch_compute_chi2(data, &new_params);

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

                equations = self.batch_build_normal_equations(data, &params);
            } else {
                // A non-finite χ² from a bad trial point skips the relative-change test, which
                // would be NaN, and falls straight through to the lambda ramp.
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
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::star_detection::centroid::lm_optimizer::{FitData, LMModel, NormalEquations};

    impl<'a> FitData<'a> {
        pub(crate) const fn unweighted(x: &'a [f64], y: &'a [f64], z: &'a [f64]) -> Self {
            Self::new(x, y, z, None)
        }
    }

    impl<const N: usize> NormalEquations<N> {
        pub(crate) const fn zeroed() -> Self {
            Self {
                hessian: [[0.0f64; N]; N],
                gradient: [0.0f64; N],
                chi2: 0.0,
            }
        }
    }

    /// The model's value at one point and its Jacobian row there.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct ModelSample<const N: usize> {
        pub(crate) value: f64,
        pub(crate) jacobian: [f64; N],
    }

    /// A model's scalar value and Jacobian row at one point: the definition each model's vector
    /// lanes mirror, and the reference the batch kernels are tested against.
    pub(crate) trait ModelJacobian<const N: usize>: LMModel<N> {
        fn evaluate_and_jacobian(&self, x: f64, y: f64, params: &[f64; N]) -> ModelSample<N>;
    }

    /// Scalar reference for the normal equations: `J^T·J`, `J^T·r`, and `Σr²`, from a jacobian and
    /// residuals computed by the caller.
    ///
    /// Ground truth in vector-vs-scalar validation tests, so it re-derives the symmetric fill instead
    /// of calling [`NormalEquations::mirror_lower_triangle`] — sharing that step with the code
    /// under test would let a bug in it pass unnoticed. Unweighted, matching the unweighted batch
    /// paths it is compared against.
    pub(crate) fn reference_normal_equations<const N: usize>(
        jacobian: &[[f64; N]],
        residuals: &[f64],
    ) -> NormalEquations<N> {
        let mut equations = NormalEquations::zeroed();
        for (row, &r) in jacobian.iter().zip(residuals.iter()) {
            equations.chi2 += r * r;
            for i in 0..N {
                equations.gradient[i] += row[i] * r;
                for j in i..N {
                    equations.hessian[i][j] += row[i] * row[j];
                }
            }
        }
        for i in 1..N {
            for j in 0..i {
                equations.hessian[i][j] = equations.hessian[j][i];
            }
        }
        equations
    }

    /// A model's own values on a `size × size` grid of integer coordinates from 0: data the model
    /// fits exactly at the parameters that made it.
    #[derive(Debug)]
    pub(crate) struct ModelStamp {
        x: Vec<f64>,
        y: Vec<f64>,
        z: Vec<f64>,
    }

    impl ModelStamp {
        pub(crate) fn of<M: LMModel<N>, const N: usize>(
            model: &M,
            size: usize,
            params: &[f64; N],
        ) -> Self {
            let mut stamp = Self {
                x: Vec::new(),
                y: Vec::new(),
                z: Vec::new(),
            };
            for iy in 0..size {
                for ix in 0..size {
                    let (x, y) = (ix as f64, iy as f64);
                    stamp.x.push(x);
                    stamp.y.push(y);
                    stamp.z.push(model.evaluate(x, y, params));
                }
            }
            stamp
        }

        pub(crate) fn data(&self) -> FitData<'_> {
            FitData::unweighted(&self.x, &self.y, &self.z)
        }
    }

    /// `model`'s batch normal equations and χ² over `stamp` at `params`, against
    /// [`reference_normal_equations`].
    ///
    /// The batch path may sum in another order (vector lanes, then the lanes together). Two orders
    /// of one sum of k terms differ by at most `2·γₖ·Σ|tᵢ|` with `γₖ = k·u / (1 − k·u)` (Higham,
    /// *Accuracy and Stability of Numerical Algorithms*, §3.1), so each entry is held to that,
    /// with `Σ|tᵢ|` the same equations built from `|J|` and `|r|`. Uniform weights `w` must give
    /// `w` times the unweighted equations, through the same kernels.
    pub(crate) fn assert_batch_matches_reference<M: ModelJacobian<N>, const N: usize>(
        model: &M,
        stamp: &ModelStamp,
        params: &[f64; N],
    ) {
        let mut jacobian = Vec::new();
        let mut residuals = Vec::new();
        for ((&x, &y), &z) in stamp.x.iter().zip(&stamp.y).zip(&stamp.z) {
            let ModelSample {
                value,
                jacobian: row,
            } = model.evaluate_and_jacobian(x, y, params);
            jacobian.push(row);
            residuals.push(z - value);
        }
        let reference = reference_normal_equations(&jacobian, &residuals);
        let magnitude = reference_normal_equations(
            &jacobian
                .iter()
                .map(|row| row.map(f64::abs))
                .collect::<Vec<_>>(),
            &residuals.iter().map(|r| r.abs()).collect::<Vec<_>>(),
        );
        let k = (stamp.x.len() + 2) as f64 * f64::EPSILON / 2.0;
        let gamma = k / (1.0 - k);
        let within = |what: &str, got: f64, want: f64, magnitude: f64| {
            assert!(
                (got - want).abs() <= 2.0 * gamma * magnitude,
                "{what}: {got} vs {want} (bound {})",
                2.0 * gamma * magnitude
            );
        };
        let compare = |label: &str, batch: &NormalEquations<N>, scale: f64| {
            within(
                &format!("{label} χ²"),
                batch.chi2,
                scale * reference.chi2,
                scale * magnitude.chi2,
            );
            for i in 0..N {
                within(
                    &format!("{label} gradient[{i}]"),
                    batch.gradient[i],
                    scale * reference.gradient[i],
                    scale * magnitude.gradient[i],
                );
                for j in 0..N {
                    within(
                        &format!("{label} hessian[{i}][{j}]"),
                        batch.hessian[i][j],
                        scale * reference.hessian[i][j],
                        scale * magnitude.hessian[i][j],
                    );
                }
            }
        };

        compare(
            "batch",
            &model.batch_build_normal_equations(stamp.data(), params),
            1.0,
        );
        within(
            "batch χ² alone",
            model.batch_compute_chi2(stamp.data(), params),
            reference.chi2,
            magnitude.chi2,
        );
        for w in [1.0, 2.0] {
            let weights = vec![w; stamp.x.len()];
            let weighted = FitData::new(&stamp.x, &stamp.y, &stamp.z, Some(&weights));
            compare(
                &format!("weight {w}"),
                &model.batch_build_normal_equations(weighted, params),
                w,
            );
        }
    }

    /// The fused Jacobian of `model` against central differences of its `evaluate`, at each of
    /// `points`: a check independent of how either derivative was written.
    ///
    /// Each step is `h = ε^⅓·max(1, |p|)`, which balances the difference's truncation, `h²·|f‴|/6`,
    /// against its rounding, `ε·|f|/h`: both come to about `ε^⅔ ≈ 4e-11` of the scale. `1e-6` of
    /// `max(1, |f|, |J|)` leaves that four orders of magnitude.
    pub(crate) fn assert_jacobian_matches_differences<M: ModelJacobian<N>, const N: usize>(
        model: &M,
        params: &[f64; N],
        points: &[(f64, f64)],
    ) {
        for &(x, y) in points {
            let ModelSample { value, jacobian } = model.evaluate_and_jacobian(x, y, params);
            for i in 0..N {
                let h = f64::EPSILON.cbrt() * params[i].abs().max(1.0);
                let (mut up, mut down) = (*params, *params);
                up[i] += h;
                down[i] -= h;
                let difference =
                    (model.evaluate(x, y, &up) - model.evaluate(x, y, &down)) / (up[i] - down[i]);
                let scale = 1f64.max(value.abs()).max(jacobian[i].abs());
                assert!(
                    (difference - jacobian[i]).abs() <= 1e-6 * scale,
                    "∂f/∂p{i} at ({x}, {y}): fused {} vs difference {difference}",
                    jacobian[i]
                );
            }
        }
    }
}
