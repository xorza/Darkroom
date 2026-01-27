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
//!
//! // Create a model with known curvature
//! let model = FieldCurvature::new(FieldCurvatureConfig {
//!     c1: 0.00001,  // Primary curvature
//!     c2: 0.0,
//!     center: (1024.0, 768.0),
//! });
//!
//! // Apply curvature effect (ideal -> curved)
//! let (x_c, y_c) = model.apply(512.0, 384.0);
//!
//! // Remove curvature effect (curved -> ideal)
//! let (x_f, y_f) = model.correct(x_c, y_c);
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
    pub center: (f64, f64),
}

impl Default for FieldCurvatureConfig {
    fn default() -> Self {
        Self {
            c1: 0.0,
            c2: 0.0,
            center: (0.0, 0.0),
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
    pub fn simple(c1: f64, center: (f64, f64)) -> Self {
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
    pub fn from_petzval_radius(petzval_radius: f64, pixel_scale: f64, center: (f64, f64)) -> Self {
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
            center: (width as f64 / 2.0, height as f64 / 2.0),
        }
    }

    /// Create a config with both curvature coefficients.
    #[inline]
    pub fn with_coefficients(c1: f64, c2: f64, center: (f64, f64)) -> Self {
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
    /// Optical center x coordinate
    center_x: f64,
    /// Optical center y coordinate
    center_y: f64,
}

impl FieldCurvature {
    /// Create a new field curvature model.
    #[inline]
    pub fn new(config: FieldCurvatureConfig) -> Self {
        Self {
            c1: config.c1,
            c2: config.c2,
            center_x: config.center.0,
            center_y: config.center.1,
        }
    }

    /// Create an identity model (no curvature).
    #[inline]
    pub fn identity() -> Self {
        Self {
            c1: 0.0,
            c2: 0.0,
            center_x: 0.0,
            center_y: 0.0,
        }
    }

    /// Get the curvature coefficients.
    #[inline]
    pub fn coefficients(&self) -> (f64, f64) {
        (self.c1, self.c2)
    }

    /// Get the optical center.
    #[inline]
    pub fn center(&self) -> (f64, f64) {
        (self.center_x, self.center_y)
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
    /// * `x` - Ideal x coordinate (pixel)
    /// * `y` - Ideal y coordinate (pixel)
    ///
    /// # Returns
    /// Curved (x, y) coordinates
    #[inline]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let dx = x - self.center_x;
        let dy = y - self.center_y;
        let r_squared = dx * dx + dy * dy;

        let factor = self.scale_factor(r_squared);

        let x_c = self.center_x + dx * factor;
        let y_c = self.center_y + dy * factor;

        (x_c, y_c)
    }

    /// Correct field curvature: curved coordinates -> ideal (flat) coordinates.
    ///
    /// This transforms coordinates from a curved Petzval surface back to
    /// coordinates on an ideal flat focal plane.
    ///
    /// Uses Newton-Raphson iteration for accurate inversion.
    ///
    /// # Arguments
    /// * `x` - Curved x coordinate (pixel)
    /// * `y` - Curved y coordinate (pixel)
    ///
    /// # Returns
    /// Ideal (x, y) coordinates
    pub fn correct(&self, x: f64, y: f64) -> (f64, f64) {
        // Fast path for identity
        if self.is_identity() {
            return (x, y);
        }

        let dx_c = x - self.center_x;
        let dy_c = y - self.center_y;
        let r_c_squared = dx_c * dx_c + dy_c * dy_c;

        // Fast path for center point
        if r_c_squared < f64::EPSILON {
            return (x, y);
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

        let x_f = self.center_x + dx_c * scale;
        let y_f = self.center_y + dy_c * scale;

        (x_f, y_f)
    }

    /// Apply curvature to multiple points.
    #[inline]
    pub fn apply_points(&self, points: &[(f64, f64)]) -> Vec<(f64, f64)> {
        points.iter().map(|&(x, y)| self.apply(x, y)).collect()
    }

    /// Correct curvature for multiple points.
    #[inline]
    pub fn correct_points(&self, points: &[(f64, f64)]) -> Vec<(f64, f64)> {
        points.iter().map(|&(x, y)| self.correct(x, y)).collect()
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
                let (x_c, y_c) = self.apply(x, y);
                let dx = x_c - x;
                let dy = y_c - y;
                (dx * dx + dy * dy).sqrt()
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
    /// * `x` - x coordinate (pixel)
    /// * `y` - y coordinate (pixel)
    ///
    /// # Returns
    /// Sag (defocus) in arbitrary units proportional to pixel displacement
    pub fn sag_at(&self, x: f64, y: f64) -> f64 {
        let dx = x - self.center_x;
        let dy = y - self.center_y;
        let r_squared = dx * dx + dy * dy;

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
        ideal: &[(f64, f64)],
        curved: &[(f64, f64)],
        center: Option<(f64, f64)>,
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
        let (cx, cy) = center.unwrap_or_else(|| {
            let sum: (f64, f64) = ideal
                .iter()
                .fold((0.0, 0.0), |acc, &(x, y)| (acc.0 + x, acc.1 + y));
            let n = ideal.len() as f64;
            (sum.0 / n, sum.1 / n)
        });

        // Build the least-squares system
        // For each point: r_c = r_f × (1 + c1×r_f² + c2×r_f⁴)
        // Rearranging: r_c - r_f = r_f × (c1×r_f² + c2×r_f⁴)
        // This is linear in c1, c2

        let n = ideal.len();
        let mut ata = vec![vec![0.0; num_coefficients]; num_coefficients];
        let mut atb = vec![0.0; num_coefficients];

        for i in 0..n {
            let (x_f, y_f) = ideal[i];
            let (x_c, y_c) = curved[i];

            let dx_f = x_f - cx;
            let dy_f = y_f - cy;
            let dx_c = x_c - cx;
            let dy_c = y_c - cy;

            let r_f = (dx_f * dx_f + dy_f * dy_f).sqrt();
            let r_c = (dx_c * dx_c + dy_c * dy_c).sqrt();

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

        Some(Self {
            c1,
            c2,
            center_x: cx,
            center_y: cy,
        })
    }

    /// Compute RMS residual error for a set of point correspondences.
    ///
    /// # Arguments
    /// * `ideal` - Ideal (flat) point coordinates
    /// * `curved` - Measured (curved) point coordinates
    ///
    /// # Returns
    /// RMS error in pixels
    pub fn rms_error(&self, ideal: &[(f64, f64)], curved: &[(f64, f64)]) -> f64 {
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
            .map(|(&(x_f, y_f), &(x_c, y_c))| {
                let (pred_x, pred_y) = self.apply(x_f, y_f);
                let dx = pred_x - x_c;
                let dy = pred_y - y_c;
                dx * dx + dy * dy
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

        let (x_c, y_c) = model.apply(100.0, 200.0);
        assert!((x_c - 100.0).abs() < TOLERANCE);
        assert!((y_c - 200.0).abs() < TOLERANCE);

        let (x_f, y_f) = model.correct(100.0, 200.0);
        assert!((x_f - 100.0).abs() < TOLERANCE);
        assert!((y_f - 200.0).abs() < TOLERANCE);
    }

    #[test]
    fn test_outward_curvature() {
        let center = (512.0, 512.0);
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.00001, center));

        assert!(model.is_outward());
        assert!(!model.is_inward());
        assert!(!model.is_identity());

        // Point at corner should be pushed outward
        let (x_c, y_c) = model.apply(0.0, 0.0);

        let r_orig = ((0.0 - center.0).powi(2) + (0.0 - center.1).powi(2)).sqrt();
        let r_curved = ((x_c - center.0).powi(2) + (y_c - center.1).powi(2)).sqrt();

        assert!(
            r_curved > r_orig,
            "Outward curvature should push corners outward: {} <= {}",
            r_curved,
            r_orig
        );
    }

    #[test]
    fn test_inward_curvature() {
        let center = (512.0, 512.0);
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(-0.000001, center));

        assert!(!model.is_outward());
        assert!(model.is_inward());
        assert!(!model.is_identity());

        // Point at corner should be pulled inward
        let (x_c, y_c) = model.apply(0.0, 0.0);

        let r_orig = ((0.0 - center.0).powi(2) + (0.0 - center.1).powi(2)).sqrt();
        let r_curved = ((x_c - center.0).powi(2) + (y_c - center.1).powi(2)).sqrt();

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
            let (x_c, y_c) = model.apply(x, y);
            let (x_f, y_f) = model.correct(x_c, y_c);

            assert!(
                (x_f - x).abs() < 1e-5,
                "Roundtrip failed for ({}, {}): got ({}, {})",
                x,
                y,
                x_f,
                y_f
            );
            assert!(
                (y_f - y).abs() < 1e-5,
                "Roundtrip failed for ({}, {}): got ({}, {})",
                x,
                y,
                x_f,
                y_f
            );
        }
    }

    #[test]
    fn test_center_unchanged() {
        let center = (512.0, 384.0);
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.0001, center));

        let (x_c, y_c) = model.apply(center.0, center.1);
        assert!((x_c - center.0).abs() < TOLERANCE);
        assert!((y_c - center.1).abs() < TOLERANCE);

        let (x_f, y_f) = model.correct(center.0, center.1);
        assert!((x_f - center.0).abs() < TOLERANCE);
        assert!((y_f - center.1).abs() < TOLERANCE);
    }

    #[test]
    fn test_radial_symmetry() {
        let center = (500.0, 500.0);
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.00001, center));

        // Points equidistant from center should have same magnitude of effect
        let r = 200.0;
        let angles: [f64; 8] = [0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0];

        let mut effects = Vec::new();

        for &angle in &angles {
            let rad = angle.to_radians();
            let x = center.0 + r * rad.cos();
            let y = center.1 + r * rad.sin();

            let (x_c, y_c) = model.apply(x, y);
            let effect = ((x_c - x).powi(2) + (y_c - y).powi(2)).sqrt();
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
        let center = (500.0, 500.0);
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.00001, center));

        let mut prev_effect = 0.0;

        for r in [100.0, 200.0, 300.0, 400.0, 500.0] {
            let x = center.0 + r;
            let y = center.1;

            let (x_c, _) = model.apply(x, y);
            let effect = (x_c - x).abs();

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
        let config1 = FieldCurvatureConfig::simple(0.001, (100.0, 100.0));
        assert_eq!(config1.c1, 0.001);
        assert_eq!(config1.c2, 0.0);
        assert_eq!(config1.center, (100.0, 100.0));

        let config2 = FieldCurvatureConfig::centered(1024, 768, 0.0005);
        assert_eq!(config2.center, (512.0, 384.0));

        let config3 = FieldCurvatureConfig::with_coefficients(0.001, -0.0001, (200.0, 200.0));
        assert_eq!(config3.c1, 0.001);
        assert_eq!(config3.c2, -0.0001);
    }

    #[test]
    fn test_from_petzval_radius() {
        // Petzval radius of 1000mm, pixel scale of 0.01 (100 pixels per mm)
        let config = FieldCurvatureConfig::from_petzval_radius(1000.0, 0.01, (500.0, 500.0));

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
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.00001, (500.0, 500.0)));
        let points = vec![(100.0, 100.0), (200.0, 200.0), (300.0, 300.0)];

        let curved = model.apply_points(&points);

        assert_eq!(curved.len(), points.len());

        for (i, &(x, y)) in points.iter().enumerate() {
            let (x_c, y_c) = model.apply(x, y);
            assert!((curved[i].0 - x_c).abs() < TOLERANCE);
            assert!((curved[i].1 - y_c).abs() < TOLERANCE);
        }
    }

    #[test]
    fn test_correct_points_batch() {
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.00001, (500.0, 500.0)));
        let points = vec![(100.0, 100.0), (200.0, 200.0), (300.0, 300.0)];

        let corrected = model.correct_points(&points);

        assert_eq!(corrected.len(), points.len());

        for (i, &(x, y)) in points.iter().enumerate() {
            let (x_f, y_f) = model.correct(x, y);
            assert!((corrected[i].0 - x_f).abs() < TOLERANCE);
            assert!((corrected[i].1 - y_f).abs() < TOLERANCE);
        }
    }

    #[test]
    fn test_max_effect() {
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.00001, (512.0, 512.0)));
        let max = model.max_effect(1024, 1024);

        // Max effect should be at corners
        let (x_c, y_c) = model.apply(0.0, 0.0);
        let corner_effect = ((x_c - 0.0).powi(2) + (y_c - 0.0).powi(2)).sqrt();

        assert!(
            (max - corner_effect).abs() < 1e-6,
            "Max effect {} should match corner effect {}",
            max,
            corner_effect
        );
    }

    #[test]
    fn test_sag_at() {
        let center = (500.0, 500.0);
        let c1 = 0.00001;
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(c1, center));

        // At center, sag should be zero
        let sag_center = model.sag_at(center.0, center.1);
        assert!(sag_center.abs() < TOLERANCE);

        // At radius r, sag ≈ r² × c1 / 2
        let test_x = 600.0;
        let test_y = 500.0;
        let r = test_x - center.0;
        let expected_sag = r * r * c1 / 2.0;
        let actual_sag = model.sag_at(test_x, test_y);

        assert!(
            (actual_sag - expected_sag).abs() < 1e-10,
            "Sag {} != expected {}",
            actual_sag,
            expected_sag
        );
    }

    #[test]
    fn test_estimate_curvature() {
        let center = (500.0, 500.0);
        let c1_true = 0.00001;
        let model_true = FieldCurvature::new(FieldCurvatureConfig::simple(c1_true, center));

        // Generate synthetic data
        let mut ideal = Vec::new();
        let mut curved = Vec::new();

        for y in (0..=1000).step_by(100) {
            for x in (0..=1000).step_by(100) {
                let xf = x as f64;
                let yf = y as f64;
                ideal.push((xf, yf));
                curved.push(model_true.apply(xf, yf));
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
        let center = (500.0, 500.0);
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
                let xf = x as f64;
                let yf = y as f64;
                ideal.push((xf, yf));
                curved.push(model_true.apply(xf, yf));
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
        let model = FieldCurvature::new(FieldCurvatureConfig::simple(0.00001, (500.0, 500.0)));

        // Perfect fit should have zero error
        let ideal = vec![(100.0, 100.0), (200.0, 200.0), (300.0, 300.0)];
        let curved: Vec<_> = ideal.iter().map(|&(x, y)| model.apply(x, y)).collect();

        let rms = model.rms_error(&ideal, &curved);
        assert!(rms < TOLERANCE, "Perfect fit should have zero RMS: {}", rms);
    }

    #[test]
    fn test_coefficients_getter() {
        let config = FieldCurvatureConfig {
            c1: 0.001,
            c2: -0.0001,
            center: (100.0, 100.0),
        };
        let model = FieldCurvature::new(config);

        let (c1, c2) = model.coefficients();
        assert_eq!(c1, 0.001);
        assert_eq!(c2, -0.0001);

        let (cx, cy) = model.center();
        assert_eq!(cx, 100.0);
        assert_eq!(cy, 100.0);
    }

    #[test]
    fn test_strong_curvature_converges() {
        // Test with moderately strong curvature
        let model = FieldCurvature::new(FieldCurvatureConfig {
            c1: 0.000001,
            c2: 0.0,
            center: (500.0, 500.0),
        });

        let test_points = [(0.0, 0.0), (100.0, 100.0), (900.0, 900.0), (1000.0, 1000.0)];

        for &(x, y) in &test_points {
            let (x_c, y_c) = model.apply(x, y);
            let (x_f, y_f) = model.correct(x_c, y_c);

            assert!(
                (x_f - x).abs() < 0.01,
                "Strong curvature roundtrip failed for ({}, {}): got ({}, {})",
                x,
                y,
                x_f,
                y_f
            );
            assert!(
                (y_f - y).abs() < 0.01,
                "Strong curvature roundtrip failed for ({}, {}): got ({}, {})",
                x,
                y,
                x_f,
                y_f
            );
        }
    }

    #[test]
    fn test_estimate_fails_with_insufficient_points() {
        let ideal = vec![(100.0, 100.0), (200.0, 200.0)];
        let curved = vec![(101.0, 101.0), (202.0, 202.0)];

        let result = FieldCurvature::estimate(&ideal, &curved, None, 1);
        assert!(result.is_none(), "Should fail with insufficient points");
    }

    #[test]
    fn test_estimate_fails_with_mismatched_lengths() {
        let ideal = vec![(100.0, 100.0), (200.0, 200.0), (300.0, 300.0)];
        let curved = vec![(101.0, 101.0)];

        let result = FieldCurvature::estimate(&ideal, &curved, None, 1);
        assert!(result.is_none(), "Should fail with mismatched lengths");
    }

    #[test]
    fn test_estimate_fails_with_invalid_num_coefficients() {
        let ideal = vec![(100.0, 100.0), (200.0, 200.0), (300.0, 300.0)];
        let curved = vec![(101.0, 101.0), (202.0, 202.0), (303.0, 303.0)];

        let result = FieldCurvature::estimate(&ideal, &curved, None, 0);
        assert!(result.is_none(), "Should fail with 0 coefficients");

        let result = FieldCurvature::estimate(&ideal, &curved, None, 3);
        assert!(result.is_none(), "Should fail with 3 coefficients");
    }

    #[test]
    fn test_combined_with_radial_distortion() {
        // Test that field curvature can work alongside radial distortion
        use crate::registration::distortion::RadialDistortion;

        let center = (500.0, 500.0);

        let radial = RadialDistortion::barrel(0.00001, center);
        let curvature = FieldCurvature::new(FieldCurvatureConfig::simple(0.000005, center));

        // Apply both effects
        let x = 300.0;
        let y = 400.0;

        // Apply radial first, then field curvature
        let (x_r, y_r) = radial.distort(x, y);
        let (x_rc, y_rc) = curvature.apply(x_r, y_r);

        // The combined effect should be different from either alone
        let (x_c, y_c) = curvature.apply(x, y);

        assert!(
            (x_rc - x_c).abs() > 0.01 || (y_rc - y_c).abs() > 0.01,
            "Combined effect should differ from curvature alone"
        );
        assert!(
            (x_rc - x_r).abs() > 0.01 || (y_rc - y_r).abs() > 0.01,
            "Combined effect should differ from radial alone"
        );
    }

    #[test]
    fn test_negative_c2() {
        // c2 < 0 should work correctly (higher-order correction)
        let model = FieldCurvature::new(FieldCurvatureConfig {
            c1: 0.00001,
            c2: -0.0000000001,
            center: (500.0, 500.0),
        });

        // Roundtrip should still work
        let (x_c, y_c) = model.apply(300.0, 400.0);
        let (x_f, y_f) = model.correct(x_c, y_c);

        assert!((x_f - 300.0).abs() < 1e-6);
        assert!((y_f - 400.0).abs() < 1e-6);
    }

    #[test]
    fn test_estimate_with_center_auto() {
        // Test estimation with automatic center detection
        let center = (500.0, 500.0);
        let model_true = FieldCurvature::new(FieldCurvatureConfig::simple(0.000005, center));

        // Generate grid centered around the expected center
        let mut ideal = Vec::new();
        let mut curved = Vec::new();

        for y in (100..=900).step_by(100) {
            for x in (100..=900).step_by(100) {
                let xf = x as f64;
                let yf = y as f64;
                ideal.push((xf, yf));
                curved.push(model_true.apply(xf, yf));
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
