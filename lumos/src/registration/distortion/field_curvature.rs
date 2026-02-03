//! Field curvature correction for optical systems.
//!
//! This module provides correction for Petzval field curvature, an optical aberration
//! where the focal plane is curved rather than flat. This causes defocus that varies
//! with radial distance from the optical axis.
//!
//! # Model
//!
//! ## Petzval Field Curvature
//!
//! In a simple optical system, the image of a flat object is formed on a curved
//! surface called the Petzval surface. The sag (defocus distance) at radius r is:
//!
//! ```text
//! z = r² / (2R)
//! ```
//!
//! where R is the Petzval radius of curvature.
//!
//! ## Effect on Image Coordinates
//!
//! Field curvature causes a radial scaling effect due to defocus-induced magnification
//! change. The coordinate transformation is modeled as:
//!
//! ```text
//! r' = r × (1 + c₁r² + c₂r⁴)
//! ```
//!
//! where:
//! - r is the radial distance from the optical center
//! - c₁ is the primary curvature coefficient (related to 1/R)
//! - c₂ is the secondary curvature coefficient (higher-order correction)
//!
//! **Interpretation**:
//! - c₁ > 0: Outward field curvature (magnification increases with radius)
//! - c₁ < 0: Inward field curvature (magnification decreases with radius)
//!
//! This is similar to radial distortion but models a different physical phenomenon.
//! In practice, field curvature and radial distortion often occur together and can
//! be corrected simultaneously by combining their coefficients.
//!
//! # Usage
//!
//! ```ignore
//! use lumos::registration::distortion::{FieldCurvature, FieldCurvatureConfig};
//! use glam::DVec2;
//!
//! // Create a model with known curvature
//! let model = FieldCurvature::new(FieldCurvatureConfig {
//!     c1: 0.00001,  // Primary curvature
//!     c2: 0.0,
//!     center: DVec2::new(1024.0, 768.0),
//! });
//!
//! // Apply curvature effect (ideal -> curved)
//! let curved = model.apply(DVec2::new(512.0, 384.0));
//!
//! // Remove curvature effect (curved -> ideal)
//! let flat = model.correct(curved);
//! ```
//!
//! # Relationship to Other Distortions
//!
//! Field curvature is related to but distinct from:
//! - **Radial distortion**: Caused by lens geometry, affects all focus planes equally
//! - **Field curvature**: Caused by Petzval sum, is a focus-dependent effect
//!
//! In astrophotography, both effects manifest as radial coordinate shifts and are
//! often corrected together. For best results, calibrate both models from star data.

use glam::DVec2;

/// Configuration for field curvature correction.
#[derive(Debug, Clone, Copy)]
pub struct FieldCurvatureConfig {
    /// Primary field curvature coefficient (r² term).
    /// Positive values indicate outward curvature (magnification increases with radius).
    /// Negative values indicate inward curvature (magnification decreases with radius).
    /// Typical values: 1e-8 to 1e-5 for astrophotography.
    pub c1: f64,
    /// Secondary field curvature coefficient (r⁴ term).
    /// Higher-order correction for complex optical systems.
    pub c2: f64,
    /// Optical center (principal point) in pixel coordinates.
    pub center: DVec2,
}

impl Default for FieldCurvatureConfig {
    fn default() -> Self {
        Self {
            c1: 0.0,
            c2: 0.0,
            center: DVec2::ZERO,
        }
    }
}

impl FieldCurvatureConfig {
    /// Create a config with a single curvature coefficient.
    ///
    /// # Arguments
    /// * `c1` - Primary curvature coefficient
    /// * `center` - Optical center in pixel coordinates
    #[inline]
    pub fn simple(c1: f64, center: DVec2) -> Self {
        Self {
            c1,
            c2: 0.0,
            center,
        }
    }

    /// Create a config from a Petzval radius.
    ///
    /// The Petzval radius R is related to the curvature coefficient by:
    /// c₁ ≈ k / (2R × f²)
    ///
    /// where f is the focal length and k is a scaling factor that depends on
    /// the optical system. For a simplified model, we use:
    /// c₁ ≈ 1 / (2R × pixel_scale²)
    ///
    /// # Arguments
    /// * `petzval_radius` - Petzval radius of curvature in the same units as focal length
    /// * `pixel_scale` - Pixels per unit length (e.g., microns per pixel)
    /// * `center` - Optical center in pixel coordinates
    #[inline]
    pub fn from_petzval_radius(petzval_radius: f64, pixel_scale: f64, center: DVec2) -> Self {
        debug_assert!(
            petzval_radius.abs() > f64::EPSILON,
            "Petzval radius must be non-zero"
        );
        debug_assert!(pixel_scale > 0.0, "Pixel scale must be positive");
        let c1 = 1.0 / (2.0 * petzval_radius * pixel_scale * pixel_scale);
        Self {
            c1,
            c2: 0.0,
            center,
        }
    }

    /// Create a config centered at the image center.
    #[inline]
    pub fn centered(width: u32, height: u32, c1: f64) -> Self {
        Self {
            c1,
            c2: 0.0,
            center: DVec2::new(width as f64 / 2.0, height as f64 / 2.0),
        }
    }

    /// Create a config with both curvature coefficients.
    #[inline]
    pub fn with_coefficients(c1: f64, c2: f64, center: DVec2) -> Self {
        Self { c1, c2, center }
    }
}

/// Field curvature model for Petzval surface correction.
///
/// This model corrects for the radial scaling effect caused by field curvature,
/// where the focal plane is curved rather than flat.
#[derive(Debug, Clone, Copy)]
pub struct FieldCurvature {
    /// Primary curvature coefficient
    c1: f64,
    /// Secondary curvature coefficient
    c2: f64,
    /// Optical center
    center: DVec2,
}

impl FieldCurvature {
    /// Create a new field curvature model.
    #[inline]
    pub fn new(config: FieldCurvatureConfig) -> Self {
        Self {
            c1: config.c1,
            c2: config.c2,
            center: config.center,
        }
    }

    /// Create an identity model (no curvature).
    #[inline]
    pub fn identity() -> Self {
        Self {
            c1: 0.0,
            c2: 0.0,
            center: DVec2::ZERO,
        }
    }

    /// Get the curvature coefficients.
    #[inline]
    pub fn coefficients(&self) -> (f64, f64) {
        (self.c1, self.c2)
    }

    /// Get the optical center.
    #[inline]
    pub fn center(&self) -> DVec2 {
        self.center
    }

    /// Check if this is an identity (no curvature).
    #[inline]
    pub fn is_identity(&self) -> bool {
        self.c1.abs() < f64::EPSILON && self.c2.abs() < f64::EPSILON
    }

    /// Check if curvature is outward (magnification increases with radius).
    #[inline]
    pub fn is_outward(&self) -> bool {
        self.c1 > 0.0
    }

    /// Check if curvature is inward (magnification decreases with radius).
    #[inline]
    pub fn is_inward(&self) -> bool {
        self.c1 < 0.0
    }

    /// Compute the scaling factor at a given radius squared.
    ///
    /// Returns `1 + c₁r² + c₂r⁴`
    #[inline]
    fn scale_factor(&self, r_squared: f64) -> f64 {
        let r4 = r_squared * r_squared;
        1.0 + self.c1 * r_squared + self.c2 * r4
    }

    /// Apply field curvature effect: ideal (flat) coordinates -> curved coordinates.
    ///
    /// This transforms coordinates from an ideal flat focal plane to coordinates
    /// as they would appear on a curved Petzval surface.
    ///
    /// # Arguments
    /// * `p` - Ideal coordinates (pixel)
    ///
    /// # Returns
    /// Curved coordinates
    #[inline]
    pub fn apply(&self, p: DVec2) -> DVec2 {
        let d = p - self.center;
        let r_squared = d.length_squared();
        let factor = self.scale_factor(r_squared);
        self.center + d * factor
    }

    /// Correct field curvature: curved coordinates -> ideal (flat) coordinates.
    ///
    /// This transforms coordinates from a curved Petzval surface back to
    /// coordinates on an ideal flat focal plane.
    ///
    /// Uses Newton-Raphson iteration for accurate inversion.
    ///
    /// # Arguments
    /// * `p` - Curved coordinates (pixel)
    ///
    /// # Returns
    /// Ideal coordinates
    pub fn correct(&self, p: DVec2) -> DVec2 {
        // Fast path for identity
        if self.is_identity() {
            return p;
        }

        let d_c = p - self.center;
        let r_c_squared = d_c.length_squared();

        // Fast path for center point
        if r_c_squared < f64::EPSILON {
            return p;
        }

        // Newton-Raphson iteration to find r_f from r_c
        // We want: r_c = r_f × (1 + c1×r_f² + c2×r_f⁴)
        // Let f(r_f) = r_f × scale(r_f) - r_c = 0
        let r_c = r_c_squared.sqrt();
        let mut r_f = r_c;

        for _ in 0..MAX_ITERATIONS {
            let r_f_squared = r_f * r_f;
            let factor = self.scale_factor(r_f_squared);

            // Check for invalid model (factor should be positive)
            if factor <= 0.0 {
                break;
            }

            let f = r_f * factor - r_c;

            // Derivative: d/dr_f [r_f × (1 + c1×r² + c2×r⁴)]
            // = 1 + 3×c1×r² + 5×c2×r⁴
            let r4 = r_f_squared * r_f_squared;
            let df = 1.0 + 3.0 * self.c1 * r_f_squared + 5.0 * self.c2 * r4;

            // Guard against division by very small derivative
            if df.abs() < 1e-12 {
                break;
            }

            let delta = f / df;

            // Limit step size to prevent divergence
            let max_step = r_f * 0.5;
            let delta = delta.clamp(-max_step, max_step);

            r_f -= delta;

            // Ensure r_f stays positive
            if r_f <= 0.0 {
                r_f = r_c * 0.1;
            }

            if delta.abs() < CONVERGENCE_THRESHOLD {
                break;
            }
        }

        // Compute ideal coordinates using the ratio
        let scale = if r_c > f64::EPSILON { r_f / r_c } else { 1.0 };

        self.center + d_c * scale
    }

    /// Apply curvature to multiple points.
    #[inline]
    pub fn apply_points(&self, points: &[DVec2]) -> Vec<DVec2> {
        points.iter().map(|&p| self.apply(p)).collect()
    }

    /// Correct curvature for multiple points.
    #[inline]
    pub fn correct_points(&self, points: &[DVec2]) -> Vec<DVec2> {
        points.iter().map(|&p| self.correct(p)).collect()
    }

    /// Compute the maximum curvature effect across an image.
    ///
    /// Samples corners and edges to find the maximum displacement.
    ///
    /// # Arguments
    /// * `width` - Image width in pixels
    /// * `height` - Image height in pixels
    ///
    /// # Returns
    /// Maximum displacement in pixels due to field curvature
    pub fn max_effect(&self, width: u32, height: u32) -> f64 {
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
                let c = self.apply(p);
                p.distance(c)
            })
            .fold(0.0f64, f64::max)
    }

    /// Compute the sag (defocus distance) at a given position.
    ///
    /// This represents how far the Petzval surface deviates from a flat plane
    /// at the given position. Useful for understanding the physical effect.
    ///
    /// The sag is approximated as: z ≈ r² × c₁ / 2
    ///
    /// # Arguments
    /// * `p` - Position (pixel)
    ///
    /// # Returns
    /// Sag (defocus) in arbitrary units proportional to pixel displacement
    pub fn sag_at(&self, p: DVec2) -> f64 {
        let d = p - self.center;
        let r_squared = d.length_squared();

        // Sag ≈ r²×c₁/2 (first-order approximation)
        // For the full model including c2: sag ≈ r²×c₁/2 + r⁴×c₂/4
        let r4 = r_squared * r_squared;
        r_squared * self.c1 / 2.0 + r4 * self.c2 / 4.0
    }

    /// Estimate field curvature coefficients from matched point pairs.
    ///
    /// Given pairs of (ideal, curved) coordinates, estimates the field curvature
    /// coefficients using least-squares fitting.
    ///
    /// # Arguments
    /// * `ideal` - Ideal (flat) point coordinates
    /// * `curved` - Measured (curved) point coordinates
    /// * `center` - Optical center (or None to estimate from data mean)
    /// * `num_coefficients` - Number of coefficients to estimate (1-2)
    ///
    /// # Returns
    /// Estimated `FieldCurvature` model, or None if fitting fails
    pub fn estimate(
        ideal: &[DVec2],
        curved: &[DVec2],
        center: Option<DVec2>,
        num_coefficients: usize,
    ) -> Option<Self> {
        if ideal.len() != curved.len() {
            return None;
        }
        if ideal.len() < num_coefficients.max(3) {
            return None;
        }
        if num_coefficients == 0 || num_coefficients > 2 {
            return None;
        }

        // Determine center
        let c = center.unwrap_or_else(|| {
            let sum: DVec2 = ideal.iter().copied().sum();
            sum / ideal.len() as f64
        });

        // Build the least-squares system
        // For each point: r_c = r_f × (1 + c1×r_f² + c2×r_f⁴)
        // Rearranging: r_c - r_f = r_f × (c1×r_f² + c2×r_f⁴)
        // This is linear in c1, c2

        let n = ideal.len();
        let mut ata = vec![vec![0.0; num_coefficients]; num_coefficients];
        let mut atb = vec![0.0; num_coefficients];

        for i in 0..n {
            let pf = ideal[i] - c;
            let pc = curved[i] - c;

            let r_f = pf.length();
            let r_c = pc.length();

            if r_f < f64::EPSILON {
                continue;
            }

            // Measurement: (r_c / r_f - 1)
            let measurement = r_c / r_f - 1.0;

            // Design matrix row: [r_f², r_f⁴]
            let r2 = r_f * r_f;
            let r4 = r2 * r2;
            let design_row = [r2, r4];

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

        let c1 = solution.first().copied().unwrap_or(0.0);
        let c2 = solution.get(1).copied().unwrap_or(0.0);

        Some(Self { c1, c2, center: c })
    }

    /// Compute RMS residual error for a set of point correspondences.
    ///
    /// # Arguments
    /// * `ideal` - Ideal (flat) point coordinates
    /// * `curved` - Measured (curved) point coordinates
    ///
    /// # Returns
    /// RMS error in pixels
    pub fn rms_error(&self, ideal: &[DVec2], curved: &[DVec2]) -> f64 {
        debug_assert_eq!(
            ideal.len(),
            curved.len(),
            "Point arrays must have same length"
        );

        if ideal.is_empty() {
            return 0.0;
        }

        let sum_sq: f64 = ideal
            .iter()
            .zip(curved.iter())
            .map(|(&pf, &pc)| {
                let pred = self.apply(pf);
                pred.distance_squared(pc)
            })
            .sum();

        (sum_sq / ideal.len() as f64).sqrt()
    }
}

/// Maximum iterations for Newton-Raphson correction.
const MAX_ITERATIONS: usize = 10;

/// Convergence threshold for Newton-Raphson iteration.
const CONVERGENCE_THRESHOLD: f64 = 1e-10;

/// Solve a symmetric positive definite system using Cholesky decomposition.
#[allow(clippy::needless_range_loop)]
fn solve_symmetric_positive_definite(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    if n == 0 || a.len() != n {
        return None;
    }

    // Cholesky decomposition: A = L × L^T
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

    // Forward substitution: L × y = b
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

    // Backward substitution: L^T × x = y
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
    fn test_identity() {
        let model = FieldCurvature::identity();

        assert!(model.is_identity());
        assert!(!model.is_outward());
        assert!(!model.is_inward());

        let p = DVec2::new(100.0, 200.0);
        let c = model.apply(p);
        assert!((c.x - 100.0).abs() < TOLERANCE);
        assert!((c.y - 200.0).abs() < TOLERANCE);

        let f = model.correct(p);
        assert!((f.x - 100.0).abs() < TOLERANCE);
        assert!((f.y - 200.0).abs() < TOLERANCE);
    }

    #[test]
    fn test_outward_curvature() {
        let center = DVec2::new(512.0, 512.0);
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.00001, center));

        assert!(model.is_outward());
        assert!(!model.is_inward());
        assert!(!model.is_identity());

        // Point at corner should be pushed outward
        let p = DVec2::ZERO;
        let c = model.apply(p);

        let r_orig = p.distance(center);
        let r_curved = c.distance(center);

        assert!(
            r_curved > r_orig,
            "Outward curvature should push corners outward: {} <= {}",
            r_curved,
            r_orig
        );
    }

    #[test]
    fn test_inward_curvature() {
        let center = DVec2::new(512.0, 512.0);
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(-0.000001, center));

        assert!(!model.is_outward());
        assert!(model.is_inward());
        assert!(!model.is_identity());

        // Point at corner should be pulled inward
        let p = DVec2::ZERO;
        let c = model.apply(p);

        let r_orig = p.distance(center);
        let r_curved = c.distance(center);

        assert!(
            r_curved < r_orig,
            "Inward curvature should pull corners inward: {} >= {}",
            r_curved,
            r_orig
        );
    }

    #[test]
    fn test_apply_correct_roundtrip() {
        let model = FieldCurvature::new(FieldCurvatureConfig {
            c1: 1e-8,
            c2: -1e-16,
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
            let c = model.apply(p);
            let f = model.correct(c);

            assert!(
                (f.x - p.x).abs() < 1e-5,
                "Roundtrip failed for {:?}: got {:?}",
                p,
                f
            );
            assert!(
                (f.y - p.y).abs() < 1e-5,
                "Roundtrip failed for {:?}: got {:?}",
                p,
                f
            );
        }
    }

    #[test]
    fn test_center_unchanged() {
        let center = DVec2::new(512.0, 384.0);
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.0001, center));

        let c = model.apply(center);
        assert!((c.x - center.x).abs() < TOLERANCE);
        assert!((c.y - center.y).abs() < TOLERANCE);

        let f = model.correct(center);
        assert!((f.x - center.x).abs() < TOLERANCE);
        assert!((f.y - center.y).abs() < TOLERANCE);
    }

    #[test]
    fn test_radial_symmetry() {
        let center = DVec2::new(500.0, 500.0);
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.00001, center));

        // Points equidistant from center should have same magnitude of effect
        let r = 200.0;
        let angles: [f64; 8] = [0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0];

        let mut effects = Vec::new();

        for &angle in &angles {
            let rad = angle.to_radians();
            let p = center + DVec2::new(r * rad.cos(), r * rad.sin());

            let c = model.apply(p);
            let effect = p.distance(c);
            effects.push(effect);
        }

        // All effects should be equal (within tolerance)
        let first = effects[0];
        for (i, &e) in effects.iter().enumerate() {
            assert!(
                (e - first).abs() < 1e-6,
                "Effect at angle {} differs: {} vs {}",
                angles[i],
                e,
                first
            );
        }
    }

    #[test]
    fn test_effect_increases_with_radius() {
        let center = DVec2::new(500.0, 500.0);
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.00001, center));

        let mut prev_effect = 0.0;

        for r in [100.0, 200.0, 300.0, 400.0, 500.0] {
            let p = DVec2::new(center.x + r, center.y);
            let c = model.apply(p);
            let effect = (c.x - p.x).abs();

            assert!(
                effect > prev_effect,
                "Effect should increase with radius: {} <= {} at r={}",
                effect,
                prev_effect,
                r
            );
            prev_effect = effect;
        }
    }

    #[test]
    fn test_config_constructors() {
        let config1 = FieldCurvatureConfig::simple(0.001, DVec2::new(100.0, 100.0));
        assert_eq!(config1.c1, 0.001);
        assert_eq!(config1.c2, 0.0);
        assert_eq!(config1.center, DVec2::new(100.0, 100.0));

        let config2 = FieldCurvatureConfig::centered(1024, 768, 0.0005);
        assert_eq!(config2.center, DVec2::new(512.0, 384.0));

        let config3 =
            FieldCurvatureConfig::with_coefficients(0.001, -0.0001, DVec2::new(200.0, 200.0));
        assert_eq!(config3.c1, 0.001);
        assert_eq!(config3.c2, -0.0001);
    }

    #[test]
    fn test_from_petzval_radius() {
        // Petzval radius of 1000mm, pixel scale of 0.01 (100 pixels per mm)
        let config =
            FieldCurvatureConfig::from_petzval_radius(1000.0, 0.01, DVec2::new(500.0, 500.0));

        // c1 = 1 / (2 × 1000 × 0.01²) = 1 / (2 × 1000 × 0.0001) = 1 / 0.2 = 5.0
        let expected_c1 = 1.0 / (2.0 * 1000.0 * 0.01 * 0.01);
        assert!(
            (config.c1 - expected_c1).abs() < 1e-10,
            "c1 {} != expected {}",
            config.c1,
            expected_c1
        );
    }

    #[test]
    fn test_apply_points_batch() {
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(
            0.00001,
            DVec2::new(500.0, 500.0),
        ));
        let points = vec![
            DVec2::new(100.0, 100.0),
            DVec2::new(200.0, 200.0),
            DVec2::new(300.0, 300.0),
        ];

        let curved = model.apply_points(&points);

        assert_eq!(curved.len(), points.len());

        for (i, &p) in points.iter().enumerate() {
            let c = model.apply(p);
            assert!((curved[i].x - c.x).abs() < TOLERANCE);
            assert!((curved[i].y - c.y).abs() < TOLERANCE);
        }
    }

    #[test]
    fn test_correct_points_batch() {
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(
            0.00001,
            DVec2::new(500.0, 500.0),
        ));
        let points = vec![
            DVec2::new(100.0, 100.0),
            DVec2::new(200.0, 200.0),
            DVec2::new(300.0, 300.0),
        ];

        let corrected = model.correct_points(&points);

        assert_eq!(corrected.len(), points.len());

        for (i, &p) in points.iter().enumerate() {
            let f = model.correct(p);
            assert!((corrected[i].x - f.x).abs() < TOLERANCE);
            assert!((corrected[i].y - f.y).abs() < TOLERANCE);
        }
    }

    #[test]
    fn test_max_effect() {
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(
            0.00001,
            DVec2::new(512.0, 512.0),
        ));
        let max = model.max_effect(1024, 1024);

        // Max effect should be at corners
        let p = DVec2::ZERO;
        let c = model.apply(p);
        let corner_effect = p.distance(c);

        assert!(
            (max - corner_effect).abs() < 1e-6,
            "Max effect {} should match corner effect {}",
            max,
            corner_effect
        );
    }

    #[test]
    fn test_sag_at() {
        let center = DVec2::new(500.0, 500.0);
        let c1 = 0.00001;
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(c1, center));

        // At center, sag should be zero
        let sag_center = model.sag_at(center);
        assert!(sag_center.abs() < TOLERANCE);

        // At radius r, sag ≈ r² × c1 / 2
        let test_p = DVec2::new(600.0, 500.0);
        let r = test_p.x - center.x;
        let expected_sag = r * r * c1 / 2.0;
        let actual_sag = model.sag_at(test_p);

        assert!(
            (actual_sag - expected_sag).abs() < 1e-10,
            "Sag {} != expected {}",
            actual_sag,
            expected_sag
        );
    }

    #[test]
    fn test_estimate_curvature() {
        let center = DVec2::new(500.0, 500.0);
        let c1_true = 0.00001;
        let model_true = FieldCurvature::new(FieldCurvatureConfig::simple(c1_true, center));

        // Generate synthetic data
        let mut ideal = Vec::new();
        let mut curved = Vec::new();

        for y in (0..=1000).step_by(100) {
            for x in (0..=1000).step_by(100) {
                let pf = DVec2::new(x as f64, y as f64);
                ideal.push(pf);
                curved.push(model_true.apply(pf));
            }
        }

        // Estimate the model
        let model_est = FieldCurvature::estimate(&ideal, &curved, Some(center), 1);

        assert!(model_est.is_some(), "Estimation should succeed");
        let model_est = model_est.unwrap();

        let (c1_est, _) = model_est.coefficients();
        assert!(
            (c1_est - c1_true).abs() < 1e-8,
            "Estimated c1={} should match true c1={}",
            c1_est,
            c1_true
        );
    }

    #[test]
    fn test_estimate_with_two_coefficients() {
        let center = DVec2::new(500.0, 500.0);
        // Use more significant coefficients that are numerically distinguishable
        let model_true = FieldCurvature::new(FieldCurvatureConfig {
            c1: 0.000005,
            c2: -0.00000000001,
            center,
        });

        // Generate synthetic data with finer grid for better conditioning
        let mut ideal = Vec::new();
        let mut curved = Vec::new();

        for y in (0..=1000).step_by(25) {
            for x in (0..=1000).step_by(25) {
                let pf = DVec2::new(x as f64, y as f64);
                ideal.push(pf);
                curved.push(model_true.apply(pf));
            }
        }

        // Estimate with 2 coefficients
        let model_est = FieldCurvature::estimate(&ideal, &curved, Some(center), 2);

        assert!(model_est.is_some(), "Estimation should succeed");
        let model_est = model_est.unwrap();

        // Check RMS error is small (relative to the magnitude of effect)
        let rms = model_est.rms_error(&ideal, &curved);
        // The effect magnitude is small, so we allow a small RMS relative to max displacement
        let max_effect = model_true.max_effect(1000, 1000);
        assert!(
            rms < max_effect * 0.01 || rms < 0.1,
            "RMS error {} should be small (max effect: {})",
            rms,
            max_effect
        );
    }

    #[test]
    fn test_rms_error() {
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(
            0.00001,
            DVec2::new(500.0, 500.0),
        ));

        // Perfect fit should have zero error
        let ideal = vec![
            DVec2::new(100.0, 100.0),
            DVec2::new(200.0, 200.0),
            DVec2::new(300.0, 300.0),
        ];
        let curved: Vec<_> = ideal.iter().map(|&p| model.apply(p)).collect();

        let rms = model.rms_error(&ideal, &curved);
        assert!(rms < TOLERANCE, "Perfect fit should have zero RMS: {}", rms);
    }

    #[test]
    fn test_coefficients_getter() {
        let config = FieldCurvatureConfig {
            c1: 0.001,
            c2: -0.0001,
            center: DVec2::new(100.0, 100.0),
        };
        let model = FieldCurvature::new(config);

        let (c1, c2) = model.coefficients();
        assert_eq!(c1, 0.001);
        assert_eq!(c2, -0.0001);

        let c = model.center();
        assert_eq!(c, DVec2::new(100.0, 100.0));
    }

    #[test]
    fn test_strong_curvature_converges() {
        // Test with moderately strong curvature
        let model = FieldCurvature::new(FieldCurvatureConfig {
            c1: 0.000001,
            c2: 0.0,
            center: DVec2::new(500.0, 500.0),
        });

        let test_points = [
            DVec2::new(0.0, 0.0),
            DVec2::new(100.0, 100.0),
            DVec2::new(900.0, 900.0),
            DVec2::new(1000.0, 1000.0),
        ];

        for &p in &test_points {
            let c = model.apply(p);
            let f = model.correct(c);

            assert!(
                (f.x - p.x).abs() < 0.01,
                "Strong curvature roundtrip failed for {:?}: got {:?}",
                p,
                f
            );
            assert!(
                (f.y - p.y).abs() < 0.01,
                "Strong curvature roundtrip failed for {:?}: got {:?}",
                p,
                f
            );
        }
    }

    #[test]
    fn test_estimate_fails_with_insufficient_points() {
        let ideal = vec![DVec2::new(100.0, 100.0), DVec2::new(200.0, 200.0)];
        let curved = vec![DVec2::new(101.0, 101.0), DVec2::new(202.0, 202.0)];

        let result = FieldCurvature::estimate(&ideal, &curved, None, 1);
        assert!(result.is_none(), "Should fail with insufficient points");
    }

    #[test]
    fn test_estimate_fails_with_mismatched_lengths() {
        let ideal = vec![
            DVec2::new(100.0, 100.0),
            DVec2::new(200.0, 200.0),
            DVec2::new(300.0, 300.0),
        ];
        let curved = vec![DVec2::new(101.0, 101.0)];

        let result = FieldCurvature::estimate(&ideal, &curved, None, 1);
        assert!(result.is_none(), "Should fail with mismatched lengths");
    }

    #[test]
    fn test_estimate_fails_with_invalid_num_coefficients() {
        let ideal = vec![
            DVec2::new(100.0, 100.0),
            DVec2::new(200.0, 200.0),
            DVec2::new(300.0, 300.0),
        ];
        let curved = vec![
            DVec2::new(101.0, 101.0),
            DVec2::new(202.0, 202.0),
            DVec2::new(303.0, 303.0),
        ];

        let result = FieldCurvature::estimate(&ideal, &curved, None, 0);
        assert!(result.is_none(), "Should fail with 0 coefficients");

        let result = FieldCurvature::estimate(&ideal, &curved, None, 3);
        assert!(result.is_none(), "Should fail with 3 coefficients");
    }

    #[test]
    fn test_combined_with_radial_distortion() {
        // Test that field curvature can work alongside radial distortion
        use crate::registration::distortion::RadialDistortion;

        let center = DVec2::new(500.0, 500.0);

        let radial = RadialDistortion::barrel(0.00001, center);
        let curvature = FieldCurvature::new(FieldCurvatureConfig::simple(0.000005, center));

        // Apply both effects
        let p = DVec2::new(300.0, 400.0);

        // Apply radial first, then field curvature
        let r = radial.distort(p);
        let rc = curvature.apply(r);

        // The combined effect should be different from either alone
        let c = curvature.apply(p);

        assert!(
            (rc.x - c.x).abs() > 0.01 || (rc.y - c.y).abs() > 0.01,
            "Combined effect should differ from curvature alone"
        );
        assert!(
            (rc.x - r.x).abs() > 0.01 || (rc.y - r.y).abs() > 0.01,
            "Combined effect should differ from radial alone"
        );
    }

    #[test]
    fn test_negative_c2() {
        // c2 < 0 should work correctly (higher-order correction)
        let model = FieldCurvature::new(FieldCurvatureConfig {
            c1: 0.00001,
            c2: -0.0000000001,
            center: DVec2::new(500.0, 500.0),
        });

        // Roundtrip should still work
        let p = DVec2::new(300.0, 400.0);
        let c = model.apply(p);
        let f = model.correct(c);

        assert!((f.x - 300.0).abs() < 1e-6);
        assert!((f.y - 400.0).abs() < 1e-6);
    }

    #[test]
    fn test_estimate_with_center_auto() {
        // Test estimation with automatic center detection
        let center = DVec2::new(500.0, 500.0);
        let model_true = FieldCurvature::new(FieldCurvatureConfig::simple(0.000005, center));

        // Generate grid centered around the expected center
        let mut ideal = Vec::new();
        let mut curved = Vec::new();

        for y in (100..=900).step_by(100) {
            for x in (100..=900).step_by(100) {
                let pf = DVec2::new(x as f64, y as f64);
                ideal.push(pf);
                curved.push(model_true.apply(pf));
            }
        }

        // Estimate without specifying center
        let model_est = FieldCurvature::estimate(&ideal, &curved, None, 1);
        assert!(model_est.is_some());

        let model_est = model_est.unwrap();

        // RMS error should be small
        let rms = model_est.rms_error(&ideal, &curved);
        assert!(rms < 0.1, "RMS error {} should be small", rms);
    }
}
