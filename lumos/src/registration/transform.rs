//! Transformation matrix for image registration.

use glam::DVec2;

use crate::math::DMat3;
use crate::registration::distortion::SipPolynomial;

/// Supported transformation models with increasing degrees of freedom.
///
/// Variants are ordered by complexity (used for `compose()` to pick the
/// more complex type). `Auto` is a pipeline-level directive that resolves
/// to a concrete type at runtime — it must not reach RANSAC or transform
/// estimation directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransformType {
    /// Translation only (2 DOF: dx, dy)
    Translation,
    /// Translation + Rotation (3 DOF: dx, dy, angle)
    Euclidean,
    /// Translation + Rotation + Uniform Scale (4 DOF)
    Similarity,
    /// Full affine (6 DOF: handles differential scaling and shear)
    Affine,
    /// Projective/Homography (8 DOF: handles perspective)
    Homography,
    /// Automatic model selection: starts with Similarity, upgrades to
    /// Homography if residuals exceed the threshold. Resolved by the
    /// pipeline before RANSAC estimation.
    #[default]
    Auto,
}

impl TransformType {
    /// Minimum number of point correspondences required to estimate this transform.
    pub fn min_points(&self) -> usize {
        match self {
            TransformType::Translation => 1,
            TransformType::Euclidean => 2,
            TransformType::Similarity => 2,
            TransformType::Affine => 3,
            TransformType::Homography => 4,
            TransformType::Auto => TransformType::Similarity.min_points(),
        }
    }

    /// Degrees of freedom for this transformation.
    pub fn degrees_of_freedom(&self) -> usize {
        match self {
            TransformType::Translation => 2,
            TransformType::Euclidean => 3,
            TransformType::Similarity => 4,
            TransformType::Affine => 6,
            TransformType::Homography => 8,
            TransformType::Auto => {
                panic!("Auto must be resolved to a concrete type before querying DOF")
            }
        }
    }
}

/// 3x3 homogeneous transformation matrix.
///
/// Stored as a row-major [`DMat3`]:
/// ```text
/// | a  b  tx |   | m[0] m[1] m[2] |
/// | c  d  ty | = | m[3] m[4] m[5] |
/// | g  h  1  |   | m[6] m[7] m[8] |
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Transform {
    /// Row-major 3x3 matrix.
    pub matrix: DMat3,
    /// The type of transformation this matrix represents.
    pub transform_type: TransformType,
}

impl Default for Transform {
    fn default() -> Self {
        Self::identity()
    }
}

impl std::fmt::Display for Transform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let t = self.translation_components();
        let rotation_deg = self.rotation_angle().to_degrees();
        let scale = self.scale_factor();

        match self.transform_type {
            TransformType::Translation => {
                write!(f, "Translation(dx={:.2}, dy={:.2})", t.x, t.y)
            }
            TransformType::Euclidean => {
                write!(
                    f,
                    "Euclidean(dx={:.2}, dy={:.2}, rot={:.3}°)",
                    t.x, t.y, rotation_deg
                )
            }
            TransformType::Similarity => {
                write!(
                    f,
                    "Similarity(dx={:.2}, dy={:.2}, rot={:.3}°, scale={:.4})",
                    t.x, t.y, rotation_deg, scale
                )
            }
            TransformType::Affine => {
                write!(
                    f,
                    "Affine(dx={:.2}, dy={:.2}, rot={:.3}°, scale={:.4})",
                    t.x, t.y, rotation_deg, scale
                )
            }
            TransformType::Homography => {
                write!(
                    f,
                    "Homography(dx={:.2}, dy={:.2}, rot={:.3}°, scale={:.4})",
                    t.x, t.y, rotation_deg, scale
                )
            }
            TransformType::Auto => {
                panic!("Auto should be resolved before creating a Transform")
            }
        }
    }
}

impl Transform {
    /// Create identity transform.
    pub fn identity() -> Self {
        Self {
            matrix: DMat3::identity(),
            transform_type: TransformType::Translation,
        }
    }

    /// Create translation transform.
    pub fn translation(t: DVec2) -> Self {
        Self {
            matrix: DMat3::from_array([1.0, 0.0, t.x, 0.0, 1.0, t.y, 0.0, 0.0, 1.0]),
            transform_type: TransformType::Translation,
        }
    }

    /// Create Euclidean transform (translation + rotation).
    pub fn euclidean(t: DVec2, angle: f64) -> Self {
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        Self {
            matrix: DMat3::from_array([cos_a, -sin_a, t.x, sin_a, cos_a, t.y, 0.0, 0.0, 1.0]),
            transform_type: TransformType::Euclidean,
        }
    }

    /// Create similarity transform (translation + rotation + uniform scale).
    pub fn similarity(t: DVec2, angle: f64, scale: f64) -> Self {
        let cos_a = angle.cos() * scale;
        let sin_a = angle.sin() * scale;
        Self {
            matrix: DMat3::from_array([cos_a, -sin_a, t.x, sin_a, cos_a, t.y, 0.0, 0.0, 1.0]),
            transform_type: TransformType::Similarity,
        }
    }

    /// Create affine transform from 6 parameters [a, b, tx, c, d, ty].
    pub fn affine(params: [f64; 6]) -> Self {
        Self {
            matrix: DMat3::from_array([
                params[0], params[1], params[2], params[3], params[4], params[5], 0.0, 0.0, 1.0,
            ]),
            transform_type: TransformType::Affine,
        }
    }

    /// Create homography from 8 parameters (9th element is 1.0).
    pub fn homography(params: [f64; 8]) -> Self {
        Self {
            matrix: DMat3::from_array([
                params[0], params[1], params[2], params[3], params[4], params[5], params[6],
                params[7], 1.0,
            ]),
            transform_type: TransformType::Homography,
        }
    }

    /// Create scale transform.
    pub fn scale(s: DVec2) -> Self {
        Self {
            matrix: DMat3::from_array([s.x, 0.0, 0.0, 0.0, s.y, 0.0, 0.0, 0.0, 1.0]),
            transform_type: TransformType::Affine,
        }
    }

    /// Create rotation transform around a specified center point.
    pub fn rotation_around(center: DVec2, angle: f64) -> Self {
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        // T(-cx,-cy) * R(angle) * T(cx,cy)
        let tx = center.x - cos_a * center.x + sin_a * center.y;
        let ty = center.y - sin_a * center.x - cos_a * center.y;
        Self {
            matrix: DMat3::from_array([cos_a, -sin_a, tx, sin_a, cos_a, ty, 0.0, 0.0, 1.0]),
            transform_type: TransformType::Euclidean,
        }
    }

    /// Create transform from a [`DMat3`] matrix.
    pub fn from_matrix(matrix: DMat3, transform_type: TransformType) -> Self {
        Self {
            matrix,
            transform_type,
        }
    }

    /// Apply transform to map a point from REFERENCE coordinates to TARGET coordinates.
    ///
    /// Given a transform T estimated from `register_stars(ref_stars, target_stars)`:
    /// - `T.apply(ref_point)` gives the corresponding target point
    /// - `T.apply_inverse(target_point)` gives the corresponding reference point
    ///
    /// # Image Warping
    ///
    /// To align a target image to the reference frame (so it overlays correctly
    /// with the reference), you need to sample the target image at positions
    /// mapped from reference coordinates. This means using `apply()` to find
    /// where each reference pixel maps to in the target, then sampling there.
    ///
    /// The `warp_image` function handles this automatically.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let result = registrator.register_stars(&ref_stars, &target_stars)?;
    /// let transform = result.transform;
    ///
    /// // Map a reference point to its corresponding target location
    /// let target_pos = transform.apply(ref_pos);
    ///
    /// // Map a target point back to reference coordinates
    /// let ref_pos = transform.apply_inverse(target_pos);
    /// ```
    pub fn apply(&self, p: DVec2) -> DVec2 {
        self.matrix.transform_point(p)
    }

    /// Apply inverse transform to map a point from TARGET coordinates to REFERENCE coordinates.
    ///
    /// This is the inverse of `apply()`. Given a point in the target image,
    /// it returns the corresponding point in the reference image.
    ///
    /// See [`apply`](Self::apply) for more details on transform direction.
    pub fn apply_inverse(&self, p: DVec2) -> DVec2 {
        self.inverse().apply(p)
    }

    /// Compute matrix inverse.
    ///
    /// # Panics
    /// Panics if the matrix is singular (determinant near zero).
    pub fn inverse(&self) -> Self {
        let inv = self
            .matrix
            .inverse()
            .expect("Cannot invert singular transform matrix");
        Self {
            matrix: inv,
            transform_type: self.transform_type,
        }
    }

    /// Compose two transforms: self * other (apply other first, then self).
    pub fn compose(&self, other: &Self) -> Self {
        // Result type is the more complex of the two
        let transform_type = if self.transform_type as u8 > other.transform_type as u8 {
            self.transform_type
        } else {
            other.transform_type
        };

        Self {
            matrix: self.matrix.mul_mat(&other.matrix),
            transform_type,
        }
    }

    /// Extract translation components as DVec2.
    pub fn translation_components(&self) -> DVec2 {
        DVec2::new(self.matrix[2], self.matrix[5])
    }

    /// Extract rotation angle in radians (valid for Euclidean/Similarity transforms).
    pub fn rotation_angle(&self) -> f64 {
        self.matrix[3].atan2(self.matrix[0])
    }

    /// Extract scale factor (valid for Similarity transforms).
    pub fn scale_factor(&self) -> f64 {
        let a = self.matrix[0];
        let c = self.matrix[3];
        (a * a + c * c).sqrt()
    }

    /// Check if this is a valid (non-degenerate) transformation.
    pub fn is_valid(&self) -> bool {
        let det = self.matrix[0] * self.matrix[4] - self.matrix[1] * self.matrix[3];
        det.abs() > 1e-10 && det.is_finite()
    }

    /// Compute Frobenius norm of difference from identity.
    pub fn deviation_from_identity(&self) -> f64 {
        self.matrix.deviation_from_identity()
    }
}

/// Combined transform + optional SIP distortion correction for warping.
///
/// Bundles a linear `Transform` with an optional `SipPolynomial` so that
/// callers of `warp()` cannot forget to include the SIP correction.
/// For each output pixel `p`, the source coordinate is:
/// `src = transform.apply(sip.correct(p))` when SIP is present,
/// or `src = transform.apply(p)` otherwise.
#[derive(Debug, Clone)]
pub struct WarpTransform {
    pub transform: Transform,
    pub sip: Option<SipPolynomial>,
}

impl WarpTransform {
    /// Create a warp transform with no SIP correction.
    pub fn new(transform: Transform) -> Self {
        Self {
            transform,
            sip: None,
        }
    }

    /// Create a warp transform with SIP distortion correction.
    pub fn with_sip(transform: Transform, sip: SipPolynomial) -> Self {
        Self {
            transform,
            sip: Some(sip),
        }
    }

    /// Compute the source coordinate for a given output pixel position.
    pub fn apply(&self, p: DVec2) -> DVec2 {
        let corrected = match &self.sip {
            Some(sip) => sip.correct(p),
            None => p,
        };
        self.transform.apply(corrected)
    }

    /// Whether this transform has a nonlinear SIP component.
    pub fn has_sip(&self) -> bool {
        self.sip.is_some()
    }

    /// Whether this transform is purely linear (affine or simpler, no SIP).
    /// When true, incremental stepping and SIMD can be used.
    pub fn is_linear(&self) -> bool {
        self.sip.is_none() && !matches!(self.transform.transform_type, TransformType::Homography)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    const EPSILON: f64 = 1e-10;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < EPSILON
    }

    #[test]
    fn test_transform_type_min_points() {
        assert_eq!(TransformType::Translation.min_points(), 1);
        assert_eq!(TransformType::Euclidean.min_points(), 2);
        assert_eq!(TransformType::Similarity.min_points(), 2);
        assert_eq!(TransformType::Affine.min_points(), 3);
        assert_eq!(TransformType::Homography.min_points(), 4);
    }

    #[test]
    fn test_identity_transform() {
        let t = Transform::identity();
        let p = t.apply(DVec2::new(5.0, 7.0));
        assert!(approx_eq(p.x, 5.0));
        assert!(approx_eq(p.y, 7.0));
    }

    #[test]
    fn test_translation_transform() {
        let t = Transform::translation(DVec2::new(10.0, -5.0));
        let p = t.apply(DVec2::new(3.0, 4.0));
        assert!(approx_eq(p.x, 13.0));
        assert!(approx_eq(p.y, -1.0));
    }

    #[test]
    fn test_rotation_90_degrees() {
        let t = Transform::euclidean(DVec2::ZERO, PI / 2.0);
        let p = t.apply(DVec2::new(1.0, 0.0));
        assert!(approx_eq(p.x, 0.0));
        assert!(approx_eq(p.y, 1.0));
    }

    #[test]
    fn test_rotation_180_degrees() {
        let t = Transform::euclidean(DVec2::ZERO, PI);
        let p = t.apply(DVec2::new(1.0, 0.0));
        assert!(approx_eq(p.x, -1.0));
        assert!(approx_eq(p.y, 0.0));
    }

    #[test]
    fn test_scale_transform() {
        let t = Transform::similarity(DVec2::ZERO, 0.0, 2.0);
        let p = t.apply(DVec2::new(3.0, 4.0));
        assert!(approx_eq(p.x, 6.0));
        assert!(approx_eq(p.y, 8.0));
    }

    #[test]
    fn test_similarity_with_rotation_and_scale() {
        let t = Transform::similarity(DVec2::new(5.0, 10.0), PI / 2.0, 2.0);
        let p = t.apply(DVec2::new(1.0, 0.0));
        // Rotate 90° then scale 2x: (1,0) -> (0,1) -> (0,2), then translate
        assert!(approx_eq(p.x, 5.0));
        assert!(approx_eq(p.y, 12.0));
    }

    #[test]
    fn test_affine_transform() {
        // Shear transform
        let t = Transform::affine([1.0, 0.5, 0.0, 0.0, 1.0, 0.0]);
        let p = t.apply(DVec2::new(2.0, 2.0));
        assert!(approx_eq(p.x, 3.0)); // 2 + 0.5*2
        assert!(approx_eq(p.y, 2.0));
    }

    #[test]
    fn test_translation_components() {
        let t = Transform::translation(DVec2::new(7.0, -3.0));
        let tc = t.translation_components();
        assert!(approx_eq(tc.x, 7.0));
        assert!(approx_eq(tc.y, -3.0));
    }

    #[test]
    fn test_rotation_angle() {
        let angle = 0.5;
        let t = Transform::euclidean(DVec2::ZERO, angle);
        assert!(approx_eq(t.rotation_angle(), angle));
    }

    #[test]
    fn test_scale_factor() {
        let scale = 2.5;
        let t = Transform::similarity(DVec2::ZERO, 0.0, scale);
        assert!(approx_eq(t.scale_factor(), scale));
    }

    #[test]
    fn test_is_valid() {
        let valid = Transform::similarity(DVec2::new(1.0, 2.0), 0.5, 1.5);
        assert!(valid.is_valid());

        // Degenerate matrix (zero scale)
        let degenerate = Transform::from_matrix(
            DMat3::from_array([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
            TransformType::Affine,
        );
        assert!(!degenerate.is_valid());
    }

    #[test]
    fn test_homography_transform() {
        // Simple homography that acts like translation
        let t = Transform::homography([1.0, 0.0, 5.0, 0.0, 1.0, 3.0, 0.0, 0.0]);
        let p = t.apply(DVec2::new(2.0, 4.0));
        assert!(approx_eq(p.x, 7.0));
        assert!(approx_eq(p.y, 7.0));
    }

    #[test]
    fn test_homography_perspective() {
        // Homography with perspective component
        let t = Transform::homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.001, 0.0]);
        let p = t.apply(DVec2::new(100.0, 0.0));
        // w = 0.001 * 100 + 1 = 1.1
        // x' = 100 / 1.1 ≈ 90.9
        assert!((p.x - 90.909).abs() < 0.01);
        assert!(approx_eq(p.y, 0.0));
    }

    // ========================================================================
    // WarpTransform tests
    // ========================================================================

    #[test]
    fn test_warp_transform_new() {
        let t = Transform::translation(DVec2::new(10.0, 5.0));
        let wt = WarpTransform::new(t);
        assert!(!wt.has_sip());
        assert!(wt.is_linear());

        let p = wt.apply(DVec2::new(1.0, 2.0));
        assert!(approx_eq(p.x, 11.0));
        assert!(approx_eq(p.y, 7.0));
    }

    #[test]
    fn test_warp_transform_with_sip() {
        use crate::registration::distortion::{SipConfig, SipPolynomial};

        let transform = Transform::identity();

        // Create a simple SIP from synthetic points with barrel distortion
        let cx = 50.0;
        let cy = 50.0;
        let k = 1e-4;
        let mut ref_pts = Vec::new();
        let mut tgt_pts = Vec::new();
        for gy in 0..10 {
            for gx in 0..10 {
                let rx = 5.0 + gx as f64 * 10.0;
                let ry = 5.0 + gy as f64 * 10.0;
                let dx = rx - cx;
                let dy = ry - cy;
                let r2 = dx * dx + dy * dy;
                ref_pts.push(DVec2::new(rx, ry));
                tgt_pts.push(DVec2::new(rx + k * dx * r2, ry + k * dy * r2));
            }
        }
        let sip_config = SipConfig {
            order: 3,
            reference_point: Some(DVec2::new(cx, cy)),
            ..Default::default()
        };
        let sip = SipPolynomial::fit_from_transform(&ref_pts, &tgt_pts, &transform, &sip_config)
            .unwrap()
            .polynomial;

        let wt = WarpTransform::with_sip(transform, sip);
        assert!(wt.has_sip());
        assert!(!wt.is_linear());

        // Corner point should differ from identity
        let corner = DVec2::new(0.0, 0.0);
        let result = wt.apply(corner);
        let no_sip = WarpTransform::new(transform).apply(corner);
        assert!(
            (result - no_sip).length() > 0.01,
            "SIP should produce different coordinates"
        );
    }

    #[test]
    fn test_warp_transform_is_linear() {
        // Translation: linear
        let wt = WarpTransform::new(Transform::translation(DVec2::new(1.0, 2.0)));
        assert!(wt.is_linear());

        // Euclidean: linear
        let wt = WarpTransform::new(Transform::euclidean(DVec2::ZERO, 0.1));
        assert!(wt.is_linear());

        // Similarity: linear
        let wt = WarpTransform::new(Transform::similarity(DVec2::ZERO, 0.1, 1.02));
        assert!(wt.is_linear());

        // Affine: linear
        let wt = WarpTransform::new(Transform::affine([1.0, 0.0, 5.0, 0.0, 1.0, 3.0]));
        assert!(wt.is_linear());

        // Homography: not linear
        let wt = WarpTransform::new(Transform::homography([
            1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.001, 0.0,
        ]));
        assert!(!wt.is_linear());
    }

    #[test]
    fn test_warp_transform_apply_no_sip_matches_transform() {
        let t = Transform::similarity(DVec2::new(3.0, -2.0), 0.5, 1.1);
        let wt = WarpTransform::new(t);

        for &p in &[
            DVec2::new(0.0, 0.0),
            DVec2::new(100.0, 50.0),
            DVec2::new(-10.0, 200.0),
        ] {
            let from_wt = wt.apply(p);
            let from_t = t.apply(p);
            assert!(approx_eq(from_wt.x, from_t.x));
            assert!(approx_eq(from_wt.y, from_t.y));
        }
    }

    // ========================================================================
    // TransformType additional tests
    // ========================================================================

    #[test]
    fn test_transform_type_degrees_of_freedom() {
        assert_eq!(TransformType::Translation.degrees_of_freedom(), 2);
        assert_eq!(TransformType::Euclidean.degrees_of_freedom(), 3);
        assert_eq!(TransformType::Similarity.degrees_of_freedom(), 4);
        assert_eq!(TransformType::Affine.degrees_of_freedom(), 6);
        assert_eq!(TransformType::Homography.degrees_of_freedom(), 8);
    }

    #[test]
    fn test_auto_min_points_delegates_to_similarity() {
        // Auto delegates to Similarity for min_points
        assert_eq!(
            TransformType::Auto.min_points(),
            TransformType::Similarity.min_points()
        );
        assert_eq!(TransformType::Auto.min_points(), 2);
    }

    #[test]
    #[should_panic(expected = "Auto must be resolved")]
    fn test_auto_degrees_of_freedom_panics() {
        TransformType::Auto.degrees_of_freedom();
    }

    // ========================================================================
    // Transform constructor and method tests
    // ========================================================================

    #[test]
    fn test_default_is_identity() {
        let t = Transform::default();
        let p = t.apply(DVec2::new(42.0, -17.0));
        // Identity: output == input
        assert!(approx_eq(p.x, 42.0));
        assert!(approx_eq(p.y, -17.0));
        assert_eq!(t.transform_type, TransformType::Translation);
    }

    #[test]
    fn test_scale_constructor() {
        // Transform::scale(DVec2::new(sx, sy)) creates a diagonal scaling matrix
        // Matrix: [sx, 0, 0, 0, sy, 0, 0, 0, 1]
        // (3.0, 4.0) -> (3.0*2.0, 4.0*0.5) = (6.0, 2.0)
        let t = Transform::scale(DVec2::new(2.0, 0.5));
        let p = t.apply(DVec2::new(3.0, 4.0));
        assert!(approx_eq(p.x, 6.0));
        assert!(approx_eq(p.y, 2.0));
        assert_eq!(t.transform_type, TransformType::Affine);
    }

    #[test]
    fn test_rotation_around_center() {
        // Rotate 90 degrees around center (100, 100)
        // Point (150, 100) is 50 units right of center
        // After 90° CCW rotation: should be 50 units above center -> (100, 150)
        //
        // Math: T(-cx,-cy) * R(90°) * T(cx,cy)
        // Relative to center: (50, 0)
        // After R(90°): (0, 50)
        // Back to absolute: (100, 150)
        let center = DVec2::new(100.0, 100.0);
        let t = Transform::rotation_around(center, PI / 2.0);

        let p = t.apply(DVec2::new(150.0, 100.0));
        assert!((p.x - 100.0).abs() < EPSILON, "Expected x=100, got {}", p.x);
        assert!((p.y - 150.0).abs() < EPSILON, "Expected y=150, got {}", p.y);

        // The center itself should remain fixed
        let center_mapped = t.apply(center);
        assert!(
            (center_mapped.x - 100.0).abs() < EPSILON,
            "Center should be fixed, got x={}",
            center_mapped.x
        );
        assert!(
            (center_mapped.y - 100.0).abs() < EPSILON,
            "Center should be fixed, got y={}",
            center_mapped.y
        );
    }

    #[test]
    fn test_inverse_roundtrip_translation() {
        // T(10, -5) then T^{-1} should give back the original point
        let t = Transform::translation(DVec2::new(10.0, -5.0));
        let p = DVec2::new(3.0, 7.0);

        // apply then apply_inverse should return original
        let mapped = t.apply(p);
        let recovered = t.apply_inverse(mapped);
        assert!(
            (recovered.x - p.x).abs() < EPSILON,
            "Roundtrip x: expected {}, got {}",
            p.x,
            recovered.x
        );
        assert!(
            (recovered.y - p.y).abs() < EPSILON,
            "Roundtrip y: expected {}, got {}",
            p.y,
            recovered.y
        );
    }

    #[test]
    fn test_inverse_roundtrip_similarity() {
        // Similarity with rotation and scale: roundtrip should recover original
        let t = Transform::similarity(DVec2::new(7.0, -3.0), 0.7, 1.3);
        let p = DVec2::new(100.0, 200.0);

        let mapped = t.apply(p);
        let recovered = t.apply_inverse(mapped);
        assert!(
            (recovered.x - p.x).abs() < 1e-8,
            "Roundtrip x: expected {}, got {}",
            p.x,
            recovered.x
        );
        assert!(
            (recovered.y - p.y).abs() < 1e-8,
            "Roundtrip y: expected {}, got {}",
            p.y,
            recovered.y
        );
    }

    #[test]
    fn test_compose_translation_translation() {
        // Composing T(3,4) * T(5,-2) should give T(8,2)
        // compose(other) = self * other, apply other first then self
        let t1 = Transform::translation(DVec2::new(3.0, 4.0));
        let t2 = Transform::translation(DVec2::new(5.0, -2.0));

        let composed = t1.compose(&t2);
        let p = composed.apply(DVec2::new(0.0, 0.0));
        // (0,0) -> T2 -> (5,-2) -> T1 -> (5+3, -2+4) = (8, 2)
        assert!(approx_eq(p.x, 8.0));
        assert!(approx_eq(p.y, 2.0));
    }

    #[test]
    fn test_compose_takes_more_complex_type() {
        // When composing Translation * Affine, result should be Affine
        let t1 = Transform::translation(DVec2::new(1.0, 2.0));
        let t2 = Transform::affine([1.0, 0.5, 0.0, 0.0, 1.0, 0.0]);

        let composed = t1.compose(&t2);
        assert_eq!(composed.transform_type, TransformType::Affine);

        // Verify the composed transform: apply T2 first (shear), then T1 (translate)
        // T2: (2,2) -> (2 + 0.5*2, 2) = (3, 2)
        // T1: (3,2) -> (3+1, 2+2) = (4, 4)
        let p = composed.apply(DVec2::new(2.0, 2.0));
        assert!(approx_eq(p.x, 4.0));
        assert!(approx_eq(p.y, 4.0));
    }

    #[test]
    fn test_compose_rotation_then_translation() {
        // Rotate 90° then translate by (10, 0)
        let rot = Transform::euclidean(DVec2::ZERO, PI / 2.0);
        let trans = Transform::translation(DVec2::new(10.0, 0.0));

        // trans.compose(rot): apply rot first, then trans
        let composed = trans.compose(&rot);
        // (1, 0) -> rot 90° -> (0, 1) -> translate -> (10, 1)
        let p = composed.apply(DVec2::new(1.0, 0.0));
        assert!((p.x - 10.0).abs() < EPSILON, "Expected x=10, got {}", p.x);
        assert!((p.y - 1.0).abs() < EPSILON, "Expected y=1, got {}", p.y);
    }

    #[test]
    fn test_deviation_from_identity() {
        // Identity has zero deviation
        let id = Transform::identity();
        assert!(approx_eq(id.deviation_from_identity(), 0.0));

        // Translation has non-zero deviation
        let t = Transform::translation(DVec2::new(3.0, 4.0));
        // Deviation is Frobenius norm of (M - I)
        // M - I = [[0,0,3],[0,0,4],[0,0,0]]
        // Frobenius = sqrt(9 + 16) = sqrt(25) = 5.0
        let dev = t.deviation_from_identity();
        assert!(
            (dev - 5.0).abs() < EPSILON,
            "Expected deviation 5.0, got {}",
            dev
        );
    }

    #[test]
    fn test_homography_perspective_hand_computed() {
        // Homography: h = [1, 0, 10, 0, 1, 20, 0.002, 0.001]
        // For point (200, 100):
        // w = 0.002*200 + 0.001*100 + 1 = 0.4 + 0.1 + 1 = 1.5
        // x' = (1*200 + 0*100 + 10) / 1.5 = 210 / 1.5 = 140.0
        // y' = (0*200 + 1*100 + 20) / 1.5 = 120 / 1.5 = 80.0
        let t = Transform::homography([1.0, 0.0, 10.0, 0.0, 1.0, 20.0, 0.002, 0.001]);
        let p = t.apply(DVec2::new(200.0, 100.0));
        assert!((p.x - 140.0).abs() < EPSILON, "Expected x=140, got {}", p.x);
        assert!((p.y - 80.0).abs() < EPSILON, "Expected y=80, got {}", p.y);
    }

    #[test]
    fn test_display_translation() {
        let t = Transform::translation(DVec2::new(10.5, -3.2));
        let s = format!("{}", t);
        assert_eq!(s, "Translation(dx=10.50, dy=-3.20)");
    }

    #[test]
    fn test_display_euclidean() {
        let t = Transform::euclidean(DVec2::new(5.0, -2.0), 0.0);
        let s = format!("{}", t);
        // rotation_angle() = atan2(sin_a, cos_a) = atan2(0, 1) = 0
        assert_eq!(s, "Euclidean(dx=5.00, dy=-2.00, rot=0.000\u{b0})");
    }

    #[test]
    fn test_display_similarity() {
        let t = Transform::similarity(DVec2::new(1.0, 2.0), 0.0, 1.5);
        let s = format!("{}", t);
        assert_eq!(
            s,
            "Similarity(dx=1.00, dy=2.00, rot=0.000\u{b0}, scale=1.5000)"
        );
    }

    #[test]
    #[should_panic(expected = "Cannot invert singular transform matrix")]
    fn test_inverse_singular_panics() {
        let degenerate = Transform::from_matrix(
            DMat3::from_array([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            TransformType::Affine,
        );
        let _ = degenerate.inverse();
    }

    #[test]
    fn test_from_matrix() {
        let m = DMat3::from_array([2.0, 0.0, 5.0, 0.0, 3.0, -1.0, 0.0, 0.0, 1.0]);
        let t = Transform::from_matrix(m, TransformType::Affine);
        // (1, 1) -> (2*1 + 0*1 + 5, 0*1 + 3*1 + (-1)) = (7, 2)
        let p = t.apply(DVec2::new(1.0, 1.0));
        assert!(approx_eq(p.x, 7.0));
        assert!(approx_eq(p.y, 2.0));
        assert_eq!(t.transform_type, TransformType::Affine);
    }
}
