//! The profile models' side of a fit: the samples, and the model as [`LmController`] reads it.

use crate::math::lm_controller::{LmController, LmFit, LmProblem, NormalEquations};
use crate::math::pixel_quadrature::PixelQuadrature;

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

/// A profile model that can be fit with L-M optimization. Its samples are pixels, so it is fit
/// as the profile integrated over each by its [`PixelQuadrature`].
pub(super) trait LMModel<const N: usize>: std::fmt::Debug {
    /// The profile at a point.
    fn point(&self, x: f64, y: f64, params: &[f64; N]) -> f64;

    /// The quadrature the profile is integrated over a pixel by.
    fn quadrature(&self) -> &PixelQuadrature;

    /// Integrate the profile by the Gauss–Legendre rule of `order` from now on.
    fn integrate_at(&mut self, order: usize);

    /// The smallest order that integrates the profile at `params` over a pixel within
    /// [`PIXEL_MEAN_TOLERANCE`](crate::star_detection::centroid::PIXEL_MEAN_TOLERANCE) of its
    /// amplitude.
    fn sufficient_order(&self, params: &[f64; N]) -> usize;

    /// The profile integrated over the pixel centred at `(x, y)`.
    fn evaluate(&self, x: f64, y: f64, params: &[f64; N]) -> f64 {
        self.quadrature()
            .integrate(x, y, |x, y| self.point(x, y, params))
    }

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

    /// Fit this model to `data` from `initial_params`; `None` when the fit does not converge.
    fn fit(&self, data: FitData<'_>, initial_params: [f64; N]) -> Option<LmFit<N>>
    where
        Self: Sized,
    {
        LmController::STANDARD.fit(&ProfileProblem { model: self, data }, initial_params)
    }
}

/// A profile model and the samples it is fit against, as one least-squares problem.
#[derive(Debug)]
struct ProfileProblem<'a, M> {
    model: &'a M,
    data: FitData<'a>,
}

impl<M: LMModel<N>, const N: usize> LmProblem<N> for ProfileProblem<'_, M> {
    fn normal_equations(&self, params: &[f64; N]) -> NormalEquations<N> {
        self.model.batch_build_normal_equations(self.data, params)
    }

    fn chi2(&self, params: &[f64; N]) -> f64 {
        self.model.batch_compute_chi2(self.data, params)
    }

    fn constrain(&self, params: &mut [f64; N]) {
        self.model.constrain(params);
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::math::lm_controller::NormalEquations;
    use crate::star_detection::centroid::lm_optimizer::{FitData, LMModel};

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

    /// A model's scalar value and Jacobian row: the definition each model's vector lanes mirror,
    /// and the reference the batch kernels are tested against.
    pub(crate) trait ModelJacobian<const N: usize>: LMModel<N> {
        /// At one point.
        fn point_and_jacobian(&self, x: f64, y: f64, params: &[f64; N]) -> ModelSample<N>;

        /// Over the pixel centred at `(x, y)`, by the model's quadrature in its order.
        fn evaluate_and_jacobian(&self, x: f64, y: f64, params: &[f64; N]) -> ModelSample<N> {
            let quadrature = self.quadrature();
            let mut sum = ModelSample {
                value: 0.0,
                jacobian: [0.0; N],
            };
            for (&dy, &wy) in quadrature.nodes().iter().zip(quadrature.weights()) {
                for (&dx, &wx) in quadrature.nodes().iter().zip(quadrature.weights()) {
                    let point = self.point_and_jacobian(x + dx, y + dy, params);
                    let weight = wx * wy;
                    sum.value = weight.mul_add(point.value, sum.value);
                    for (total, term) in sum.jacobian.iter_mut().zip(point.jacobian) {
                        *total = weight.mul_add(term, *total);
                    }
                }
            }
            sum
        }
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
