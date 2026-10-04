//! [`LmController`]: Levenberg–Marquardt for small dense least-squares problems.

/// One step's normal equations: the Hessian `JᵀWJ`, the gradient `JᵀW·r` with `r` the data less
/// the model, and χ².
///
/// Accumulation fills the Hessian's upper triangle only; [`Self::mirror_lower_triangle`] completes
/// it once the last sample has been added.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NormalEquations<const N: usize> {
    pub(crate) hessian: [[f64; N]; N],
    pub(crate) gradient: [f64; N],
    pub(crate) chi2: f64,
}

impl<const N: usize> NormalEquations<N> {
    /// Copy the accumulated upper triangle into the lower one, making the Hessian symmetric.
    pub(crate) fn mirror_lower_triangle(&mut self) {
        for i in 1..N {
            for j in 0..i {
                self.hessian[i][j] = self.hessian[j][i];
            }
        }
    }
}

/// A least-squares problem in `N` parameters, as [`LmController`] reads it.
pub(crate) trait LmProblem<const N: usize> {
    /// The normal equations and χ² at `params`.
    fn normal_equations(&self, params: &[f64; N]) -> NormalEquations<N>;

    /// χ² at `params`.
    fn chi2(&self, params: &[f64; N]) -> f64;

    /// Project `params` back into the region the model is defined on, after a step.
    fn constrain(&self, params: &mut [f64; N]);
}

/// A converged fit.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the profile fits read only the parameters until position σ reads the rest"
    )
)]
pub(crate) struct LmFit<const N: usize> {
    pub(crate) params: [f64; N],
    pub(crate) chi2: f64,
    pub(crate) iterations: usize,
    /// The diagonal of `(JᵀWJ)⁻¹` at the solution: each parameter's variance per unit of the
    /// residuals' variance. `None` when the undamped Hessian is singular there.
    pub(crate) inverse_hessian_diagonal: Option<[f64; N]>,
}

/// Levenberg–Marquardt as Madsen, Nielsen and Tingleff state it (*Methods for Non-Linear Least
/// Squares Problems*, 2004), on the Marquardt-scaled system.
///
/// - Each step solves `(D·H·D + λ·I)·δ̃ = D·g` with `D = diag(H)^(−½)`, by Cholesky. The scaled
///   matrix has a unit diagonal, so the singularity test, a pivot at or below `N·ε·(1 + λ)`, is
///   relative and holds whatever units the parameters carry.
/// - The gain ratio `ρ` of the actual to the predicted decrease of χ² sets λ by Nielsen's rule:
///   an accepted step multiplies it by `max(⅓, 1 − (2ρ − 1)³)`, a rejected one by `ν`, which
///   doubles on every rejection in a row.
/// - The fit has converged when a step is negligible, `‖D⁻¹·δ‖ ≤ step_tolerance·‖D⁻¹·x‖` — either
///   an accepted step or the undamped Gauss–Newton step from the current point, which no damping
///   shortens; or when that Gauss–Newton step would lower χ² by at most `chi2_tolerance·χ²`, which
///   leaves an error of at most `√chi2_tolerance` of `√χ²` in the scaled units, and is where the
///   rounding of χ² stops a fit of a model the data do not follow exactly; or when the gradient is
///   orthogonal to the residuals: every scaled component
///   `g_i / √(H_ii·χ²)`, the cosine between the residuals and a column of the Jacobian, at most
///   `gradient_tolerance`. In the scaled coordinates every parameter is in units of √χ², so the
///   norms compare like with like and the tests hold at any scale of the data; a parameter near
///   zero is judged against the whole vector, as Madsen, Nielsen and Tingleff state the test. At
///   the solution of noise-free data χ² sits at rounding level and no step can lower it; the
///   Gauss–Newton step then is negligible.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LmController {
    pub(crate) max_iterations: usize,
    pub(crate) step_tolerance: f64,
    pub(crate) chi2_tolerance: f64,
    pub(crate) gradient_tolerance: f64,
}

/// The initial damping of the scaled system: a start believed close to the solution, by Madsen,
/// Nielsen and Tingleff's guidance.
const INITIAL_DAMPING: f64 = 1e-3;

/// A damping past which no step can be found: the gradient direction itself does not decrease χ².
const MAX_DAMPING: f64 = 1e16;

impl LmController {
    /// The settings every fit in the crate runs with.
    pub(crate) const STANDARD: Self = Self {
        max_iterations: 50,
        step_tolerance: 1e-10,
        chi2_tolerance: 1e-12,
        gradient_tolerance: 1e-10,
    };

    /// Fit `problem` from `start`; `None` when it does not converge within the iteration budget,
    /// or when the Hessian is singular.
    pub(crate) fn fit<const N: usize>(
        &self,
        problem: &impl LmProblem<N>,
        start: [f64; N],
    ) -> Option<LmFit<N>> {
        let mut params = start;
        let mut equations = problem.normal_equations(&params);
        let mut chi2 = equations.chi2;
        let mut damping = INITIAL_DAMPING;
        let mut growth = 2.0;
        for iteration in 1..=self.max_iterations {
            if !chi2.is_finite() {
                return None;
            }
            let scale = Scale::of(&equations.hessian)?;
            if chi2 == 0.0 || self.gradient_is_orthogonal(&equations, &scale, chi2) {
                return Some(self.fitted(problem, params, chi2, iteration - 1));
            }
            if let Some(step) = scale.solve_damped(&equations, 0.0)
                && (self.is_negligible(&step, &params, &scale)
                    || predicted_decrease(&equations, &step) <= self.chi2_tolerance * chi2)
            {
                // The remaining Newton correction is below the tolerance: take it and stop.
                for (value, delta) in params.iter_mut().zip(step) {
                    *value += delta;
                }
                problem.constrain(&mut params);
                let chi2 = problem.chi2(&params);
                return Some(self.fitted(problem, params, chi2, iteration));
            }
            let step = scale.solve_damped(&equations, damping)?;
            let mut trial = params;
            for (value, delta) in trial.iter_mut().zip(step) {
                *value += delta;
            }
            problem.constrain(&mut trial);
            let applied: [f64; N] = std::array::from_fn(|i| trial[i] - params[i]);
            let predicted = predicted_decrease(&equations, &applied);
            let trial_chi2 = problem.chi2(&trial);
            let ratio = (chi2 - trial_chi2) / predicted;
            if predicted > 0.0 && trial_chi2.is_finite() && ratio > 0.0 {
                let settled = self.is_negligible(&applied, &params, &scale);
                params = trial;
                chi2 = trial_chi2;
                damping *= (1.0 - (2.0 * ratio - 1.0).powi(3)).max(1.0 / 3.0);
                growth = 2.0;
                if settled {
                    return Some(self.fitted(problem, params, chi2, iteration));
                }
                equations = problem.normal_equations(&params);
            } else {
                damping *= growth;
                growth *= 2.0;
                if damping > MAX_DAMPING {
                    return None;
                }
            }
        }
        None
    }

    fn is_negligible<const N: usize>(
        &self,
        step: &[f64; N],
        params: &[f64; N],
        scale: &Scale<N>,
    ) -> bool {
        let mut step_norm = 0.0;
        let mut params_norm = 0.0;
        for i in 0..N {
            step_norm += (step[i] / scale.factors[i]).powi(2);
            params_norm += (params[i] / scale.factors[i]).powi(2);
        }
        step_norm <= self.step_tolerance * self.step_tolerance * params_norm
    }

    fn gradient_is_orthogonal<const N: usize>(
        &self,
        equations: &NormalEquations<N>,
        scale: &Scale<N>,
        chi2: f64,
    ) -> bool {
        let norm = chi2.sqrt();
        equations
            .gradient
            .iter()
            .zip(&scale.factors)
            .all(|(&gradient, &factor)| (gradient * factor).abs() <= self.gradient_tolerance * norm)
    }

    fn fitted<const N: usize>(
        &self,
        problem: &impl LmProblem<N>,
        params: [f64; N],
        chi2: f64,
        iterations: usize,
    ) -> LmFit<N> {
        let equations = problem.normal_equations(&params);
        LmFit {
            params,
            chi2,
            iterations,
            inverse_hessian_diagonal: Scale::of(&equations.hessian)
                .and_then(|scale| scale.inverse_diagonal(&equations.hessian)),
        }
    }
}

/// The decrease of χ² the Gauss–Newton model predicts for the step `delta`: `2·δᵀg − δᵀHδ`. Exact
/// for the quadratic model whatever produced the step, so it holds for a step the constraints
/// clipped.
fn predicted_decrease<const N: usize>(equations: &NormalEquations<N>, delta: &[f64; N]) -> f64 {
    delta
        .iter()
        .zip(&equations.hessian)
        .zip(&equations.gradient)
        .map(|((&step, row), &gradient)| {
            let curvature: f64 = row.iter().zip(delta).map(|(&h, &d)| h * d).sum();
            step * (2.0 * gradient - curvature)
        })
        .sum()
}

/// Marquardt's scaling: `D = diag(H)^(−½)`, which gives the scaled Hessian a unit diagonal.
#[derive(Debug, Clone, Copy)]
struct Scale<const N: usize> {
    factors: [f64; N],
}

impl<const N: usize> Scale<N> {
    /// `None` when a parameter has no curvature: the data do not constrain it.
    fn of(hessian: &[[f64; N]; N]) -> Option<Self> {
        let mut factors = [0.0; N];
        for (i, factor) in factors.iter_mut().enumerate() {
            let diagonal = hessian[i][i];
            if diagonal.is_nan() || diagonal <= 0.0 {
                return None;
            }
            *factor = 1.0 / diagonal.sqrt();
        }
        Some(Self { factors })
    }

    /// `D·H·D + λ·I`.
    fn scaled(&self, hessian: &[[f64; N]; N], damping: f64) -> [[f64; N]; N] {
        let mut scaled = [[0.0; N]; N];
        for i in 0..N {
            for j in 0..N {
                scaled[i][j] = self.factors[i] * hessian[i][j] * self.factors[j];
            }
            scaled[i][i] = 1.0 + damping;
        }
        scaled
    }

    /// The step `δ = D·δ̃` of the damped scaled system; `None` when it is singular.
    fn solve_damped(&self, equations: &NormalEquations<N>, damping: f64) -> Option<[f64; N]> {
        let factor = Cholesky::of(self.scaled(&equations.hessian, damping), 1.0 + damping)?;
        let mut step: [f64; N] = std::array::from_fn(|i| self.factors[i] * equations.gradient[i]);
        factor.solve(&mut step);
        for (value, scale) in step.iter_mut().zip(&self.factors) {
            *value *= scale;
        }
        Some(step)
    }

    /// The diagonal of `H⁻¹ = D·(DHD)⁻¹·D`; `None` when `H` is singular.
    fn inverse_diagonal(&self, hessian: &[[f64; N]; N]) -> Option<[f64; N]> {
        let factor = Cholesky::of(self.scaled(hessian, 0.0), 1.0)?;
        let mut diagonal = [0.0; N];
        for (i, value) in diagonal.iter_mut().enumerate() {
            let mut column = [0.0; N];
            column[i] = 1.0;
            factor.solve(&mut column);
            *value = self.factors[i] * column[i] * self.factors[i];
        }
        Some(diagonal)
    }
}

/// The Cholesky factor `L` of a symmetric positive-definite matrix, `A = L·Lᵀ`.
#[derive(Debug, Clone, Copy)]
struct Cholesky<const N: usize> {
    lower: [[f64; N]; N],
}

impl<const N: usize> Cholesky<N> {
    /// `None` when a pivot falls to `N·ε·largest_diagonal` or below: the matrix is singular to
    /// working precision.
    fn of(matrix: [[f64; N]; N], largest_diagonal: f64) -> Option<Self> {
        let floor = N as f64 * f64::EPSILON * largest_diagonal;
        let mut lower = [[0.0; N]; N];
        for j in 0..N {
            let pivot = matrix[j][j] - lower[j][..j].iter().map(|&l| l * l).sum::<f64>();
            if pivot.is_nan() || pivot <= floor {
                return None;
            }
            let root = pivot.sqrt();
            lower[j][j] = root;
            for i in j + 1..N {
                let dot: f64 = lower[i][..j]
                    .iter()
                    .zip(&lower[j][..j])
                    .map(|(&a, &b)| a * b)
                    .sum();
                lower[i][j] = (matrix[i][j] - dot) / root;
            }
        }
        Some(Self { lower })
    }

    /// Overwrite `b` with the solution of `L·Lᵀ·x = b`.
    fn solve(&self, b: &mut [f64; N]) {
        for i in 0..N {
            let dot: f64 = self.lower[i][..i]
                .iter()
                .zip(&b[..i])
                .map(|(&l, &x)| l * x)
                .sum();
            b[i] = (b[i] - dot) / self.lower[i][i];
        }
        for i in (0..N).rev() {
            let dot: f64 = (i + 1..N).map(|k| self.lower[k][i] * b[k]).sum();
            b[i] = (b[i] - dot) / self.lower[i][i];
        }
    }
}

#[cfg(test)]
mod tests;
