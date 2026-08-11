// WIP: TPS distortion modeling is not yet integrated into the registration pipeline.
// Blanket allow because no code outside this module (or its tests) calls TPS yet.
// Remove once TPS is integrated as a post-RANSAC distortion correction option.
#![allow(dead_code)]

//! Thin-Plate Spline (TPS) distortion modeling.
//!
//! Smooth RBF interpolation that minimizes "bending energy":
//!
//! ```text
//! f(x,y) = a₀ + a₁x + a₂y + Σᵢ wᵢ U(||(x,y) - (xᵢ,yᵢ)||)
//! ```
//!
//! where U(r) = r² log(r). Use when distortion is non-radial or non-uniform.

use glam::DVec2;

use crate::math::linear_system;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::stacking::registration::distortion::SINGULAR_THRESHOLD;
use crate::stacking::registration::distortion::point_normalization::PointNormalization;

/// Configuration for thin-plate spline fitting.
#[derive(Debug, Clone)]
struct TpsConfig {
    /// Regularization parameter (lambda). Higher values produce smoother
    /// interpolation but may not pass exactly through control points.
    /// Default: 0.0 (exact interpolation)
    regularization: f64,
}

impl Default for TpsConfig {
    fn default() -> Self {
        Self {
            regularization: 0.0,
        }
    }
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
    /// Fit a thin-plate spline to a set of control point correspondences.
    ///
    /// # Arguments
    /// * `source_points` - Source (reference) point positions
    /// * `target_points` - Target point positions
    /// * `config` - TPS configuration
    ///
    /// # Returns
    /// A fitted TPS model, or None if fitting fails (e.g., singular matrix)
    fn fit(source_points: &[DVec2], target_points: &[DVec2], config: TpsConfig) -> Option<Self> {
        let n = source_points.len();
        if n < 3 {
            return None; // Need at least 3 points for TPS
        }

        if source_points.len() != target_points.len() {
            return None;
        }

        // Compute normalization: center and scale coordinates to [-1, 1] range.
        // This dramatically improves conditioning of the TPS system matrix because
        // the kernel r^2*ln(r) amplifies scale differences (e.g. r=7200 gives ~4.6e8
        // while affine terms are O(6000)).
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

        // Fill K matrix (upper-left n×n block)
        for i in 0..n {
            for j in 0..n {
                matrix[at(i, j)] = if i == j {
                    config.regularization
                } else {
                    tps_kernel(src_norm[i].distance(src_norm[j]))
                };
            }
        }

        // Fill P matrix (upper-right n×3 block) and P^T (lower-left 3×n block)
        for i in 0..n {
            let p = src_norm[i];
            matrix[at(i, n)] = 1.0;
            matrix[at(i, n + 1)] = p.x;
            matrix[at(i, n + 2)] = p.y;

            matrix[at(n, i)] = 1.0;
            matrix[at(n + 1, i)] = p.x;
            matrix[at(n + 2, i)] = p.y;
        }

        // Lower-right 3×3 block is zeros (already initialized)

        // Right-hand side vectors (normalized target coordinates)
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

        // Extract weights and affine coefficients
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

    /// Transform a point using the fitted TPS model.
    ///
    /// # Arguments
    /// * `p` - Source point coordinates
    ///
    /// # Returns
    /// Transformed coordinates
    fn transform(&self, p: DVec2) -> DVec2 {
        // Normalize input to the same space used during fitting
        let pn = self.norm.normalize(p);

        // Affine component using dot product for linear terms
        let affine_coeffs_x = DVec2::new(self.affine_x[1], self.affine_x[2]);
        let affine_coeffs_y = DVec2::new(self.affine_y[1], self.affine_y[2]);
        let mut tx = self.affine_x[0] + affine_coeffs_x.dot(pn);
        let mut ty = self.affine_y[0] + affine_coeffs_y.dot(pn);

        // Radial basis function component
        for (i, &cp) in self.control_points.iter().enumerate() {
            let r = pn.distance(cp);
            let u = tps_kernel(r);
            tx += self.weights_x[i] * u;
            ty += self.weights_y[i] * u;
        }

        // Denormalize output back to pixel coordinates
        self.norm.denormalize(DVec2::new(tx, ty))
    }

    /// Transform multiple points efficiently.
    ///
    /// # Arguments
    /// * `points` - Source points to transform
    ///
    /// # Returns
    /// Vector of transformed points
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

    /// Get the number of control points.
    fn num_control_points(&self) -> usize {
        self.control_points.len()
    }

    /// Get the control points (in normalized coordinates, not pixel space).
    fn control_points(&self) -> &[DVec2] {
        &self.control_points
    }

    /// Compute the residuals at the control points.
    ///
    /// Returns the distance between the transformed source points
    /// and the original target points. With zero regularization,
    /// these should be very close to zero.
    fn compute_residuals(&self, target_points: &[DVec2]) -> Vec<f64> {
        self.control_points
            .iter()
            .zip(target_points.iter())
            .map(|(&pn, &tgt)| {
                // Evaluate TPS directly in normalized space (control points
                // are already normalized, skip denormalize→renormalize roundtrip)
                let affine_x = DVec2::new(self.affine_x[1], self.affine_x[2]);
                let affine_y = DVec2::new(self.affine_y[1], self.affine_y[2]);
                let mut tx = self.affine_x[0] + affine_x.dot(pn);
                let mut ty = self.affine_y[0] + affine_y.dot(pn);
                for (i, &cp) in self.control_points.iter().enumerate() {
                    let u = tps_kernel(pn.distance(cp));
                    tx += self.weights_x[i] * u;
                    ty += self.weights_y[i] * u;
                }
                let result = self.norm.denormalize(DVec2::new(tx, ty));
                result.distance(tgt)
            })
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
    // Guard against degenerate case (all points coincident)
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

/// Distortion map for visualizing local distortions.
///
/// This structure stores the distortion vectors at a grid of points,
/// useful for visualization and analysis.
#[derive(Debug, Clone)]
struct DistortionMap {
    /// Extent of the grid in grid points
    grid: Size2us,
    /// Grid spacing in pixels
    spacing: f64,
    /// Distortion vectors at each grid point
    vectors: Vec<DVec2>,
    /// Maximum distortion magnitude
    max_magnitude: f64,
    /// Mean distortion magnitude
    mean_magnitude: f64,
}

impl DistortionMap {
    /// Create a distortion map from a TPS model.
    ///
    /// # Arguments
    /// * `tps` - The thin-plate spline model
    /// * `image_width` - Image width in pixels
    /// * `image_height` - Image height in pixels
    /// * `grid_spacing` - Spacing between grid points
    fn from_tps(tps: &ThinPlateSpline, image: Size2us, grid_spacing: f64) -> Self {
        let grid = Size2us::new(
            (image.width as f64 / grid_spacing).ceil() as usize + 1,
            (image.height as f64 / grid_spacing).ceil() as usize + 1,
        );

        let mut vectors = Vec::with_capacity(grid.pixel_count());
        let mut max_magnitude = 0.0f64;
        let mut sum_magnitude = 0.0;

        for gy in 0..grid.height {
            for gx in 0..grid.width {
                let p = DVec2::new(gx as f64 * grid_spacing, gy as f64 * grid_spacing);
                let t = tps.transform(p);
                let d = t - p;
                let magnitude = d.length();

                vectors.push(d);
                max_magnitude = max_magnitude.max(magnitude);
                sum_magnitude += magnitude;
            }
        }

        let mean_magnitude = sum_magnitude / vectors.len() as f64;

        Self {
            grid,
            spacing: grid_spacing,
            vectors,
            max_magnitude,
            mean_magnitude,
        }
    }

    /// Get the distortion vector at a grid position.
    fn get(&self, point: Vec2us) -> Option<DVec2> {
        self.grid
            .contains(point)
            .then(|| self.vectors[self.grid.index_of(point)])
    }

    /// Interpolate the distortion at an arbitrary position.
    fn interpolate(&self, p: DVec2) -> DVec2 {
        let gx = p.x / self.spacing;
        let gy = p.y / self.spacing;

        let gx0 = gx.floor() as usize;
        let gy0 = gy.floor() as usize;
        let gx1 = (gx0 + 1).min(self.grid.width - 1);
        let gy1 = (gy0 + 1).min(self.grid.height - 1);

        let fx = gx - gx0 as f64;
        let fy = gy - gy0 as f64;

        let v00 = self.get(Vec2us::new(gx0, gy0)).unwrap_or(DVec2::ZERO);
        let v10 = self.get(Vec2us::new(gx1, gy0)).unwrap_or(DVec2::ZERO);
        let v01 = self.get(Vec2us::new(gx0, gy1)).unwrap_or(DVec2::ZERO);
        let v11 = self.get(Vec2us::new(gx1, gy1)).unwrap_or(DVec2::ZERO);

        // Bilinear interpolation
        (1.0 - fx) * (1.0 - fy) * v00
            + fx * (1.0 - fy) * v10
            + (1.0 - fx) * fy * v01
            + fx * fy * v11
    }
}

#[cfg(test)]
mod tests;
