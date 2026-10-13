#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "TPS is kept as work in progress: its tests are its only callers until it \
                  joins registration as a post-RANSAC distortion correction"
    )
)]

//! Thin-Plate Spline (TPS) distortion modeling.
//!
//! Smooth RBF interpolation that minimizes "bending energy":
//!
//! ```text
//! f(x,y) = a₀ + a₁x + a₂y + Σᵢ wᵢ U(||(x,y) - (xᵢ,yᵢ)||)
//! ```
//!
//! where U(r) = r² log(r). Use when distortion is non-radial or non-uniform.

mod distortion_map;

use glam::DVec2;

use crate::math::linear_system;
use crate::registration::distortion::SINGULAR_THRESHOLD;
use crate::registration::point_normalization::PointNormalization;

/// Configuration for thin-plate spline fitting.
///
/// No default: the right λ depends on how noisy the control points are. At 0 the spline passes
/// through every one exactly, so it reproduces each centroid's error as distortion; measured star
/// positions want λ > 0.
#[derive(Debug, Clone, Copy)]
struct TpsConfig {
    /// Regularization parameter (λ). Higher values produce smoother interpolation that passes
    /// less close to the control points.
    regularization: f64,
}

/// Thin-plate spline for 2D coordinate transformation.
///
/// This implements a smooth, non-rigid transformation that can model
/// local distortions in optical systems.
#[derive(Debug, Clone)]
struct ThinPlateSpline {
    /// Control points in normalized coordinates
    control_points: Vec<DVec2>,
    /// Weights for the radial basis functions (x-direction)
    weights_x: Vec<f64>,
    /// Weights for the radial basis functions (y-direction)
    weights_y: Vec<f64>,
    /// Affine coefficients for x: a0 + a1*x + a2*y (in normalized space)
    affine_x: [f64; 3],
    /// Affine coefficients for y: b0 + b1*x + b2*y (in normalized space)
    affine_y: [f64; 3],
    /// Coordinate normalization the control points and coefficients are expressed in
    norm: PointNormalization,
}

impl ThinPlateSpline {
    /// The spline through `source_points` onto `target_points`, paired by index; `None` for
    /// fewer than three points or a singular system.
    fn fit(source_points: &[DVec2], target_points: &[DVec2], config: TpsConfig) -> Option<Self> {
        debug_assert_eq!(
            source_points.len(),
            target_points.len(),
            "control points pair by index"
        );
        let n = source_points.len();
        if n < 3 {
            return None;
        }

        // Coordinates in [-1, 1] condition the system: the kernel r²·ln(r) amplifies scale
        // differences (r = 7200 gives ~4.6e8 while the affine terms are O(6000)).
        let norm = compute_normalization(source_points);

        let src_norm: Vec<DVec2> = source_points.iter().map(|&p| norm.normalize(p)).collect();
        let tgt_norm: Vec<DVec2> = target_points.iter().map(|&p| norm.normalize(p)).collect();

        // Build the TPS system matrix in normalized coordinates.
        // The system has the form:
        // [K + λI  P] [w]   [v]
        // [P^T     0] [a] = [0]
        //
        // where K[i,j] = U(||p_i - p_j||)
        //       P[i,:] = [1, x_i, y_i]
        //       w = weights for RBF
        //       a = affine coefficients

        // Row-major and contiguous, because the solver takes it that way — and because a row of
        // `Vec`s cost one allocation per row to hold `f64`s that are read in row order anyway.
        let matrix_size = n + 3;
        let mut matrix = vec![0.0; matrix_size * matrix_size];
        let at = |row: usize, col: usize| row * matrix_size + col;

        // K, the upper-left n×n block.
        for i in 0..n {
            for j in 0..n {
                matrix[at(i, j)] = if i == j {
                    config.regularization
                } else {
                    tps_kernel(src_norm[i].distance(src_norm[j]))
                };
            }
        }

        // P, the upper-right n×3 block, and Pᵀ, the lower-left 3×n.
        for i in 0..n {
            let p = src_norm[i];
            matrix[at(i, n)] = 1.0;
            matrix[at(i, n + 1)] = p.x;
            matrix[at(i, n + 2)] = p.y;

            matrix[at(n, i)] = 1.0;
            matrix[at(n + 1, i)] = p.x;
            matrix[at(n + 2, i)] = p.y;
        }

        let mut solution_x = vec![0.0; matrix_size];
        let mut solution_y = vec![0.0; matrix_size];

        for i in 0..n {
            solution_x[i] = tgt_norm[i].x;
            solution_y[i] = tgt_norm[i].y;
        }

        // Solved per axis against the same system, and the solve consumes its matrix, so each pass
        // gets its own copy. The solution replaces the right-hand side it was solved from.
        let mut scratch = matrix.clone();
        linear_system::solve_in_place(&mut scratch, &mut solution_x, SINGULAR_THRESHOLD)?;
        scratch.copy_from_slice(&matrix);
        linear_system::solve_in_place(&mut scratch, &mut solution_y, SINGULAR_THRESHOLD)?;

        let weights_x: Vec<f64> = solution_x[..n].to_vec();
        let weights_y: Vec<f64> = solution_y[..n].to_vec();

        let affine_x = [solution_x[n], solution_x[n + 1], solution_x[n + 2]];
        let affine_y = [solution_y[n], solution_y[n + 1], solution_y[n + 2]];

        Some(Self {
            control_points: src_norm,
            weights_x,
            weights_y,
            affine_x,
            affine_y,
            norm,
        })
    }

    /// The spline at `p`, in pixels.
    fn transform(&self, p: DVec2) -> DVec2 {
        self.transform_normalized(self.norm.normalize(p))
    }

    /// The spline at `pn`, a point already in the normalized coordinates the fit is held in, in
    /// pixels.
    fn transform_normalized(&self, pn: DVec2) -> DVec2 {
        let affine_coeffs_x = DVec2::new(self.affine_x[1], self.affine_x[2]);
        let affine_coeffs_y = DVec2::new(self.affine_y[1], self.affine_y[2]);
        let mut tx = self.affine_x[0] + affine_coeffs_x.dot(pn);
        let mut ty = self.affine_y[0] + affine_coeffs_y.dot(pn);
        for (i, &cp) in self.control_points.iter().enumerate() {
            let u = tps_kernel(pn.distance(cp));
            tx += self.weights_x[i] * u;
            ty += self.weights_y[i] * u;
        }
        self.norm.denormalize(DVec2::new(tx, ty))
    }

    /// [`Self::transform`] of every point.
    fn transform_points(&self, points: &[DVec2]) -> Vec<DVec2> {
        points.iter().map(|&p| self.transform(p)).collect()
    }

    /// Compute the bending energy of the spline.
    ///
    /// Lower values indicate smoother interpolation. This is useful for
    /// comparing different TPS fits or for choosing regularization parameters.
    fn bending_energy(&self) -> f64 {
        let n = self.control_points.len();
        let mut energy = 0.0;

        for i in 0..n {
            for j in 0..n {
                if i != j {
                    let r = self.control_points[i].distance(self.control_points[j]);
                    let u = tps_kernel(r);
                    energy += self.weights_x[i] * self.weights_x[j] * u;
                    energy += self.weights_y[i] * self.weights_y[j] * u;
                }
            }
        }

        energy
    }

    const fn num_control_points(&self) -> usize {
        self.control_points.len()
    }

    /// The control points, in the normalized coordinates the fit is held in.
    fn control_points(&self) -> &[DVec2] {
        &self.control_points
    }

    /// Compute the residuals at the control points.
    ///
    /// Returns the distance between the transformed source points
    /// and the original target points. With zero regularization,
    /// these should be very close to zero.
    fn compute_residuals(&self, target_points: &[DVec2]) -> Vec<f64> {
        // The control points are held normalized, so they are evaluated as they are, without a
        // round trip through pixels.
        self.control_points
            .iter()
            .zip(target_points.iter())
            .map(|(&pn, &tgt)| self.transform_normalized(pn).distance(tgt))
            .collect()
    }
}

/// Fit the coordinate normalization to a set of points: the bounding-box midpoint as center,
/// half the larger box dimension as scale (so normalized coords are in ~[-1, 1]).
fn compute_normalization(points: &[DVec2]) -> PointNormalization {
    let mut min = DVec2::new(f64::MAX, f64::MAX);
    let mut max = DVec2::new(f64::MIN, f64::MIN);
    for &p in points {
        min = min.min(p);
        max = max.max(p);
    }
    let center = (min + max) * 0.5;
    let range = max - min;
    let scale = range.x.max(range.y) * 0.5;
    // Coincident points have no extent to scale by.
    let scale = if scale < 1e-12 { 1.0 } else { scale };
    PointNormalization::new(center, scale)
}

/// TPS radial basis function: U(r) = r² log(r)
///
/// For r = 0, we define U(0) = 0 (the limit as r → 0).
#[inline]
fn tps_kernel(r: f64) -> f64 {
    if r < 1e-10 { 0.0 } else { r * r * r.ln() }
}

#[cfg(test)]
mod tests;
