//! Distortion modeling for optical corrections.
//!
//! This module provides parametric distortion models (radial, tangential, and field
//! curvature) and non-parametric thin-plate spline (TPS) interpolation for correcting
//! optical distortions in astronomical images.
//!
//! # Distortion Types
//!
//! ## Radial Distortion (Parametric)
//!
//! The Brown-Conrady model handles barrel and pincushion distortion:
//!
//! ```text
//! r' = r(1 + k₁r² + k₂r⁴ + k₃r⁶)
//! ```
//!
//! - **Barrel distortion** (k₁ > 0): Edges bow outward, common in wide-angle lenses
//! - **Pincushion distortion** (k₁ < 0): Edges bow inward, common in telephoto lenses
//!
//! Use `RadialDistortion` when:
//! - Lens distortion coefficients are known (from calibration)
//! - The distortion pattern is radially symmetric around the optical center
//! - You need fast forward/inverse transformations
//!
//! ## Tangential Distortion (Parametric)
//!
//! Tangential (decentering) distortion occurs when the lens is not perfectly aligned
//! with the image sensor. The Brown-Conrady model:
//!
//! ```text
//! x' = x + [2p₁xy + p₂(r² + 2x²)]
//! y' = y + [p₁(r² + 2y²) + 2p₂xy]
//! ```
//!
//! - **p₁**: Vertical decentering - causes y-dependent shift in x
//! - **p₂**: Horizontal decentering - causes x-dependent shift in y
//!
//! Use `TangentialDistortion` when:
//! - Lens-sensor alignment is imperfect
//! - Distortion pattern is asymmetric
//! - Often combined with radial distortion for complete lens modeling
//!
//! ## Field Curvature (Parametric)
//!
//! Field curvature (Petzval curvature) occurs when the focal plane is curved rather
//! than flat. The model corrects for the radial scaling effect:
//!
//! ```text
//! r' = r × (1 + c₁r² + c₂r⁴)
//! ```
//!
//! - **c₁ > 0**: Outward curvature (magnification increases with radius)
//! - **c₁ < 0**: Inward curvature (magnification decreases with radius)
//!
//! Use `FieldCurvature` when:
//! - Stars appear defocused at field edges but sharp at center (or vice versa)
//! - Using fast optical systems with uncorrected Petzval sum
//! - Combined with radial distortion for complete optical modeling
//!
//! ## Thin-Plate Spline (Non-Parametric)
//!
//! TPS provides smooth interpolation that minimizes "bending energy":
//!
//! ```text
//! f(x,y) = a₀ + a₁x + a₂y + Σᵢ wᵢ U(||(x,y) - (xᵢ,yᵢ)||)
//! ```
//!
//! where U(r) = r² log(r) is the TPS radial basis function.
//!
//! Use `ThinPlateSpline` when:
//! - Distortion pattern is non-radial or non-uniform
//! - You have matched star pairs but no calibration data
//! - Complex field distortions from multiple optical elements
//!
//! # Usage Examples
//!
//! ## Radial Distortion
//!
//! ```ignore
//! use lumos::registration::distortion::{RadialDistortion, RadialDistortionConfig};
//!
//! // Create model with known coefficients
//! let model = RadialDistortion::barrel(0.0001, (512.0, 384.0));
//!
//! // Correct a point (remove distortion)
//! let (x_u, y_u) = model.undistort(x_distorted, y_distorted);
//!
//! // Or estimate coefficients from star matches
//! let model = RadialDistortion::estimate(&undistorted, &distorted, None, 1)?;
//! ```
//!
//! ## Field Curvature
//!
//! ```ignore
//! use lumos::registration::distortion::{FieldCurvature, FieldCurvatureConfig};
//!
//! // Create model with known curvature coefficient
//! let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.00001, (512.0, 384.0)));
//!
//! // Correct field curvature (curved -> flat)
//! let (x_flat, y_flat) = model.correct(x_curved, y_curved);
//!
//! // Or estimate from star matches
//! let model = FieldCurvature::estimate(&ideal, &curved, None, 1)?;
//! ```
//!
//! ## Thin-Plate Spline
//!
//! ```ignore
//! use lumos::registration::distortion::{ThinPlateSpline, TpsConfig};
//!
//! // Control points from star matching
//! let source_points = vec![(100.0, 100.0), (200.0, 150.0), ...];
//! let target_points = vec![(101.2, 99.8), (201.5, 150.3), ...];
//!
//! // Build TPS model
//! let tps = ThinPlateSpline::fit(&source_points, &target_points, TpsConfig::default())?;
//!
//! // Transform a point
//! let (tx, ty) = tps.transform(150.0, 125.0);
//! ```

mod field_curvature;
mod radial;
mod tangential;

#[cfg(test)]
mod tests;

pub use field_curvature::{FieldCurvature, FieldCurvatureConfig};
pub use radial::{RadialDistortion, RadialDistortionConfig};
pub use tangential::{TangentialDistortion, TangentialDistortionConfig};

use glam::DVec2;

/// Configuration for thin-plate spline fitting.
#[derive(Debug, Clone)]
pub struct TpsConfig {
    /// Regularization parameter (lambda). Higher values produce smoother
    /// interpolation but may not pass exactly through control points.
    /// Default: 0.0 (exact interpolation)
    pub regularization: f64,
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
pub struct ThinPlateSpline {
    /// Control points (source positions)
    control_points: Vec<DVec2>,
    /// Weights for the radial basis functions (x-direction)
    weights_x: Vec<f64>,
    /// Weights for the radial basis functions (y-direction)
    weights_y: Vec<f64>,
    /// Affine coefficients for x: a0 + a1*x + a2*y
    affine_x: [f64; 3],
    /// Affine coefficients for y: b0 + b1*x + b2*y
    affine_y: [f64; 3],
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
    pub fn fit(
        source_points: &[DVec2],
        target_points: &[DVec2],
        config: TpsConfig,
    ) -> Option<Self> {
        let n = source_points.len();
        if n < 3 {
            return None; // Need at least 3 points for TPS
        }

        if source_points.len() != target_points.len() {
            return None;
        }

        // Build the TPS system matrix
        // The system has the form:
        // [K + λI  P] [w]   [v]
        // [P^T     0] [a] = [0]
        //
        // where K[i,j] = U(||p_i - p_j||)
        //       P[i,:] = [1, x_i, y_i]
        //       w = weights for RBF
        //       a = affine coefficients

        let matrix_size = n + 3;
        let mut matrix = vec![vec![0.0; matrix_size]; matrix_size];

        // Fill K matrix (upper-left n×n block)
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    matrix[i][j] = config.regularization;
                } else {
                    let r = source_points[i].distance(source_points[j]);
                    matrix[i][j] = tps_kernel(r);
                }
            }
        }

        // Fill P matrix (upper-right n×3 block) and P^T (lower-left 3×n block)
        for i in 0..n {
            let p = source_points[i];
            matrix[i][n] = 1.0;
            matrix[i][n + 1] = p.x;
            matrix[i][n + 2] = p.y;

            matrix[n][i] = 1.0;
            matrix[n + 1][i] = p.x;
            matrix[n + 2][i] = p.y;
        }

        // Lower-right 3×3 block is zeros (already initialized)

        // Right-hand side vectors
        let mut rhs_x = vec![0.0; matrix_size];
        let mut rhs_y = vec![0.0; matrix_size];

        for i in 0..n {
            rhs_x[i] = target_points[i].x;
            rhs_y[i] = target_points[i].y;
        }

        // Solve the system using LU decomposition
        let solution_x = solve_linear_system(&matrix, &rhs_x)?;
        let solution_y = solve_linear_system(&matrix, &rhs_y)?;

        // Extract weights and affine coefficients
        let weights_x: Vec<f64> = solution_x[..n].to_vec();
        let weights_y: Vec<f64> = solution_y[..n].to_vec();

        let affine_x = [solution_x[n], solution_x[n + 1], solution_x[n + 2]];
        let affine_y = [solution_y[n], solution_y[n + 1], solution_y[n + 2]];

        Some(Self {
            control_points: source_points.to_vec(),
            weights_x,
            weights_y,
            affine_x,
            affine_y,
        })
    }

    /// Transform a point using the fitted TPS model.
    ///
    /// # Arguments
    /// * `p` - Source point coordinates
    ///
    /// # Returns
    /// Transformed coordinates
    pub fn transform(&self, p: DVec2) -> DVec2 {
        // Affine component using dot product for linear terms
        let affine_coeffs_x = DVec2::new(self.affine_x[1], self.affine_x[2]);
        let affine_coeffs_y = DVec2::new(self.affine_y[1], self.affine_y[2]);
        let mut tx = self.affine_x[0] + affine_coeffs_x.dot(p);
        let mut ty = self.affine_y[0] + affine_coeffs_y.dot(p);

        // Radial basis function component
        for (i, &cp) in self.control_points.iter().enumerate() {
            let r = p.distance(cp);
            let u = tps_kernel(r);
            tx += self.weights_x[i] * u;
            ty += self.weights_y[i] * u;
        }

        DVec2::new(tx, ty)
    }

    /// Transform multiple points efficiently.
    ///
    /// # Arguments
    /// * `points` - Source points to transform
    ///
    /// # Returns
    /// Vector of transformed points
    pub fn transform_points(&self, points: &[DVec2]) -> Vec<DVec2> {
        points.iter().map(|&p| self.transform(p)).collect()
    }

    /// Compute the bending energy of the spline.
    ///
    /// Lower values indicate smoother interpolation. This is useful for
    /// comparing different TPS fits or for choosing regularization parameters.
    pub fn bending_energy(&self) -> f64 {
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
    pub fn num_control_points(&self) -> usize {
        self.control_points.len()
    }

    /// Get the control points.
    pub fn control_points(&self) -> &[DVec2] {
        &self.control_points
    }

    /// Compute the residuals at the control points.
    ///
    /// Returns the distance between the transformed source points
    /// and the original target points. With zero regularization,
    /// these should be very close to zero.
    pub fn compute_residuals(&self, target_points: &[DVec2]) -> Vec<f64> {
        self.control_points
            .iter()
            .zip(target_points.iter())
            .map(|(&src, &tgt)| self.transform(src).distance(tgt))
            .collect()
    }
}

/// TPS radial basis function: U(r) = r² log(r)
///
/// For r = 0, we define U(0) = 0 (the limit as r → 0).
#[inline]
fn tps_kernel(r: f64) -> f64 {
    if r < 1e-10 { 0.0 } else { r * r * r.ln() }
}

/// Solve a linear system Ax = b using LU decomposition with partial pivoting.
#[allow(clippy::needless_range_loop)]
fn solve_linear_system(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    if a.len() != n || a.iter().any(|row| row.len() != n) {
        return None;
    }

    // Create augmented matrix
    let mut aug: Vec<Vec<f64>> = a
        .iter()
        .zip(b.iter())
        .map(|(row, &bi)| {
            let mut new_row = row.clone();
            new_row.push(bi);
            new_row
        })
        .collect();

    // Forward elimination with partial pivoting
    for col in 0..n {
        // Find pivot
        let mut max_row = col;
        let mut max_val = aug[col][col].abs();
        for row in (col + 1)..n {
            let val = aug[row][col].abs();
            if val > max_val {
                max_val = val;
                max_row = row;
            }
        }

        if max_val < 1e-12 {
            return None; // Singular matrix
        }

        // Swap rows
        if max_row != col {
            aug.swap(col, max_row);
        }

        // Eliminate
        for row in (col + 1)..n {
            let factor = aug[row][col] / aug[col][col];
            for j in col..=n {
                aug[row][j] -= factor * aug[col][j];
            }
        }
    }

    // Back substitution
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        x[i] = aug[i][n];
        for j in (i + 1)..n {
            x[i] -= aug[i][j] * x[j];
        }
        x[i] /= aug[i][i];
    }

    Some(x)
}

/// Distortion map for visualizing local distortions.
///
/// This structure stores the distortion vectors at a grid of points,
/// useful for visualization and analysis.
#[derive(Debug, Clone)]
pub struct DistortionMap {
    /// Width of the grid
    pub width: usize,
    /// Height of the grid
    pub height: usize,
    /// Grid spacing in pixels
    pub spacing: f64,
    /// Distortion vectors at each grid point
    pub vectors: Vec<DVec2>,
    /// Maximum distortion magnitude
    pub max_magnitude: f64,
    /// Mean distortion magnitude
    pub mean_magnitude: f64,
}

impl DistortionMap {
    /// Create a distortion map from a TPS model.
    ///
    /// # Arguments
    /// * `tps` - The thin-plate spline model
    /// * `image_width` - Image width in pixels
    /// * `image_height` - Image height in pixels
    /// * `grid_spacing` - Spacing between grid points
    pub fn from_tps(
        tps: &ThinPlateSpline,
        image_width: usize,
        image_height: usize,
        grid_spacing: f64,
    ) -> Self {
        let grid_width = (image_width as f64 / grid_spacing).ceil() as usize + 1;
        let grid_height = (image_height as f64 / grid_spacing).ceil() as usize + 1;

        let mut vectors = Vec::with_capacity(grid_width * grid_height);
        let mut max_magnitude = 0.0f64;
        let mut sum_magnitude = 0.0;

        for gy in 0..grid_height {
            for gx in 0..grid_width {
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
            width: grid_width,
            height: grid_height,
            spacing: grid_spacing,
            vectors,
            max_magnitude,
            mean_magnitude,
        }
    }

    /// Get the distortion vector at a grid position.
    pub fn get(&self, gx: usize, gy: usize) -> Option<DVec2> {
        if gx < self.width && gy < self.height {
            Some(self.vectors[gy * self.width + gx])
        } else {
            None
        }
    }

    /// Interpolate the distortion at an arbitrary position.
    pub fn interpolate(&self, p: DVec2) -> DVec2 {
        let gx = p.x / self.spacing;
        let gy = p.y / self.spacing;

        let gx0 = gx.floor() as usize;
        let gy0 = gy.floor() as usize;
        let gx1 = (gx0 + 1).min(self.width - 1);
        let gy1 = (gy0 + 1).min(self.height - 1);

        let fx = gx - gx0 as f64;
        let fy = gy - gy0 as f64;

        let v00 = self.get(gx0, gy0).unwrap_or(DVec2::ZERO);
        let v10 = self.get(gx1, gy0).unwrap_or(DVec2::ZERO);
        let v01 = self.get(gx0, gy1).unwrap_or(DVec2::ZERO);
        let v11 = self.get(gx1, gy1).unwrap_or(DVec2::ZERO);

        // Bilinear interpolation
        (1.0 - fx) * (1.0 - fy) * v00
            + fx * (1.0 - fy) * v10
            + (1.0 - fx) * fy * v01
            + fx * fy * v11
    }
}
