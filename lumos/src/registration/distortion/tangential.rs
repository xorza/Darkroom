//! Tangential distortion models for lens correction.
//!
//! This module provides parametric tangential distortion models commonly used for
//! correcting decentering distortion caused by lens misalignment with the image sensor.
//!
//! # Models
//!
//! ## Brown-Conrady Tangential Model
//!
//! Tangential distortion occurs when the lens is not perfectly aligned parallel to
//! the imaging plane. The standard model (following OpenCV conventions):
//!
//! ```text
//! x_d = x_u + [2*p₁*x*y + p₂*(r² + 2*x²)]
//! y_d = y_u + [p₁*(r² + 2*y²) + 2*p₂*x*y]
//! ```
//!
//! where:
//! - (x_u, y_u) = undistorted coordinates relative to optical center
//! - (x_d, y_d) = distorted coordinates relative to optical center
//! - r² = x_u² + y_u²
//! - p₁, p₂ = tangential distortion coefficients
//!
//! **Physical interpretation**:
//! - p₁ affects vertical decentering (causes y-dependent shift in x)
//! - p₂ affects horizontal decentering (causes x-dependent shift in y)
//!
//! # Usage
//!
//! ```ignore
//! use lumos::registration::distortion::{TangentialDistortion, TangentialDistortionConfig};
//!
//! // Create a model with known coefficients
//! let model = TangentialDistortion::new(
//!     TangentialDistortionConfig {
//!         p1: 0.0001,  // Vertical decentering
//!         p2: -0.00005, // Horizontal decentering
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

/// Configuration for tangential distortion model.
#[derive(Debug, Clone, Copy)]
pub struct TangentialDistortionConfig {
    /// First tangential distortion coefficient (p₁).
    /// Affects vertical decentering - causes y-dependent shift in x.
    pub p1: f64,
    /// Second tangential distortion coefficient (p₂).
    /// Affects horizontal decentering - causes x-dependent shift in y.
    pub p2: f64,
    /// Optical center (principal point) in pixel coordinates.
    pub center: (f64, f64),
}

impl Default for TangentialDistortionConfig {
    fn default() -> Self {
        Self {
            p1: 0.0,
            p2: 0.0,
            center: (0.0, 0.0),
        }
    }
}

impl TangentialDistortionConfig {
    /// Create a config with specified coefficients.
    ///
    /// # Arguments
    /// * `p1` - Vertical decentering coefficient
    /// * `p2` - Horizontal decentering coefficient
    /// * `center` - Optical center in pixel coordinates
    #[inline]
    pub fn with_coefficients(p1: f64, p2: f64, center: (f64, f64)) -> Self {
        Self { p1, p2, center }
    }

    /// Create a config centered at the image center.
    #[inline]
    pub fn centered(width: u32, height: u32, p1: f64, p2: f64) -> Self {
        Self {
            p1,
            p2,
            center: (width as f64 / 2.0, height as f64 / 2.0),
        }
    }
}

/// Tangential distortion model using Brown-Conrady coefficients.
///
/// This model handles decentering distortion caused by misalignment between
/// the lens and the image sensor plane. It uses two coefficients (p₁, p₂)
/// following the OpenCV convention.
#[derive(Debug, Clone, Copy)]
pub struct TangentialDistortion {
    /// Tangential distortion coefficient p₁ (vertical decentering)
    p1: f64,
    /// Tangential distortion coefficient p₂ (horizontal decentering)
    p2: f64,
    /// Optical center (principal point)
    center_x: f64,
    center_y: f64,
}

impl TangentialDistortion {
    /// Create a new tangential distortion model.
    #[inline]
    pub fn new(config: TangentialDistortionConfig) -> Self {
        Self {
            p1: config.p1,
            p2: config.p2,
            center_x: config.center.0,
            center_y: config.center.1,
        }
    }

    /// Create an identity model (no distortion).
    #[inline]
    pub fn identity() -> Self {
        Self {
            p1: 0.0,
            p2: 0.0,
            center_x: 0.0,
            center_y: 0.0,
        }
    }

    /// Get the tangential distortion coefficients.
    #[inline]
    pub fn coefficients(&self) -> (f64, f64) {
        (self.p1, self.p2)
    }

    /// Get the optical center.
    #[inline]
    pub fn center(&self) -> (f64, f64) {
        (self.center_x, self.center_y)
    }

    /// Check if this is an identity (no distortion).
    #[inline]
    pub fn is_identity(&self) -> bool {
        self.p1.abs() < f64::EPSILON && self.p2.abs() < f64::EPSILON
    }

    /// Compute the tangential distortion displacement at a given point.
    ///
    /// Returns (dx, dy) displacement caused by tangential distortion.
    #[inline]
    fn distortion_displacement(&self, x: f64, y: f64) -> (f64, f64) {
        let r_squared = x * x + y * y;
        let xy = x * y;

        // Brown-Conrady tangential distortion:
        // dx = 2*p1*x*y + p2*(r² + 2*x²)
        // dy = p1*(r² + 2*y²) + 2*p2*x*y
        let dx = 2.0 * self.p1 * xy + self.p2 * (r_squared + 2.0 * x * x);
        let dy = self.p1 * (r_squared + 2.0 * y * y) + 2.0 * self.p2 * xy;

        (dx, dy)
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

        let (disp_x, disp_y) = self.distortion_displacement(dx, dy);

        let x_d = self.center_x + dx + disp_x;
        let y_d = self.center_y + dy + disp_y;

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

        // Newton-Raphson iteration to find (x_u, y_u) from (x_d, y_d)
        // We want to solve: (x_d, y_d) = (x_u, y_u) + distortion(x_u, y_u)
        // Start with initial guess (x_u, y_u) ≈ (x_d, y_d)
        let mut x_u = dx_d;
        let mut y_u = dy_d;

        for _ in 0..MAX_ITERATIONS {
            let (disp_x, disp_y) = self.distortion_displacement(x_u, y_u);

            // Residual: f(x_u, y_u) = (x_u + disp_x - x_d, y_u + disp_y - y_d)
            let fx = x_u + disp_x - dx_d;
            let fy = y_u + disp_y - dy_d;

            // Check convergence
            if fx.abs() < CONVERGENCE_THRESHOLD && fy.abs() < CONVERGENCE_THRESHOLD {
                break;
            }

            // Jacobian of the distortion function:
            // J = I + [∂disp_x/∂x  ∂disp_x/∂y]
            //         [∂disp_y/∂x  ∂disp_y/∂y]
            //
            // disp_x = 2*p1*x*y + p2*(r² + 2*x²)
            // disp_y = p1*(r² + 2*y²) + 2*p2*x*y
            //
            // ∂disp_x/∂x = 2*p1*y + p2*(2*x + 4*x) = 2*p1*y + 6*p2*x
            // ∂disp_x/∂y = 2*p1*x + p2*(2*y) = 2*p1*x + 2*p2*y
            // ∂disp_y/∂x = p1*(2*x) + 2*p2*y = 2*p1*x + 2*p2*y
            // ∂disp_y/∂y = p1*(2*y + 4*y) + 2*p2*x = 6*p1*y + 2*p2*x

            let j11 = 1.0 + 2.0 * self.p1 * y_u + 6.0 * self.p2 * x_u;
            let j12 = 2.0 * self.p1 * x_u + 2.0 * self.p2 * y_u;
            let j21 = 2.0 * self.p1 * x_u + 2.0 * self.p2 * y_u;
            let j22 = 1.0 + 6.0 * self.p1 * y_u + 2.0 * self.p2 * x_u;

            // Solve J * delta = -f using Cramer's rule
            let det = j11 * j22 - j12 * j21;
            if det.abs() < 1e-12 {
                break; // Singular Jacobian
            }

            let delta_x = (-fx * j22 + fy * j12) / det;
            let delta_y = (-fy * j11 + fx * j21) / det;

            // Limit step size to prevent divergence
            let max_step = (dx_d.abs() + dy_d.abs()).max(10.0) * 0.5;
            let delta_x = delta_x.clamp(-max_step, max_step);
            let delta_y = delta_y.clamp(-max_step, max_step);

            x_u += delta_x;
            y_u += delta_y;
        }

        (self.center_x + x_u, self.center_y + y_u)
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

    /// Estimate tangential distortion coefficients from matched point pairs.
    ///
    /// Given pairs of (undistorted, distorted) coordinates, estimates the
    /// tangential distortion coefficients using least-squares fitting.
    ///
    /// # Arguments
    /// * `undistorted` - Ideal undistorted point coordinates
    /// * `distorted` - Measured distorted point coordinates
    /// * `center` - Optical center (or None to estimate from data mean)
    ///
    /// # Returns
    /// Estimated `TangentialDistortion` model, or None if fitting fails
    pub fn estimate(
        undistorted: &[(f64, f64)],
        distorted: &[(f64, f64)],
        center: Option<(f64, f64)>,
    ) -> Option<Self> {
        if undistorted.len() != distorted.len() {
            return None;
        }
        if undistorted.len() < 4 {
            // Need at least 4 points for stable estimation
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
        // For each point pair:
        //   dx_d - dx_u = 2*p1*x*y + p2*(r² + 2*x²)
        //   dy_d - dy_u = p1*(r² + 2*y²) + 2*p2*x*y
        //
        // This gives us two equations per point, linear in p1 and p2

        let n = undistorted.len();
        let mut ata = [[0.0; 2]; 2];
        let mut atb = [0.0; 2];

        for i in 0..n {
            let (x_u, y_u) = undistorted[i];
            let (x_d, y_d) = distorted[i];

            let x = x_u - cx;
            let y = y_u - cy;
            let dx_d = x_d - cx;
            let dy_d = y_d - cy;

            let r_squared = x * x + y * y;
            let xy = x * y;

            // First equation: dx_d - x = 2*p1*x*y + p2*(r² + 2*x²)
            // Coefficient for p1: 2*x*y
            // Coefficient for p2: r² + 2*x²
            let a1_p1 = 2.0 * xy;
            let a1_p2 = r_squared + 2.0 * x * x;
            let b1 = dx_d - x;

            // Second equation: dy_d - y = p1*(r² + 2*y²) + 2*p2*x*y
            // Coefficient for p1: r² + 2*y²
            // Coefficient for p2: 2*x*y
            let a2_p1 = r_squared + 2.0 * y * y;
            let a2_p2 = 2.0 * xy;
            let b2 = dy_d - y;

            // Accumulate A^T A and A^T b (both equations)
            ata[0][0] += a1_p1 * a1_p1 + a2_p1 * a2_p1;
            ata[0][1] += a1_p1 * a1_p2 + a2_p1 * a2_p2;
            ata[1][0] += a1_p2 * a1_p1 + a2_p2 * a2_p1;
            ata[1][1] += a1_p2 * a1_p2 + a2_p2 * a2_p2;

            atb[0] += a1_p1 * b1 + a2_p1 * b2;
            atb[1] += a1_p2 * b1 + a2_p2 * b2;
        }

        // Solve the 2x2 system using Cramer's rule
        let det = ata[0][0] * ata[1][1] - ata[0][1] * ata[1][0];
        if det.abs() < 1e-15 {
            return None; // Singular matrix
        }

        let p1 = (atb[0] * ata[1][1] - atb[1] * ata[0][1]) / det;
        let p2 = (ata[0][0] * atb[1] - ata[1][0] * atb[0]) / det;

        Some(Self {
            p1,
            p2,
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
const MAX_ITERATIONS: usize = 15;

/// Convergence threshold for Newton-Raphson iteration.
const CONVERGENCE_THRESHOLD: f64 = 1e-10;

#[cfg(test)]
mod tests {
    use super::*;

    const TOLERANCE: f64 = 1e-6;

    #[test]
    fn test_identity_distortion() {
        let model = TangentialDistortion::identity();

        assert!(model.is_identity());

        let (x_d, y_d) = model.distort(100.0, 200.0);
        assert!((x_d - 100.0).abs() < TOLERANCE);
        assert!((y_d - 200.0).abs() < TOLERANCE);

        let (x_u, y_u) = model.undistort(100.0, 200.0);
        assert!((x_u - 100.0).abs() < TOLERANCE);
        assert!((y_u - 200.0).abs() < TOLERANCE);
    }

    #[test]
    fn test_tangential_distortion_formula() {
        // Test that the distortion formula matches Brown-Conrady model
        let center = (500.0, 500.0);
        let p1 = 0.0001;
        let p2 = 0.00005;
        let model = TangentialDistortion::new(TangentialDistortionConfig { p1, p2, center });

        // Test at a point (100, 200) relative to center
        let test_x = 600.0; // 100 from center
        let test_y = 700.0; // 200 from center

        let x = test_x - center.0; // 100
        let y = test_y - center.1; // 200
        let r_squared = x * x + y * y; // 50000
        let xy = x * y; // 20000

        // Expected displacement by formula:
        // dx = 2*p1*x*y + p2*(r² + 2*x²) = 2*0.0001*20000 + 0.00005*(50000 + 20000) = 4.0 + 3.5 = 7.5
        // dy = p1*(r² + 2*y²) + 2*p2*x*y = 0.0001*(50000 + 80000) + 2*0.00005*20000 = 13.0 + 2.0 = 15.0
        let expected_dx = 2.0 * p1 * xy + p2 * (r_squared + 2.0 * x * x);
        let expected_dy = p1 * (r_squared + 2.0 * y * y) + 2.0 * p2 * xy;

        let (x_d, y_d) = model.distort(test_x, test_y);
        let actual_dx = x_d - test_x;
        let actual_dy = y_d - test_y;

        assert!(
            (actual_dx - expected_dx).abs() < TOLERANCE,
            "dx: {} vs expected {}",
            actual_dx,
            expected_dx
        );
        assert!(
            (actual_dy - expected_dy).abs() < TOLERANCE,
            "dy: {} vs expected {}",
            actual_dy,
            expected_dy
        );
    }

    #[test]
    fn test_distort_undistort_roundtrip() {
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.00005,
            p2: -0.00003,
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
                (x_u - x).abs() < 1e-6,
                "Roundtrip failed for ({}, {}): got ({}, {})",
                x,
                y,
                x_u,
                y_u
            );
            assert!(
                (y_u - y).abs() < 1e-6,
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
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.001,
            p2: -0.0005,
            center,
        });

        let (x_d, y_d) = model.distort(center.0, center.1);
        assert!((x_d - center.0).abs() < TOLERANCE);
        assert!((y_d - center.1).abs() < TOLERANCE);

        let (x_u, y_u) = model.undistort(center.0, center.1);
        assert!((x_u - center.0).abs() < TOLERANCE);
        assert!((y_u - center.1).abs() < TOLERANCE);
    }

    #[test]
    fn test_tangential_asymmetry() {
        // Tangential distortion should NOT be radially symmetric
        let center = (500.0, 500.0);
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.0001,
            p2: 0.0,
            center,
        });

        // Points on opposite sides of y-axis should have different distortions
        let (x1_d, _y1_d) = model.distort(400.0, 600.0); // x=-100, y=100
        let (x2_d, _y2_d) = model.distort(600.0, 600.0); // x=100, y=100

        // For p1 only with same y, p2=0:
        // dx1 = 2*p1*(-100)*100 = -20*p1 = -0.002
        // dx2 = 2*p1*(100)*100 = 20*p1 = 0.002
        // These have opposite signs due to x dependency

        let dx1 = x1_d - 400.0;
        let dx2 = x2_d - 600.0;

        assert!(
            dx1 * dx2 < 0.0,
            "Tangential distortion should be asymmetric: dx1={}, dx2={}",
            dx1,
            dx2
        );
    }

    #[test]
    fn test_p1_vertical_effect() {
        // p1 primarily affects vertical lines
        let center = (500.0, 500.0);
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.0001,
            p2: 0.0,
            center,
        });

        // Points along vertical line (same x, different y)
        let (x1_d, _) = model.distort(600.0, 400.0);
        let (x2_d, _) = model.distort(600.0, 600.0);

        // With p1 > 0 and x > center.x:
        // At y < center.y: xy < 0, dx < 0
        // At y > center.y: xy > 0, dx > 0
        let dx1 = x1_d - 600.0;
        let dx2 = x2_d - 600.0;

        assert!(
            dx1 < dx2,
            "p1 should cause different x-shifts at different y: dx1={}, dx2={}",
            dx1,
            dx2
        );
    }

    #[test]
    fn test_p2_horizontal_effect() {
        // p2 primarily affects horizontal lines
        let center = (500.0, 500.0);
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.0,
            p2: 0.0001,
            center,
        });

        // Points along horizontal line (same y, different x)
        let (_, y1_d) = model.distort(400.0, 600.0);
        let (_, y2_d) = model.distort(600.0, 600.0);

        // With p2 > 0 and y > center.y:
        // At x < center.x: xy < 0, dy term from 2*p2*xy < 0
        // At x > center.x: xy > 0, dy term from 2*p2*xy > 0
        let dy1 = y1_d - 600.0;
        let dy2 = y2_d - 600.0;

        assert!(
            dy1 < dy2,
            "p2 should cause different y-shifts at different x: dy1={}, dy2={}",
            dy1,
            dy2
        );
    }

    #[test]
    fn test_config_constructors() {
        let config1 = TangentialDistortionConfig::with_coefficients(0.001, -0.0005, (100.0, 100.0));
        assert_eq!(config1.p1, 0.001);
        assert_eq!(config1.p2, -0.0005);
        assert_eq!(config1.center, (100.0, 100.0));

        let config2 = TangentialDistortionConfig::centered(1024, 768, 0.0002, -0.0001);
        assert_eq!(config2.center, (512.0, 384.0));
        assert_eq!(config2.p1, 0.0002);
        assert_eq!(config2.p2, -0.0001);

        let config3 = TangentialDistortionConfig::default();
        assert_eq!(config3.p1, 0.0);
        assert_eq!(config3.p2, 0.0);
    }

    #[test]
    fn test_distort_points_batch() {
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.0001,
            p2: -0.00005,
            center: (500.0, 500.0),
        });
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
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.0001,
            p2: -0.00005,
            center: (500.0, 500.0),
        });
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
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.00005,
            p2: 0.00003,
            center: (512.0, 512.0),
        });
        let max = model.max_distortion(1024, 1024);

        // Max distortion should be positive
        assert!(max > 0.0, "Max distortion should be positive: {}", max);

        // Verify by checking corners
        let corners = [(0.0, 0.0), (1024.0, 0.0), (0.0, 1024.0), (1024.0, 1024.0)];
        let mut max_corner = 0.0f64;
        for &(x, y) in &corners {
            let (x_d, y_d) = model.distort(x, y);
            let dist = ((x_d - x).powi(2) + (y_d - y).powi(2)).sqrt();
            max_corner = max_corner.max(dist);
        }

        // max_distortion checks corners and edges, so it should be >= corner max
        assert!(
            max >= max_corner - 1e-6,
            "Max {} should be >= corner max {}",
            max,
            max_corner
        );
    }

    #[test]
    fn test_estimate_tangential_distortion() {
        let center = (500.0, 500.0);
        let p1_true = 0.00005;
        let p2_true = -0.00003;
        let model_true = TangentialDistortion::new(TangentialDistortionConfig {
            p1: p1_true,
            p2: p2_true,
            center,
        });

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
        let model_est = TangentialDistortion::estimate(&undistorted, &distorted, Some(center));

        assert!(model_est.is_some(), "Estimation should succeed");
        let model_est = model_est.unwrap();

        let (p1_est, p2_est) = model_est.coefficients();
        assert!(
            (p1_est - p1_true).abs() < 1e-8,
            "Estimated p1={} should match true p1={}",
            p1_est,
            p1_true
        );
        assert!(
            (p2_est - p2_true).abs() < 1e-8,
            "Estimated p2={} should match true p2={}",
            p2_est,
            p2_true
        );
    }

    #[test]
    fn test_rms_error() {
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.0001,
            p2: -0.00005,
            center: (500.0, 500.0),
        });

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
        let config = TangentialDistortionConfig {
            p1: 0.001,
            p2: -0.0005,
            center: (100.0, 100.0),
        };
        let model = TangentialDistortion::new(config);

        let (p1, p2) = model.coefficients();
        assert_eq!(p1, 0.001);
        assert_eq!(p2, -0.0005);

        let (cx, cy) = model.center();
        assert_eq!(cx, 100.0);
        assert_eq!(cy, 100.0);
    }

    #[test]
    fn test_strong_distortion_undistort_converges() {
        // Test with stronger distortion to ensure Newton-Raphson converges
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.0005,
            p2: -0.0003,
            center: (500.0, 500.0),
        });

        let test_points = [
            (100.0, 100.0),
            (300.0, 700.0),
            (900.0, 900.0),
            (700.0, 300.0),
        ];

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
    fn test_estimate_fails_with_insufficient_points() {
        let undistorted = vec![(100.0, 100.0), (200.0, 200.0), (300.0, 300.0)];
        let distorted = vec![(101.0, 101.0), (202.0, 202.0), (303.0, 303.0)];

        // Not enough points for estimation (need 4)
        let result = TangentialDistortion::estimate(&undistorted, &distorted, None);
        assert!(result.is_none(), "Should fail with insufficient points");
    }

    #[test]
    fn test_estimate_fails_with_mismatched_lengths() {
        let undistorted = vec![
            (100.0, 100.0),
            (200.0, 200.0),
            (300.0, 300.0),
            (400.0, 400.0),
        ];
        let distorted = vec![(101.0, 101.0)];

        let result = TangentialDistortion::estimate(&undistorted, &distorted, None);
        assert!(result.is_none(), "Should fail with mismatched lengths");
    }

    #[test]
    fn test_combined_with_radial() {
        // Test that tangential distortion can work alongside radial
        // (commonly used together in camera calibration)
        use crate::registration::distortion::RadialDistortion;

        let center = (500.0, 500.0);

        let radial = RadialDistortion::barrel(0.00001, center);
        let tangential = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.00005,
            p2: -0.00003,
            center,
        });

        // Apply both distortions
        let x = 300.0;
        let y = 400.0;

        // Apply radial first, then tangential
        let (x_r, y_r) = radial.distort(x, y);
        let (x_rt, y_rt) = tangential.distort(x_r, y_r);

        // The combined distortion should be different from either alone
        let (x_t, y_t) = tangential.distort(x, y);

        assert!(
            (x_rt - x_t).abs() > 0.01 || (y_rt - y_t).abs() > 0.01,
            "Combined distortion should differ from tangential alone"
        );
        assert!(
            (x_rt - x_r).abs() > 0.01 || (y_rt - y_r).abs() > 0.01,
            "Combined distortion should differ from radial alone"
        );
    }

    #[test]
    fn test_negative_coefficients() {
        // Negative coefficients should work correctly
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: -0.0001,
            p2: -0.00005,
            center: (500.0, 500.0),
        });

        // Roundtrip should still work
        let (x_d, y_d) = model.distort(300.0, 400.0);
        let (x_u, y_u) = model.undistort(x_d, y_d);

        assert!((x_u - 300.0).abs() < 1e-6);
        assert!((y_u - 400.0).abs() < 1e-6);
    }

    #[test]
    fn test_estimate_with_center_auto() {
        // Test estimation with automatic center detection
        let center = (500.0, 500.0);
        let model_true = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.00003,
            p2: -0.00002,
            center,
        });

        // Generate grid centered around the expected center
        let mut undistorted = Vec::new();
        let mut distorted = Vec::new();

        for y in (100..=900).step_by(100) {
            for x in (100..=900).step_by(100) {
                let xu = x as f64;
                let yu = y as f64;
                undistorted.push((xu, yu));
                distorted.push(model_true.distort(xu, yu));
            }
        }

        // Estimate without specifying center
        let model_est = TangentialDistortion::estimate(&undistorted, &distorted, None);
        assert!(model_est.is_some());

        let model_est = model_est.unwrap();

        // RMS error should be small
        let rms = model_est.rms_error(&undistorted, &distorted);
        assert!(rms < 0.1, "RMS error {} should be small", rms);
    }
}
