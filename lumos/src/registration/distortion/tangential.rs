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
//! use glam::DVec2;
//!
//! // Create a model with known coefficients
//! let model = TangentialDistortion::new(
//!     TangentialDistortionConfig {
//!         p1: 0.0001,  // Vertical decentering
//!         p2: -0.00005, // Horizontal decentering
//!         center: DVec2::new(1024.0, 768.0),  // Optical center
//!     }
//! );
//!
//! // Apply distortion (undistorted -> distorted)
//! let distorted = model.distort(DVec2::new(512.0, 384.0));
//!
//! // Remove distortion (distorted -> undistorted)
//! let undistorted = model.undistort(distorted);
//! ```

use glam::DVec2;

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
    pub center: DVec2,
}

impl Default for TangentialDistortionConfig {
    fn default() -> Self {
        Self {
            p1: 0.0,
            p2: 0.0,
            center: DVec2::ZERO,
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
    pub fn with_coefficients(p1: f64, p2: f64, center: DVec2) -> Self {
        Self { p1, p2, center }
    }

    /// Create a config centered at the image center.
    #[inline]
    pub fn centered(width: u32, height: u32, p1: f64, p2: f64) -> Self {
        Self {
            p1,
            p2,
            center: DVec2::new(width as f64 / 2.0, height as f64 / 2.0),
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
    center: DVec2,
}

impl TangentialDistortion {
    /// Create a new tangential distortion model.
    #[inline]
    pub fn new(config: TangentialDistortionConfig) -> Self {
        Self {
            p1: config.p1,
            p2: config.p2,
            center: config.center,
        }
    }

    /// Create an identity model (no distortion).
    #[inline]
    pub fn identity() -> Self {
        Self {
            p1: 0.0,
            p2: 0.0,
            center: DVec2::ZERO,
        }
    }

    /// Get the tangential distortion coefficients.
    #[inline]
    pub fn coefficients(&self) -> (f64, f64) {
        (self.p1, self.p2)
    }

    /// Get the optical center.
    #[inline]
    pub fn center(&self) -> DVec2 {
        self.center
    }

    /// Check if this is an identity (no distortion).
    #[inline]
    pub fn is_identity(&self) -> bool {
        self.p1.abs() < f64::EPSILON && self.p2.abs() < f64::EPSILON
    }

    /// Compute the tangential distortion displacement at a given point.
    ///
    /// Returns displacement caused by tangential distortion.
    #[inline]
    fn distortion_displacement(&self, p: DVec2) -> DVec2 {
        let r_squared = p.length_squared();
        let xy = p.x * p.y;

        // Brown-Conrady tangential distortion:
        // dx = 2*p1*x*y + p2*(r² + 2*x²)
        // dy = p1*(r² + 2*y²) + 2*p2*x*y
        let dx = 2.0 * self.p1 * xy + self.p2 * (r_squared + 2.0 * p.x * p.x);
        let dy = self.p1 * (r_squared + 2.0 * p.y * p.y) + 2.0 * self.p2 * xy;

        DVec2::new(dx, dy)
    }

    /// Apply distortion: undistorted coordinates -> distorted coordinates.
    ///
    /// This is the forward model: given ideal (undistorted) pixel coordinates,
    /// compute where they appear in the distorted image.
    ///
    /// # Arguments
    /// * `p` - Undistorted coordinates (pixel)
    ///
    /// # Returns
    /// Distorted coordinates
    #[inline]
    pub fn distort(&self, p: DVec2) -> DVec2 {
        let d = p - self.center;
        let disp = self.distortion_displacement(d);
        self.center + d + disp
    }

    /// Remove distortion: distorted coordinates -> undistorted coordinates.
    ///
    /// This is the inverse model: given measured (distorted) pixel coordinates,
    /// compute the ideal undistorted coordinates.
    ///
    /// Uses Newton-Raphson iteration for accurate inversion.
    ///
    /// # Arguments
    /// * `p` - Distorted coordinates (pixel)
    ///
    /// # Returns
    /// Undistorted coordinates
    pub fn undistort(&self, p: DVec2) -> DVec2 {
        // Fast path for identity
        if self.is_identity() {
            return p;
        }

        let d = p - self.center;

        // Newton-Raphson iteration to find (x_u, y_u) from (x_d, y_d)
        // We want to solve: (x_d, y_d) = (x_u, y_u) + distortion(x_u, y_u)
        // Start with initial guess (x_u, y_u) ≈ (x_d, y_d)
        let mut u = d;

        for _ in 0..MAX_ITERATIONS {
            let disp = self.distortion_displacement(u);

            // Residual: f(u) = u + disp - d
            let f = u + disp - d;

            // Check convergence
            if f.x.abs() < CONVERGENCE_THRESHOLD && f.y.abs() < CONVERGENCE_THRESHOLD {
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

            let j11 = 1.0 + 2.0 * self.p1 * u.y + 6.0 * self.p2 * u.x;
            let j12 = 2.0 * self.p1 * u.x + 2.0 * self.p2 * u.y;
            let j21 = 2.0 * self.p1 * u.x + 2.0 * self.p2 * u.y;
            let j22 = 1.0 + 6.0 * self.p1 * u.y + 2.0 * self.p2 * u.x;

            // Solve J * delta = -f using Cramer's rule
            let det = j11 * j22 - j12 * j21;
            if det.abs() < 1e-12 {
                break; // Singular Jacobian
            }

            let delta_x = (-f.x * j22 + f.y * j12) / det;
            let delta_y = (-f.y * j11 + f.x * j21) / det;

            // Limit step size to prevent divergence
            let max_step = (d.x.abs() + d.y.abs()).max(10.0) * 0.5;
            let delta = DVec2::new(
                delta_x.clamp(-max_step, max_step),
                delta_y.clamp(-max_step, max_step),
            );

            u += delta;
        }

        self.center + u
    }

    /// Apply distortion to multiple points.
    #[inline]
    pub fn distort_points(&self, points: &[DVec2]) -> Vec<DVec2> {
        points.iter().map(|&p| self.distort(p)).collect()
    }

    /// Remove distortion from multiple points.
    #[inline]
    pub fn undistort_points(&self, points: &[DVec2]) -> Vec<DVec2> {
        points.iter().map(|&p| self.undistort(p)).collect()
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
            DVec2::new(0.0, 0.0),
            DVec2::new(w, 0.0),
            DVec2::new(0.0, h),
            DVec2::new(w, h),
            DVec2::new(w / 2.0, 0.0),
            DVec2::new(w / 2.0, h),
            DVec2::new(0.0, h / 2.0),
            DVec2::new(w, h / 2.0),
        ];

        test_points
            .iter()
            .map(|&p| {
                let d = self.distort(p);
                p.distance(d)
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
        undistorted: &[DVec2],
        distorted: &[DVec2],
        center: Option<DVec2>,
    ) -> Option<Self> {
        if undistorted.len() != distorted.len() {
            return None;
        }
        if undistorted.len() < 4 {
            // Need at least 4 points for stable estimation
            return None;
        }

        // Determine center
        let c = center.unwrap_or_else(|| {
            let sum: DVec2 = undistorted.iter().copied().sum();
            sum / undistorted.len() as f64
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
            let pu = undistorted[i] - c;
            let pd = distorted[i] - c;

            let r_squared = pu.length_squared();
            let xy = pu.x * pu.y;

            // First equation: pd.x - pu.x = 2*p1*x*y + p2*(r² + 2*x²)
            // Coefficient for p1: 2*x*y
            // Coefficient for p2: r² + 2*x²
            let a1_p1 = 2.0 * xy;
            let a1_p2 = r_squared + 2.0 * pu.x * pu.x;
            let b1 = pd.x - pu.x;

            // Second equation: pd.y - pu.y = p1*(r² + 2*y²) + 2*p2*x*y
            // Coefficient for p1: r² + 2*y²
            // Coefficient for p2: 2*x*y
            let a2_p1 = r_squared + 2.0 * pu.y * pu.y;
            let a2_p2 = 2.0 * xy;
            let b2 = pd.y - pu.y;

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

        Some(Self { p1, p2, center: c })
    }

    /// Compute RMS residual error for a set of point correspondences.
    ///
    /// # Arguments
    /// * `undistorted` - Ideal undistorted point coordinates
    /// * `distorted` - Measured distorted point coordinates
    ///
    /// # Returns
    /// RMS error in pixels
    pub fn rms_error(&self, undistorted: &[DVec2], distorted: &[DVec2]) -> f64 {
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
            .map(|(&pu, &pd)| {
                let pred = self.distort(pu);
                pred.distance_squared(pd)
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

        let p = DVec2::new(100.0, 200.0);
        let d = model.distort(p);
        assert!((d.x - 100.0).abs() < TOLERANCE);
        assert!((d.y - 200.0).abs() < TOLERANCE);

        let u = model.undistort(p);
        assert!((u.x - 100.0).abs() < TOLERANCE);
        assert!((u.y - 200.0).abs() < TOLERANCE);
    }

    #[test]
    fn test_tangential_distortion_formula() {
        // Test that the distortion formula matches Brown-Conrady model
        let center = DVec2::new(500.0, 500.0);
        let p1 = 0.0001;
        let p2 = 0.00005;
        let model = TangentialDistortion::new(TangentialDistortionConfig { p1, p2, center });

        // Test at a point (100, 200) relative to center
        let test_p = DVec2::new(600.0, 700.0); // 100, 200 from center

        let rel = test_p - center; // (100, 200)
        let r_squared = rel.length_squared(); // 50000
        let xy = rel.x * rel.y; // 20000

        // Expected displacement by formula:
        // dx = 2*p1*x*y + p2*(r² + 2*x²) = 2*0.0001*20000 + 0.00005*(50000 + 20000) = 4.0 + 3.5 = 7.5
        // dy = p1*(r² + 2*y²) + 2*p2*x*y = 0.0001*(50000 + 80000) + 2*0.00005*20000 = 13.0 + 2.0 = 15.0
        let expected_dx = 2.0 * p1 * xy + p2 * (r_squared + 2.0 * rel.x * rel.x);
        let expected_dy = p1 * (r_squared + 2.0 * rel.y * rel.y) + 2.0 * p2 * xy;

        let d = model.distort(test_p);
        let actual_dx = d.x - test_p.x;
        let actual_dy = d.y - test_p.y;

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
            center: DVec2::new(500.0, 400.0),
        });

        let test_points = [
            DVec2::new(100.0, 100.0),
            DVec2::new(500.0, 400.0), // center - should be unchanged
            DVec2::new(900.0, 700.0),
            DVec2::new(250.0, 600.0),
            DVec2::new(750.0, 200.0),
        ];

        for &p in &test_points {
            let d = model.distort(p);
            let u = model.undistort(d);

            assert!(
                (u.x - p.x).abs() < 1e-6,
                "Roundtrip failed for {:?}: got {:?}",
                p,
                u
            );
            assert!(
                (u.y - p.y).abs() < 1e-6,
                "Roundtrip failed for {:?}: got {:?}",
                p,
                u
            );
        }
    }

    #[test]
    fn test_center_point_unchanged() {
        let center = DVec2::new(512.0, 384.0);
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.001,
            p2: -0.0005,
            center,
        });

        let d = model.distort(center);
        assert!((d.x - center.x).abs() < TOLERANCE);
        assert!((d.y - center.y).abs() < TOLERANCE);

        let u = model.undistort(center);
        assert!((u.x - center.x).abs() < TOLERANCE);
        assert!((u.y - center.y).abs() < TOLERANCE);
    }

    #[test]
    fn test_tangential_asymmetry() {
        // Tangential distortion should NOT be radially symmetric
        let center = DVec2::new(500.0, 500.0);
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.0001,
            p2: 0.0,
            center,
        });

        // Points on opposite sides of y-axis should have different distortions
        let d1 = model.distort(DVec2::new(400.0, 600.0)); // x=-100, y=100
        let d2 = model.distort(DVec2::new(600.0, 600.0)); // x=100, y=100

        // For p1 only with same y, p2=0:
        // dx1 = 2*p1*(-100)*100 = -20*p1 = -0.002
        // dx2 = 2*p1*(100)*100 = 20*p1 = 0.002
        // These have opposite signs due to x dependency

        let dx1 = d1.x - 400.0;
        let dx2 = d2.x - 600.0;

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
        let center = DVec2::new(500.0, 500.0);
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.0001,
            p2: 0.0,
            center,
        });

        // Points along vertical line (same x, different y)
        let d1 = model.distort(DVec2::new(600.0, 400.0));
        let d2 = model.distort(DVec2::new(600.0, 600.0));

        // With p1 > 0 and x > center.x:
        // At y < center.y: xy < 0, dx < 0
        // At y > center.y: xy > 0, dx > 0
        let dx1 = d1.x - 600.0;
        let dx2 = d2.x - 600.0;

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
        let center = DVec2::new(500.0, 500.0);
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.0,
            p2: 0.0001,
            center,
        });

        // Points along horizontal line (same y, different x)
        let d1 = model.distort(DVec2::new(400.0, 600.0));
        let d2 = model.distort(DVec2::new(600.0, 600.0));

        // With p2 > 0 and y > center.y:
        // At x < center.x: xy < 0, dy term from 2*p2*xy < 0
        // At x > center.x: xy > 0, dy term from 2*p2*xy > 0
        let dy1 = d1.y - 600.0;
        let dy2 = d2.y - 600.0;

        assert!(
            dy1 < dy2,
            "p2 should cause different y-shifts at different x: dy1={}, dy2={}",
            dy1,
            dy2
        );
    }

    #[test]
    fn test_config_constructors() {
        let config1 =
            TangentialDistortionConfig::with_coefficients(0.001, -0.0005, DVec2::new(100.0, 100.0));
        assert_eq!(config1.p1, 0.001);
        assert_eq!(config1.p2, -0.0005);
        assert_eq!(config1.center, DVec2::new(100.0, 100.0));

        let config2 = TangentialDistortionConfig::centered(1024, 768, 0.0002, -0.0001);
        assert_eq!(config2.center, DVec2::new(512.0, 384.0));
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
            center: DVec2::new(500.0, 500.0),
        });
        let points = vec![
            DVec2::new(100.0, 100.0),
            DVec2::new(200.0, 200.0),
            DVec2::new(300.0, 300.0),
        ];

        let distorted = model.distort_points(&points);

        assert_eq!(distorted.len(), points.len());

        for (i, &p) in points.iter().enumerate() {
            let d = model.distort(p);
            assert!((distorted[i].x - d.x).abs() < TOLERANCE);
            assert!((distorted[i].y - d.y).abs() < TOLERANCE);
        }
    }

    #[test]
    fn test_undistort_points_batch() {
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.0001,
            p2: -0.00005,
            center: DVec2::new(500.0, 500.0),
        });
        let points = vec![
            DVec2::new(100.0, 100.0),
            DVec2::new(200.0, 200.0),
            DVec2::new(300.0, 300.0),
        ];

        let undistorted = model.undistort_points(&points);

        assert_eq!(undistorted.len(), points.len());

        for (i, &p) in points.iter().enumerate() {
            let u = model.undistort(p);
            assert!((undistorted[i].x - u.x).abs() < TOLERANCE);
            assert!((undistorted[i].y - u.y).abs() < TOLERANCE);
        }
    }

    #[test]
    fn test_max_distortion() {
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.00005,
            p2: 0.00003,
            center: DVec2::new(512.0, 512.0),
        });
        let max = model.max_distortion(1024, 1024);

        // Max distortion should be positive
        assert!(max > 0.0, "Max distortion should be positive: {}", max);

        // Verify by checking corners
        let corners = [
            DVec2::new(0.0, 0.0),
            DVec2::new(1024.0, 0.0),
            DVec2::new(0.0, 1024.0),
            DVec2::new(1024.0, 1024.0),
        ];
        let mut max_corner = 0.0f64;
        for &p in &corners {
            let d = model.distort(p);
            let dist = p.distance(d);
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
        let center = DVec2::new(500.0, 500.0);
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
                let pu = DVec2::new(x as f64, y as f64);
                undistorted.push(pu);
                distorted.push(model_true.distort(pu));
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
            center: DVec2::new(500.0, 500.0),
        });

        // Perfect fit should have zero error
        let undistorted = vec![
            DVec2::new(100.0, 100.0),
            DVec2::new(200.0, 200.0),
            DVec2::new(300.0, 300.0),
        ];
        let distorted: Vec<_> = undistorted.iter().map(|&p| model.distort(p)).collect();

        let rms = model.rms_error(&undistorted, &distorted);
        assert!(rms < TOLERANCE, "Perfect fit should have zero RMS: {}", rms);
    }

    #[test]
    fn test_coefficients_getter() {
        let config = TangentialDistortionConfig {
            p1: 0.001,
            p2: -0.0005,
            center: DVec2::new(100.0, 100.0),
        };
        let model = TangentialDistortion::new(config);

        let (p1, p2) = model.coefficients();
        assert_eq!(p1, 0.001);
        assert_eq!(p2, -0.0005);

        let c = model.center();
        assert_eq!(c, DVec2::new(100.0, 100.0));
    }

    #[test]
    fn test_strong_distortion_undistort_converges() {
        // Test with stronger distortion to ensure Newton-Raphson converges
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.0005,
            p2: -0.0003,
            center: DVec2::new(500.0, 500.0),
        });

        let test_points = [
            DVec2::new(100.0, 100.0),
            DVec2::new(300.0, 700.0),
            DVec2::new(900.0, 900.0),
            DVec2::new(700.0, 300.0),
        ];

        for &p in &test_points {
            let d = model.distort(p);
            let u = model.undistort(d);

            assert!(
                (u.x - p.x).abs() < 0.01,
                "Strong distortion roundtrip failed for {:?}: got {:?}",
                p,
                u
            );
            assert!(
                (u.y - p.y).abs() < 0.01,
                "Strong distortion roundtrip failed for {:?}: got {:?}",
                p,
                u
            );
        }
    }

    #[test]
    fn test_estimate_fails_with_insufficient_points() {
        let undistorted = vec![
            DVec2::new(100.0, 100.0),
            DVec2::new(200.0, 200.0),
            DVec2::new(300.0, 300.0),
        ];
        let distorted = vec![
            DVec2::new(101.0, 101.0),
            DVec2::new(202.0, 202.0),
            DVec2::new(303.0, 303.0),
        ];

        // Not enough points for estimation (need 4)
        let result = TangentialDistortion::estimate(&undistorted, &distorted, None);
        assert!(result.is_none(), "Should fail with insufficient points");
    }

    #[test]
    fn test_estimate_fails_with_mismatched_lengths() {
        let undistorted = vec![
            DVec2::new(100.0, 100.0),
            DVec2::new(200.0, 200.0),
            DVec2::new(300.0, 300.0),
            DVec2::new(400.0, 400.0),
        ];
        let distorted = vec![DVec2::new(101.0, 101.0)];

        let result = TangentialDistortion::estimate(&undistorted, &distorted, None);
        assert!(result.is_none(), "Should fail with mismatched lengths");
    }

    #[test]
    fn test_combined_with_radial() {
        // Test that tangential distortion can work alongside radial
        // (commonly used together in camera calibration)
        use crate::registration::distortion::RadialDistortion;

        let center = DVec2::new(500.0, 500.0);

        let radial = RadialDistortion::barrel(0.00001, center);
        let tangential = TangentialDistortion::new(TangentialDistortionConfig {
            p1: 0.00005,
            p2: -0.00003,
            center,
        });

        // Apply both distortions
        let p = DVec2::new(300.0, 400.0);

        // Apply radial first, then tangential
        let r = radial.distort(p);
        let rt = tangential.distort(r);

        // The combined distortion should be different from either alone
        let t = tangential.distort(p);

        assert!(
            (rt.x - t.x).abs() > 0.01 || (rt.y - t.y).abs() > 0.01,
            "Combined distortion should differ from tangential alone"
        );
        assert!(
            (rt.x - r.x).abs() > 0.01 || (rt.y - r.y).abs() > 0.01,
            "Combined distortion should differ from radial alone"
        );
    }

    #[test]
    fn test_negative_coefficients() {
        // Negative coefficients should work correctly
        let model = TangentialDistortion::new(TangentialDistortionConfig {
            p1: -0.0001,
            p2: -0.00005,
            center: DVec2::new(500.0, 500.0),
        });

        // Roundtrip should still work
        let p = DVec2::new(300.0, 400.0);
        let d = model.distort(p);
        let u = model.undistort(d);

        assert!((u.x - 300.0).abs() < 1e-6);
        assert!((u.y - 400.0).abs() < 1e-6);
    }

    #[test]
    fn test_estimate_with_center_auto() {
        // Test estimation with automatic center detection
        let center = DVec2::new(500.0, 500.0);
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
                let pu = DVec2::new(x as f64, y as f64);
                undistorted.push(pu);
                distorted.push(model_true.distort(pu));
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
