//! Levenberg-Marquardt optimizer for profile fitting.
//!
//! Generic implementation that can be used for both Gaussian and Moffat fitting.
//! Optimized to reuse buffers across iterations to minimize allocations.

use super::linear_solver::{solve_5x5, solve_6x6};

/// Configuration for Levenberg-Marquardt optimization.
#[derive(Debug, Clone)]
pub struct LMConfig {
    /// Maximum iterations.
    pub max_iterations: usize,
    /// Convergence threshold for parameter changes.
    pub convergence_threshold: f32,
    /// Initial damping parameter.
    pub initial_lambda: f32,
    /// Factor to increase lambda on failed step.
    pub lambda_up: f32,
    /// Factor to decrease lambda on successful step.
    pub lambda_down: f32,
    /// Early termination when position parameters (first 2) converge to this threshold.
    /// The optimizer stops once both x0 and y0 deltas are below this value,
    /// even if other parameters (amplitude, sigma, background) are still changing.
    /// Default is 0 (disabled). Set to 0.001 for centroid-only use cases
    /// where non-position parameters don't matter.
    pub position_convergence_threshold: f32,
}

impl Default for LMConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            convergence_threshold: 1e-6,
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
    pub params: [f32; N],
    pub chi2: f32,
    pub converged: bool,
    pub iterations: usize,
}

/// Trait for models that can be fit with L-M optimization.
pub trait LMModel<const N: usize> {
    /// Evaluate the model at a point.
    fn evaluate(&self, x: f32, y: f32, params: &[f32; N]) -> f32;

    /// Compute partial derivatives at a point.
    fn jacobian_row(&self, x: f32, y: f32, params: &[f32; N]) -> [f32; N];

    /// Apply parameter constraints after an update.
    fn constrain(&self, params: &mut [f32; N]);
}

/// Run L-M optimization for 5-parameter model.
pub fn optimize_5<M: LMModel<5>>(
    model: &M,
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    initial_params: [f32; 5],
    config: &LMConfig,
) -> LMResult<5> {
    let mut params = initial_params;
    let mut lambda = config.initial_lambda;
    let mut prev_chi2 = compute_chi2(model, data_x, data_y, data_z, &params);
    let mut converged = false;
    let mut iterations = 0;

    // Pre-allocate buffers once, reuse across iterations
    let n = data_x.len();
    let mut jacobian = Vec::with_capacity(n);
    let mut residuals = Vec::with_capacity(n);

    for iter in 0..config.max_iterations {
        iterations = iter + 1;

        fill_jacobian_residuals(
            model,
            data_x,
            data_y,
            data_z,
            &params,
            &mut jacobian,
            &mut residuals,
        );
        let (hessian, gradient) = compute_hessian_gradient_5(&jacobian, &residuals);

        let mut damped_hessian = hessian;
        for (i, row) in damped_hessian.iter_mut().enumerate() {
            row[i] *= 1.0 + lambda;
        }

        let Some(delta) = solve_5x5(&damped_hessian, &gradient) else {
            break;
        };

        let mut new_params = params;
        for (p, d) in new_params.iter_mut().zip(delta.iter()) {
            *p += d;
        }
        model.constrain(&mut new_params);

        let new_chi2 = compute_chi2(model, data_x, data_y, data_z, &new_params);

        if new_chi2 < prev_chi2 {
            params = new_params;
            lambda *= config.lambda_down;
            prev_chi2 = new_chi2;

            let max_delta = delta.iter().copied().fold(0.0f32, |a, d| a.max(d.abs()));
            if max_delta < config.convergence_threshold {
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
        } else {
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

/// Run L-M optimization for 6-parameter model.
pub fn optimize_6<M: LMModel<6>>(
    model: &M,
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    initial_params: [f32; 6],
    config: &LMConfig,
) -> LMResult<6> {
    let mut params = initial_params;
    let mut lambda = config.initial_lambda;
    let mut prev_chi2 = compute_chi2(model, data_x, data_y, data_z, &params);
    let mut converged = false;
    let mut iterations = 0;

    // Pre-allocate buffers once, reuse across iterations
    let n = data_x.len();
    let mut jacobian = Vec::with_capacity(n);
    let mut residuals = Vec::with_capacity(n);

    for iter in 0..config.max_iterations {
        iterations = iter + 1;

        fill_jacobian_residuals(
            model,
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
        model.constrain(&mut new_params);

        let new_chi2 = compute_chi2(model, data_x, data_y, data_z, &new_params);

        if new_chi2 < prev_chi2 {
            params = new_params;
            lambda *= config.lambda_down;
            prev_chi2 = new_chi2;

            let max_delta = delta.iter().copied().fold(0.0f32, |a, d| a.max(d.abs()));
            if max_delta < config.convergence_threshold {
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
        } else {
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

fn compute_chi2<const N: usize, M: LMModel<N>>(
    model: &M,
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; N],
) -> f32 {
    data_x
        .iter()
        .zip(data_y.iter())
        .zip(data_z.iter())
        .map(|((&x, &y), &z)| {
            let residual = z - model.evaluate(x, y, params);
            residual * residual
        })
        .sum()
}

/// Fill jacobian and residuals buffers, reusing existing allocations.
fn fill_jacobian_residuals<const N: usize, M: LMModel<N>>(
    model: &M,
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; N],
    jacobian: &mut Vec<[f32; N]>,
    residuals: &mut Vec<f32>,
) {
    jacobian.clear();
    residuals.clear();

    for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
        jacobian.push(model.jacobian_row(x, y, params));
        residuals.push(z - model.evaluate(x, y, params));
    }
}

/// Compute Hessian (J^T J) and gradient (J^T r) for 5-parameter model.
/// Exploits symmetry: only computes upper triangle, then mirrors.
#[allow(clippy::needless_range_loop)]
fn compute_hessian_gradient_5(
    jacobian: &[[f32; 5]],
    residuals: &[f32],
) -> ([[f32; 5]; 5], [f32; 5]) {
    let mut hessian = [[0.0f32; 5]; 5];
    let mut gradient = [0.0f32; 5];

    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..5 {
            gradient[i] += row[i] * r;
            // Only compute upper triangle (j >= i)
            for j in i..5 {
                hessian[i][j] += row[i] * row[j];
            }
        }
    }

    // Mirror upper triangle to lower
    for i in 1..5 {
        for j in 0..i {
            hessian[i][j] = hessian[j][i];
        }
    }

    (hessian, gradient)
}

/// Compute Hessian (J^T J) and gradient (J^T r) for 6-parameter model.
/// Exploits symmetry: only computes upper triangle, then mirrors.
#[allow(clippy::needless_range_loop)]
fn compute_hessian_gradient_6(
    jacobian: &[[f32; 6]],
    residuals: &[f32],
) -> ([[f32; 6]; 6], [f32; 6]) {
    let mut hessian = [[0.0f32; 6]; 6];
    let mut gradient = [0.0f32; 6];

    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..6 {
            gradient[i] += row[i] * r;
            // Only compute upper triangle (j >= i)
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
