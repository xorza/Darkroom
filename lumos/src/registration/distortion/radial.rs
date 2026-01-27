//! Radial distortion models for lens correction.
//!
//! This module provides parametric radial distortion models commonly used for
//! correcting barrel and pincushion distortion in optical systems.
//!
//! # Models
//!
//! ## Brown-Conrady Model
//!
//! The standard model for radial distortion (ISO 19795-5):
//!
//! ```text
//! x_d = x_u * (1 + k₁r² + k₂r⁴ + k₃r⁶)
//! y_d = y_u * (1 + k₁r² + k₂r⁴ + k₃r⁶)
//! ```
//!
//! where:
//! - (x_u, y_u) = undistorted coordinates relative to optical center
//! - (x_d, y_d) = distorted coordinates relative to optical center
//! - r² = x_u² + y_u²
//! - k₁, k₂, k₃ = radial distortion coefficients
//!
//! **Interpretation**:
//! - k₁ > 0: barrel distortion (edges bow outward)
//! - k₁ < 0: pincushion distortion (edges bow inward)
//!
//! # Usage
//!
//! ```ignore
//! use lumos::registration::distortion::{RadialDistortion, RadialDistortionConfig};
//!
//! // Create a model with known coefficients
//! let model = RadialDistortion::new(
//!     RadialDistortionConfig {
//!         k1: 0.001,  // Mild barrel distortion
//!         k2: 0.0,
//!         k3: 0.0,
//!         center: (1024.0, 768.0),  // Optical center
//!     }
//! );
//!
//! // Apply distortion (undistorted -> distorted)
//! let (x_d, y_d) = model.distort(512.0, 384.0);
//!
//! // Remove distortion (distorted -> undistorted)
//! let (x_u, y_u) = model.undistort(x_d, y_d);
//! ```

/// Configuration for radial distortion model.
#[derive(Debug, Clone, Copy)]
pub struct RadialDistortionConfig {
    /// First radial distortion coefficient (r² term).
    /// Positive = barrel distortion, negative = pincushion.
    pub k1: f64,
    /// Second radial distortion coefficient (r⁴ term).
    pub k2: f64,
    /// Third radial distortion coefficient (r⁶ term).
    pub k3: f64,
    /// Optical center (principal point) in pixel coordinates.
    pub center: (f64, f64),
}

impl Default for RadialDistortionConfig {
    fn default() -> Self {
        Self {
            k1: 0.0,
            k2: 0.0,
            k3: 0.0,
            center: (0.0, 0.0),
        }
    }
}

impl RadialDistortionConfig {
    /// Create a config for barrel distortion.
    ///
    /// # Arguments
    /// * `k1` - Positive coefficient for barrel distortion (typically 0.0001 to 0.01)
    /// * `center` - Optical center in pixel coordinates
    #[inline]
    pub fn barrel(k1: f64, center: (f64, f64)) -> Self {
        debug_assert!(k1 >= 0.0, "Barrel distortion requires k1 >= 0");
        Self {
            k1,
            k2: 0.0,
            k3: 0.0,
            center,
        }
    }

    /// Create a config for pincushion distortion.
    ///
    /// # Arguments
    /// * `k1` - Positive coefficient (will be negated internally)
    /// * `center` - Optical center in pixel coordinates
    #[inline]
    pub fn pincushion(k1: f64, center: (f64, f64)) -> Self {
        debug_assert!(k1 >= 0.0, "Pincushion distortion requires k1 >= 0");
        Self {
            k1: -k1,
            k2: 0.0,
            k3: 0.0,
            center,
        }
    }

    /// Create a config with all three radial coefficients.
    #[inline]
    pub fn with_coefficients(k1: f64, k2: f64, k3: f64, center: (f64, f64)) -> Self {
        Self { k1, k2, k3, center }
    }

    /// Create a config centered at the image center.
    #[inline]
    pub fn centered(width: u32, height: u32, k1: f64) -> Self {
        Self {
            k1,
            k2: 0.0,
            k3: 0.0,
            center: (width as f64 / 2.0, height as f64 / 2.0),
        }
    }
}

/// Radial distortion model using Brown-Conrady coefficients.
///
/// This model handles both barrel (k₁ > 0) and pincushion (k₁ < 0) distortion.
/// The model uses up to three radial coefficients (k₁, k₂, k₃) for accurate
/// modeling of complex lens distortion.
#[derive(Debug, Clone, Copy)]
pub struct RadialDistortion {
    /// Radial distortion coefficients
    k1: f64,
    k2: f64,
    k3: f64,
    /// Optical center (principal point)
    center_x: f64,
    center_y: f64,
}

impl RadialDistortion {
    /// Create a new radial distortion model.
    #[inline]
    pub fn new(config: RadialDistortionConfig) -> Self {
        Self {
            k1: config.k1,
            k2: config.k2,
            k3: config.k3,
            center_x: config.center.0,
            center_y: config.center.1,
        }
    }

    /// Create a model for barrel distortion (edges bow outward).
    ///
    /// # Arguments
    /// * `k1` - Distortion strength (typically 0.0001 to 0.01)
    /// * `center` - Optical center in pixel coordinates
    #[inline]
    pub fn barrel(k1: f64, center: (f64, f64)) -> Self {
        Self::new(RadialDistortionConfig::barrel(k1, center))
    }

    /// Create a model for pincushion distortion (edges bow inward).
    ///
    /// # Arguments
    /// * `k1` - Distortion strength (positive value, will be negated)
    /// * `center` - Optical center in pixel coordinates
    #[inline]
    pub fn pincushion(k1: f64, center: (f64, f64)) -> Self {
        Self::new(RadialDistortionConfig::pincushion(k1, center))
    }

    /// Create an identity model (no distortion).
    #[inline]
    pub fn identity() -> Self {
        Self {
            k1: 0.0,
            k2: 0.0,
            k3: 0.0,
            center_x: 0.0,
            center_y: 0.0,
        }
    }

    /// Get the radial distortion coefficients.
    #[inline]
    pub fn coefficients(&self) -> (f64, f64, f64) {
        (self.k1, self.k2, self.k3)
    }

    /// Get the optical center.
    #[inline]
    pub fn center(&self) -> (f64, f64) {
        (self.center_x, self.center_y)
    }

    /// Check if this is barrel distortion (k1 > 0).
    #[inline]
    pub fn is_barrel(&self) -> bool {
        self.k1 > 0.0
    }

    /// Check if this is pincushion distortion (k1 < 0).
    #[inline]
    pub fn is_pincushion(&self) -> bool {
        self.k1 < 0.0
    }

    /// Check if this is an identity (no distortion).
    #[inline]
    pub fn is_identity(&self) -> bool {
        self.k1.abs() < f64::EPSILON && self.k2.abs() < f64::EPSILON && self.k3.abs() < f64::EPSILON
    }

    /// Compute the distortion factor at a given radius.
    ///
    /// Returns `1 + k₁r² + k₂r⁴ + k₃r⁶`
    #[inline]
    fn distortion_factor(&self, r_squared: f64) -> f64 {
        let r4 = r_squared * r_squared;
        let r6 = r4 * r_squared;
        1.0 + self.k1 * r_squared + self.k2 * r4 + self.k3 * r6
    }

    /// Apply distortion: undistorted coordinates -> distorted coordinates.
    ///
    /// This is the forward model: given ideal (undistorted) pixel coordinates,
    /// compute where they appear in the distorted image.
    ///
    /// # Arguments
    /// * `x` - Undistorted x coordinate (pixel)
    /// * `y` - Undistorted y coordinate (pixel)
    ///
    /// # Returns
    /// Distorted (x, y) coordinates
    #[inline]
    pub fn distort(&self, x: f64, y: f64) -> (f64, f64) {
        let dx = x - self.center_x;
        let dy = y - self.center_y;
        let r_squared = dx * dx + dy * dy;

        let factor = self.distortion_factor(r_squared);

        let x_d = self.center_x + dx * factor;
        let y_d = self.center_y + dy * factor;

        (x_d, y_d)
    }

    /// Remove distortion: distorted coordinates -> undistorted coordinates.
    ///
    /// This is the inverse model: given measured (distorted) pixel coordinates,
    /// compute the ideal undistorted coordinates.
    ///
    /// Uses Newton-Raphson iteration for accurate inversion.
    ///
    /// # Arguments
    /// * `x` - Distorted x coordinate (pixel)
    /// * `y` - Distorted y coordinate (pixel)
    ///
    /// # Returns
    /// Undistorted (x, y) coordinates
    pub fn undistort(&self, x: f64, y: f64) -> (f64, f64) {
        // Fast path for identity
        if self.is_identity() {
            return (x, y);
        }

        let dx_d = x - self.center_x;
        let dy_d = y - self.center_y;
        let r_d_squared = dx_d * dx_d + dy_d * dy_d;

        // Fast path for center point
        if r_d_squared < f64::EPSILON {
            return (x, y);
        }

        // Newton-Raphson iteration to find r_u from r_d
        // We want: r_d = r_u * (1 + k1*r_u² + k2*r_u⁴ + k3*r_u⁶)
        // Let f(r_u) = r_u * factor(r_u) - r_d = 0
        // Start with initial guess r_u ≈ r_d
        let r_d = r_d_squared.sqrt();
        let mut r_u = r_d;

        for _ in 0..MAX_ITERATIONS {
            let r_u_squared = r_u * r_u;
            let factor = self.distortion_factor(r_u_squared);

            // Check for invalid distortion model (factor should be positive for valid model)
            if factor <= 0.0 {
                // Model is invalid at this radius - return best guess
                break;
            }

            let f = r_u * factor - r_d;

            // Derivative: d/dr_u [r_u * (1 + k1*r² + k2*r⁴ + k3*r⁶)]
            // = 1 + 3*k1*r² + 5*k2*r⁴ + 7*k3*r⁶
            let r4 = r_u_squared * r_u_squared;
            let r6 = r4 * r_u_squared;
            let df = 1.0 + 3.0 * self.k1 * r_u_squared + 5.0 * self.k2 * r4 + 7.0 * self.k3 * r6;

            // Guard against division by very small derivative
            if df.abs() < 1e-12 {
                break;
            }

            let delta = f / df;

            // Limit step size to prevent divergence
            let max_step = r_u * 0.5;
            let delta = delta.clamp(-max_step, max_step);

            r_u -= delta;

            // Ensure r_u stays positive
            if r_u <= 0.0 {
                r_u = r_d * 0.1; // Reset to small positive value
            }

            if delta.abs() < CONVERGENCE_THRESHOLD {
                break;
            }
        }

        // Compute undistorted coordinates using the ratio
        let scale = if r_d > f64::EPSILON { r_u / r_d } else { 1.0 };

        let x_u = self.center_x + dx_d * scale;
        let y_u = self.center_y + dy_d * scale;

        (x_u, y_u)
    }

    /// Apply distortion to multiple points.
    #[inline]
    pub fn distort_points(&self, points: &[(f64, f64)]) -> Vec<(f64, f64)> {
        points.iter().map(|&(x, y)| self.distort(x, y)).collect()
    }

    /// Remove distortion from multiple points.
    #[inline]
    pub fn undistort_points(&self, points: &[(f64, f64)]) -> Vec<(f64, f64)> {
        points.iter().map(|&(x, y)| self.undistort(x, y)).collect()
    }

    /// Compute the maximum distortion magnitude across an image.
    ///
    /// Samples corners and edges to find the maximum displacement.
    ///
    /// # Arguments
    /// * `width` - Image width in pixels
    /// * `height` - Image height in pixels
    ///
    /// # Returns
    /// Maximum distortion displacement in pixels
    pub fn max_distortion(&self, width: u32, height: u32) -> f64 {
        let w = width as f64;
        let h = height as f64;

        // Sample corners and midpoints of edges
        let test_points = [
            (0.0, 0.0),
            (w, 0.0),
            (0.0, h),
            (w, h),
            (w / 2.0, 0.0),
            (w / 2.0, h),
            (0.0, h / 2.0),
            (w, h / 2.0),
        ];

        test_points
            .iter()
            .map(|&(x, y)| {
                let (x_d, y_d) = self.distort(x, y);
                let dx = x_d - x;
                let dy = y_d - y;
                (dx * dx + dy * dy).sqrt()
            })
            .fold(0.0f64, f64::max)
    }

    /// Estimate distortion coefficients from matched point pairs.
    ///
    /// Given pairs of (undistorted, distorted) coordinates, estimates the
    /// radial distortion coefficients using least-squares fitting.
    ///
    /// # Arguments
    /// * `undistorted` - Ideal undistorted point coordinates
    /// * `distorted` - Measured distorted point coordinates
    /// * `center` - Optical center (or None to estimate from data mean)
    /// * `num_coefficients` - Number of coefficients to estimate (1-3)
    ///
    /// # Returns
    /// Estimated `RadialDistortion` model, or None if fitting fails
    pub fn estimate(
        undistorted: &[(f64, f64)],
        distorted: &[(f64, f64)],
        center: Option<(f64, f64)>,
        num_coefficients: usize,
    ) -> Option<Self> {
        if undistorted.len() != distorted.len() {
            return None;
        }
        if undistorted.len() < num_coefficients.max(3) {
            return None;
        }
        if num_coefficients == 0 || num_coefficients > 3 {
            return None;
        }

        // Determine center
        let (cx, cy) = center.unwrap_or_else(|| {
            let sum: (f64, f64) = undistorted
                .iter()
                .fold((0.0, 0.0), |acc, &(x, y)| (acc.0 + x, acc.1 + y));
            let n = undistorted.len() as f64;
            (sum.0 / n, sum.1 / n)
        });

        // Build the least-squares system
        // For each point: r_d = r_u * (1 + k1*r_u² + k2*r_u⁴ + k3*r_u⁶)
        // Rearranging: r_d - r_u = r_u * (k1*r_u² + k2*r_u⁴ + k3*r_u⁶)
        // This is linear in k1, k2, k3

        let n = undistorted.len();
        let mut ata = vec![vec![0.0; num_coefficients]; num_coefficients];
        let mut atb = vec![0.0; num_coefficients];

        for i in 0..n {
            let (x_u, y_u) = undistorted[i];
            let (x_d, y_d) = distorted[i];

            let dx_u = x_u - cx;
            let dy_u = y_u - cy;
            let dx_d = x_d - cx;
            let dy_d = y_d - cy;

            let r_u = (dx_u * dx_u + dy_u * dy_u).sqrt();
            let r_d = (dx_d * dx_d + dy_d * dy_d).sqrt();

            if r_u < f64::EPSILON {
                continue;
            }

            // Measurement: (r_d / r_u - 1)
            let measurement = r_d / r_u - 1.0;

            // Design matrix row: [r_u², r_u⁴, r_u⁶]
            let r2 = r_u * r_u;
            let r4 = r2 * r2;
            let r6 = r4 * r2;
            let design_row = [r2, r4, r6];

            // Accumulate A^T A and A^T b
            for j in 0..num_coefficients {
                for k in 0..num_coefficients {
                    ata[j][k] += design_row[j] * design_row[k];
                }
                atb[j] += design_row[j] * measurement;
            }
        }

        // Solve the normal equations
        let solution = solve_symmetric_positive_definite(&ata, &atb)?;

        let k1 = solution.first().copied().unwrap_or(0.0);
        let k2 = solution.get(1).copied().unwrap_or(0.0);
        let k3 = solution.get(2).copied().unwrap_or(0.0);

        Some(Self {
            k1,
            k2,
            k3,
            center_x: cx,
            center_y: cy,
        })
    }

    /// Compute RMS residual error for a set of point correspondences.
    ///
    /// # Arguments
    /// * `undistorted` - Ideal undistorted point coordinates
    /// * `distorted` - Measured distorted point coordinates
    ///
    /// # Returns
    /// RMS error in pixels
    pub fn rms_error(&self, undistorted: &[(f64, f64)], distorted: &[(f64, f64)]) -> f64 {
        debug_assert_eq!(
            undistorted.len(),
            distorted.len(),
            "Point arrays must have same length"
        );

        if undistorted.is_empty() {
            return 0.0;
        }

        let sum_sq: f64 = undistorted
            .iter()
            .zip(distorted.iter())
            .map(|(&(x_u, y_u), &(x_d, y_d))| {
                let (pred_x, pred_y) = self.distort(x_u, y_u);
                let dx = pred_x - x_d;
                let dy = pred_y - y_d;
                dx * dx + dy * dy
            })
            .sum();

        (sum_sq / undistorted.len() as f64).sqrt()
    }
}

/// Maximum iterations for Newton-Raphson undistortion.
const MAX_ITERATIONS: usize = 10;

/// Convergence threshold for Newton-Raphson iteration.
const CONVERGENCE_THRESHOLD: f64 = 1e-10;

/// Solve a symmetric positive definite system using Cholesky decomposition.
///
/// Solves Ax = b where A is symmetric positive definite.
#[allow(clippy::needless_range_loop)] // Index-based loops clearer for matrix operations
fn solve_symmetric_positive_definite(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    if n == 0 || a.len() != n {
        return None;
    }

    // Cholesky decomposition: A = L * L^T
    let mut l = vec![vec![0.0; n]; n];

    for i in 0..n {
        for j in 0..=i {
            let mut sum = 0.0;
            if i == j {
                for k in 0..j {
                    sum += l[j][k] * l[j][k];
                }
                let diag = a[j][j] - sum;
                if diag <= 0.0 {
                    return None; // Not positive definite
                }
                l[j][j] = diag.sqrt();
            } else {
                for k in 0..j {
                    sum += l[i][k] * l[j][k];
                }
                if l[j][j].abs() < 1e-15 {
                    return None;
                }
                l[i][j] = (a[i][j] - sum) / l[j][j];
            }
        }
    }

    // Forward substitution: L * y = b
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut sum = 0.0;
        for j in 0..i {
            sum += l[i][j] * y[j];
        }
        if l[i][i].abs() < 1e-15 {
            return None;
        }
        y[i] = (b[i] - sum) / l[i][i];
    }

    // Backward substitution: L^T * x = y
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = 0.0;
        for j in (i + 1)..n {
            sum += l[j][i] * x[j];
        }
        if l[i][i].abs() < 1e-15 {
            return None;
        }
        x[i] = (y[i] - sum) / l[i][i];
    }

    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOLERANCE: f64 = 1e-6;

    #[test]
    fn test_identity_distortion() {
        let model = RadialDistortion::identity();

        assert!(model.is_identity());
        assert!(!model.is_barrel());
        assert!(!model.is_pincushion());

        let (x_d, y_d) = model.distort(100.0, 200.0);
        assert!((x_d - 100.0).abs() < TOLERANCE);
        assert!((y_d - 200.0).abs() < TOLERANCE);

        let (x_u, y_u) = model.undistort(100.0, 200.0);
        assert!((x_u - 100.0).abs() < TOLERANCE);
        assert!((y_u - 200.0).abs() < TOLERANCE);
    }

    #[test]
    fn test_barrel_distortion() {
        let center = (512.0, 512.0);
        let model = RadialDistortion::barrel(0.0001, center);

        assert!(model.is_barrel());
        assert!(!model.is_pincushion());
        assert!(!model.is_identity());

        // Point at the corner should be pushed outward
        let (x_d, y_d) = model.distort(0.0, 0.0);

        // For barrel distortion, the distorted point should be farther from center
        let r_orig = ((0.0 - center.0).powi(2) + (0.0 - center.1).powi(2)).sqrt();
        let r_dist = ((x_d - center.0).powi(2) + (y_d - center.1).powi(2)).sqrt();

        assert!(
            r_dist > r_orig,
            "Barrel distortion should push corners outward: {} <= {}",
            r_dist,
            r_orig
        );
    }

    #[test]
    fn test_pincushion_distortion() {
        let center = (512.0, 512.0);
        // Use a small coefficient suitable for the image size
        // For a 1024x1024 image, corner distance is ~724 pixels
        // r² at corner ≈ 524288, so k1 = 0.000001 gives factor ≈ 0.48 (valid)
        let model = RadialDistortion::pincushion(0.0000005, center);

        assert!(!model.is_barrel());
        assert!(model.is_pincushion());
        assert!(!model.is_identity());

        // Point at the corner should be pulled inward
        let (x_d, y_d) = model.distort(0.0, 0.0);

        // For pincushion distortion, the distorted point should be closer to center
        let r_orig = ((0.0 - center.0).powi(2) + (0.0 - center.1).powi(2)).sqrt();
        let r_dist = ((x_d - center.0).powi(2) + (y_d - center.1).powi(2)).sqrt();

        assert!(
            r_dist < r_orig,
            "Pincushion distortion should pull corners inward: {} >= {}",
            r_dist,
            r_orig
        );
    }

    #[test]
    fn test_distort_undistort_roundtrip() {
        // Use realistic coefficients for camera-sized images
        // For 1000x1000 image with center at (500, 400), max r² ≈ 410000
        // k1 * r² should give factor change of ~0.01-0.1
        // So k1 ≈ 0.1 / 410000 ≈ 2.4e-7, but let's use smaller for safety
        let model = RadialDistortion::new(RadialDistortionConfig {
            k1: 1e-8,   // ~0.4% distortion at corners
            k2: -1e-16, // negligible higher order
            k3: 1e-24,  // negligible higher order
            center: (500.0, 400.0),
        });

        let test_points = [
            (100.0, 100.0),
            (500.0, 400.0), // center - should be unchanged
            (900.0, 700.0),
            (250.0, 600.0),
            (750.0, 200.0),
        ];

        for &(x, y) in &test_points {
            let (x_d, y_d) = model.distort(x, y);
            let (x_u, y_u) = model.undistort(x_d, y_d);

            assert!(
                (x_u - x).abs() < 1e-5,
                "Roundtrip failed for ({}, {}): got ({}, {})",
                x,
                y,
                x_u,
                y_u
            );
            assert!(
                (y_u - y).abs() < 1e-5,
                "Roundtrip failed for ({}, {}): got ({}, {})",
                x,
                y,
                x_u,
                y_u
            );
        }
    }

    #[test]
    fn test_center_point_unchanged() {
        let center = (512.0, 384.0);
        let model = RadialDistortion::barrel(0.001, center);

        let (x_d, y_d) = model.distort(center.0, center.1);
        assert!((x_d - center.0).abs() < TOLERANCE);
        assert!((y_d - center.1).abs() < TOLERANCE);

        let (x_u, y_u) = model.undistort(center.0, center.1);
        assert!((x_u - center.0).abs() < TOLERANCE);
        assert!((y_u - center.1).abs() < TOLERANCE);
    }

    #[test]
    fn test_radial_symmetry() {
        let center = (500.0, 500.0);
        let model = RadialDistortion::barrel(0.0001, center);

        // Points equidistant from center should have same magnitude of distortion
        let r = 200.0;
        let angles: [f64; 8] = [0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0];

        let mut distortions = Vec::new();

        for &angle in &angles {
            let rad = angle.to_radians();
            let x = center.0 + r * rad.cos();
            let y = center.1 + r * rad.sin();

            let (x_d, y_d) = model.distort(x, y);
            let distortion = ((x_d - x).powi(2) + (y_d - y).powi(2)).sqrt();
            distortions.push(distortion);
        }

        // All distortions should be equal (within tolerance)
        let first = distortions[0];
        for (i, &d) in distortions.iter().enumerate() {
            assert!(
                (d - first).abs() < 1e-6,
                "Distortion at angle {} differs: {} vs {}",
                angles[i],
                d,
                first
            );
        }
    }

    #[test]
    fn test_distortion_increases_with_radius() {
        let center = (500.0, 500.0);
        let model = RadialDistortion::barrel(0.0001, center);

        let mut prev_distortion = 0.0;

        for r in [100.0, 200.0, 300.0, 400.0, 500.0] {
            let x = center.0 + r;
            let y = center.1;

            let (x_d, _) = model.distort(x, y);
            let distortion = (x_d - x).abs();

            assert!(
                distortion > prev_distortion,
                "Distortion should increase with radius: {} <= {} at r={}",
                distortion,
                prev_distortion,
                r
            );
            prev_distortion = distortion;
        }
    }

    #[test]
    fn test_config_constructors() {
        let config1 = RadialDistortionConfig::barrel(0.001, (100.0, 100.0));
        assert!(config1.k1 > 0.0);
        assert_eq!(config1.k2, 0.0);
        assert_eq!(config1.k3, 0.0);

        let config2 = RadialDistortionConfig::pincushion(0.001, (100.0, 100.0));
        assert!(config2.k1 < 0.0);

        let config3 = RadialDistortionConfig::centered(1024, 768, 0.0005);
        assert_eq!(config3.center, (512.0, 384.0));
    }

    #[test]
    fn test_distort_points_batch() {
        let model = RadialDistortion::barrel(0.0001, (500.0, 500.0));
        let points = vec![(100.0, 100.0), (200.0, 200.0), (300.0, 300.0)];

        let distorted = model.distort_points(&points);

        assert_eq!(distorted.len(), points.len());

        for (i, &(x, y)) in points.iter().enumerate() {
            let (x_d, y_d) = model.distort(x, y);
            assert!((distorted[i].0 - x_d).abs() < TOLERANCE);
            assert!((distorted[i].1 - y_d).abs() < TOLERANCE);
        }
    }

    #[test]
    fn test_undistort_points_batch() {
        let model = RadialDistortion::barrel(0.0001, (500.0, 500.0));
        let points = vec![(100.0, 100.0), (200.0, 200.0), (300.0, 300.0)];

        let undistorted = model.undistort_points(&points);

        assert_eq!(undistorted.len(), points.len());

        for (i, &(x, y)) in points.iter().enumerate() {
            let (x_u, y_u) = model.undistort(x, y);
            assert!((undistorted[i].0 - x_u).abs() < TOLERANCE);
            assert!((undistorted[i].1 - y_u).abs() < TOLERANCE);
        }
    }

    #[test]
    fn test_max_distortion() {
        let model = RadialDistortion::barrel(0.0001, (512.0, 512.0));
        let max = model.max_distortion(1024, 1024);

        // Max distortion should be at corners
        let (x_d, y_d) = model.distort(0.0, 0.0);
        let corner_distortion = ((x_d - 0.0).powi(2) + (y_d - 0.0).powi(2)).sqrt();

        assert!(
            (max - corner_distortion).abs() < 1e-6,
            "Max distortion {} should match corner distortion {}",
            max,
            corner_distortion
        );
    }

    #[test]
    fn test_estimate_barrel_distortion() {
        let center = (500.0, 500.0);
        let k1_true = 0.0001;
        let model_true = RadialDistortion::barrel(k1_true, center);

        // Generate synthetic data
        let mut undistorted = Vec::new();
        let mut distorted = Vec::new();

        for y in (0..=1000).step_by(100) {
            for x in (0..=1000).step_by(100) {
                let xu = x as f64;
                let yu = y as f64;
                undistorted.push((xu, yu));
                distorted.push(model_true.distort(xu, yu));
            }
        }

        // Estimate the model
        let model_est = RadialDistortion::estimate(&undistorted, &distorted, Some(center), 1);

        assert!(model_est.is_some(), "Estimation should succeed");
        let model_est = model_est.unwrap();

        let (k1_est, _, _) = model_est.coefficients();
        assert!(
            (k1_est - k1_true).abs() < 1e-6,
            "Estimated k1={} should match true k1={}",
            k1_est,
            k1_true
        );
    }

    #[test]
    fn test_estimate_with_multiple_coefficients() {
        let center = (500.0, 500.0);
        // Use realistic coefficients for a 1000x1000 image
        let model_true = RadialDistortion::new(RadialDistortionConfig {
            k1: 0.0000001,
            k2: -0.000000000001,
            k3: 0.0,
            center,
        });

        // Generate synthetic data
        let mut undistorted = Vec::new();
        let mut distorted = Vec::new();

        for y in (0..=1000).step_by(50) {
            for x in (0..=1000).step_by(50) {
                let xu = x as f64;
                let yu = y as f64;
                undistorted.push((xu, yu));
                distorted.push(model_true.distort(xu, yu));
            }
        }

        // Estimate with 2 coefficients
        let model_est = RadialDistortion::estimate(&undistorted, &distorted, Some(center), 2);

        assert!(model_est.is_some(), "Estimation should succeed");
        let model_est = model_est.unwrap();

        // Check RMS error is small
        let rms = model_est.rms_error(&undistorted, &distorted);
        assert!(rms < 0.01, "RMS error {} should be small", rms);
    }

    #[test]
    fn test_rms_error() {
        let model = RadialDistortion::barrel(0.0001, (500.0, 500.0));

        // Perfect fit should have zero error
        let undistorted = vec![(100.0, 100.0), (200.0, 200.0), (300.0, 300.0)];
        let distorted: Vec<_> = undistorted
            .iter()
            .map(|&(x, y)| model.distort(x, y))
            .collect();

        let rms = model.rms_error(&undistorted, &distorted);
        assert!(rms < TOLERANCE, "Perfect fit should have zero RMS: {}", rms);
    }

    #[test]
    fn test_coefficients_getter() {
        let config = RadialDistortionConfig {
            k1: 0.001,
            k2: -0.0001,
            k3: 0.00001,
            center: (100.0, 100.0),
        };
        let model = RadialDistortion::new(config);

        let (k1, k2, k3) = model.coefficients();
        assert_eq!(k1, 0.001);
        assert_eq!(k2, -0.0001);
        assert_eq!(k3, 0.00001);

        let (cx, cy) = model.center();
        assert_eq!(cx, 100.0);
        assert_eq!(cy, 100.0);
    }

    #[test]
    fn test_strong_distortion_undistort_converges() {
        // Test with moderately strong distortion to ensure Newton-Raphson converges
        // For 1000x1000 image, corner distance is ~707 pixels, r² ≈ 500000
        // k1 = 0.000001 gives factor ≈ 1.5 at corners (significant but valid)
        let model = RadialDistortion::new(RadialDistortionConfig {
            k1: 0.000001, // Significant distortion
            k2: 0.0,
            k3: 0.0,
            center: (500.0, 500.0),
        });

        let test_points = [(0.0, 0.0), (100.0, 100.0), (900.0, 900.0), (1000.0, 1000.0)];

        for &(x, y) in &test_points {
            let (x_d, y_d) = model.distort(x, y);
            let (x_u, y_u) = model.undistort(x_d, y_d);

            assert!(
                (x_u - x).abs() < 0.01,
                "Strong distortion roundtrip failed for ({}, {}): got ({}, {})",
                x,
                y,
                x_u,
                y_u
            );
            assert!(
                (y_u - y).abs() < 0.01,
                "Strong distortion roundtrip failed for ({}, {}): got ({}, {})",
                x,
                y,
                x_u,
                y_u
            );
        }
    }

    #[test]
    fn test_negative_k2_mustache_distortion() {
        // k1 > 0, k2 < 0 creates "mustache" or "complex" distortion
        // where barrel transitions to pincushion at larger radii
        // For 1000x1000 image, corner r² ≈ 500000
        // Use coefficients that give reasonable distortion (<10% at corners)
        let center = (500.0, 500.0);
        let model = RadialDistortion::new(RadialDistortionConfig {
            k1: 1e-8,   // ~0.5% at corners
            k2: -1e-16, // small higher-order correction
            k3: 0.0,
            center,
        });

        // Roundtrip should still work
        let (x_d, y_d) = model.distort(100.0, 100.0);
        let (x_u, y_u) = model.undistort(x_d, y_d);

        assert!((x_u - 100.0).abs() < 0.01);
        assert!((y_u - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_estimate_fails_with_insufficient_points() {
        let undistorted = vec![(100.0, 100.0), (200.0, 200.0)];
        let distorted = vec![(101.0, 101.0), (202.0, 202.0)];

        // Not enough points for estimation
        let result = RadialDistortion::estimate(&undistorted, &distorted, None, 1);
        assert!(result.is_none(), "Should fail with insufficient points");
    }

    #[test]
    fn test_estimate_fails_with_mismatched_lengths() {
        let undistorted = vec![(100.0, 100.0), (200.0, 200.0), (300.0, 300.0)];
        let distorted = vec![(101.0, 101.0)];

        let result = RadialDistortion::estimate(&undistorted, &distorted, None, 1);
        assert!(result.is_none(), "Should fail with mismatched lengths");
    }

    #[test]
    fn test_estimate_fails_with_invalid_num_coefficients() {
        let undistorted = vec![(100.0, 100.0), (200.0, 200.0), (300.0, 300.0)];
        let distorted = vec![(101.0, 101.0), (202.0, 202.0), (303.0, 303.0)];

        let result = RadialDistortion::estimate(&undistorted, &distorted, None, 0);
        assert!(result.is_none(), "Should fail with 0 coefficients");

        let result = RadialDistortion::estimate(&undistorted, &distorted, None, 4);
        assert!(result.is_none(), "Should fail with 4 coefficients");
    }
}
