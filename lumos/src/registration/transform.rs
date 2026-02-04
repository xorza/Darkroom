//! Transformation matrix for image registration.

use glam::DVec2;

/// Supported transformation models with increasing degrees of freedom.
///
/// Variants are ordered by complexity (used for `compose()` to pick the
/// more complex type). `Auto` is a pipeline-level directive that resolves
/// to a concrete type at runtime — it must not reach RANSAC or transform
/// estimation directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransformType {
    /// Translation only (2 DOF: dx, dy)
    #[default]
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
/// Stored in row-major order:
/// ```text
/// | a  b  tx |   | data[0] data[1] data[2] |
/// | c  d  ty | = | data[3] data[4] data[5] |
/// | g  h  1  |   | data[6] data[7] data[8] |
/// ```
#[derive(Debug, Clone)]
pub struct Transform {
    /// Row-major 3x3 matrix elements.
    pub data: [f64; 9],
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
            data: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            transform_type: TransformType::Translation,
        }
    }

    /// Create translation transform.
    pub fn translation(t: DVec2) -> Self {
        Self {
            data: [1.0, 0.0, t.x, 0.0, 1.0, t.y, 0.0, 0.0, 1.0],
            transform_type: TransformType::Translation,
        }
    }

    /// Create Euclidean transform (translation + rotation).
    pub fn euclidean(t: DVec2, angle: f64) -> Self {
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        Self {
            data: [cos_a, -sin_a, t.x, sin_a, cos_a, t.y, 0.0, 0.0, 1.0],
            transform_type: TransformType::Euclidean,
        }
    }

    /// Create similarity transform (translation + rotation + uniform scale).
    pub fn similarity(t: DVec2, angle: f64, scale: f64) -> Self {
        let cos_a = angle.cos() * scale;
        let sin_a = angle.sin() * scale;
        Self {
            data: [cos_a, -sin_a, t.x, sin_a, cos_a, t.y, 0.0, 0.0, 1.0],
            transform_type: TransformType::Similarity,
        }
    }

    /// Create affine transform from 6 parameters [a, b, tx, c, d, ty].
    pub fn affine(params: [f64; 6]) -> Self {
        Self {
            data: [
                params[0], params[1], params[2], params[3], params[4], params[5], 0.0, 0.0, 1.0,
            ],
            transform_type: TransformType::Affine,
        }
    }

    /// Create homography from 8 parameters (9th element is 1.0).
    pub fn homography(params: [f64; 8]) -> Self {
        Self {
            data: [
                params[0], params[1], params[2], params[3], params[4], params[5], params[6],
                params[7], 1.0,
            ],
            transform_type: TransformType::Homography,
        }
    }

    /// Create scale transform.
    pub fn scale(s: DVec2) -> Self {
        Self {
            data: [s.x, 0.0, 0.0, 0.0, s.y, 0.0, 0.0, 0.0, 1.0],
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
            data: [cos_a, -sin_a, tx, sin_a, cos_a, ty, 0.0, 0.0, 1.0],
            transform_type: TransformType::Euclidean,
        }
    }

    /// Create transform from raw 3x3 matrix data.
    pub fn matrix(data: [f64; 9], transform_type: TransformType) -> Self {
        Self {
            data,
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
        let d = &self.data;
        let w = d[6] * p.x + d[7] * p.y + d[8];
        let x_prime = (d[0] * p.x + d[1] * p.y + d[2]) / w;
        let y_prime = (d[3] * p.x + d[4] * p.y + d[5]) / w;
        DVec2::new(x_prime, y_prime)
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
        let d = &self.data;

        // Compute determinant
        let det = d[0] * (d[4] * d[8] - d[5] * d[7]) - d[1] * (d[3] * d[8] - d[5] * d[6])
            + d[2] * (d[3] * d[7] - d[4] * d[6]);

        assert!(
            det.abs() >= 1e-12,
            "Cannot invert singular matrix (determinant = {})",
            det
        );

        let inv_det = 1.0 / det;

        // Compute adjugate matrix and divide by determinant
        let inv_data = [
            (d[4] * d[8] - d[5] * d[7]) * inv_det,
            (d[2] * d[7] - d[1] * d[8]) * inv_det,
            (d[1] * d[5] - d[2] * d[4]) * inv_det,
            (d[5] * d[6] - d[3] * d[8]) * inv_det,
            (d[0] * d[8] - d[2] * d[6]) * inv_det,
            (d[2] * d[3] - d[0] * d[5]) * inv_det,
            (d[3] * d[7] - d[4] * d[6]) * inv_det,
            (d[1] * d[6] - d[0] * d[7]) * inv_det,
            (d[0] * d[4] - d[1] * d[3]) * inv_det,
        ];

        Self {
            data: inv_data,
            transform_type: self.transform_type,
        }
    }

    /// Compose two transforms: self * other (apply other first, then self).
    pub fn compose(&self, other: &Self) -> Self {
        let a = &self.data;
        let b = &other.data;

        let data = [
            a[0] * b[0] + a[1] * b[3] + a[2] * b[6],
            a[0] * b[1] + a[1] * b[4] + a[2] * b[7],
            a[0] * b[2] + a[1] * b[5] + a[2] * b[8],
            a[3] * b[0] + a[4] * b[3] + a[5] * b[6],
            a[3] * b[1] + a[4] * b[4] + a[5] * b[7],
            a[3] * b[2] + a[4] * b[5] + a[5] * b[8],
            a[6] * b[0] + a[7] * b[3] + a[8] * b[6],
            a[6] * b[1] + a[7] * b[4] + a[8] * b[7],
            a[6] * b[2] + a[7] * b[5] + a[8] * b[8],
        ];

        // Result type is the more complex of the two
        let transform_type = if self.transform_type as u8 > other.transform_type as u8 {
            self.transform_type
        } else {
            other.transform_type
        };

        Self {
            data,
            transform_type,
        }
    }

    /// Extract translation components as DVec2.
    pub fn translation_components(&self) -> DVec2 {
        DVec2::new(self.data[2], self.data[5])
    }

    /// Extract rotation angle in radians (valid for Euclidean/Similarity transforms).
    pub fn rotation_angle(&self) -> f64 {
        self.data[3].atan2(self.data[0])
    }

    /// Extract scale factor (valid for Similarity transforms).
    pub fn scale_factor(&self) -> f64 {
        let a = self.data[0];
        let c = self.data[3];
        (a * a + c * c).sqrt()
    }

    /// Compute the determinant of the 2x2 linear part.
    pub fn linear_determinant(&self) -> f64 {
        self.data[0] * self.data[4] - self.data[1] * self.data[3]
    }

    /// Check if this is a valid (non-degenerate) transformation.
    pub fn is_valid(&self) -> bool {
        let det = self.linear_determinant();
        det.abs() > 1e-10 && det.is_finite()
    }

    /// Compute Frobenius norm of difference from identity.
    pub fn deviation_from_identity(&self) -> f64 {
        let id = Self::identity();
        let mut sum = 0.0;
        for i in 0..9 {
            let diff = self.data[i] - id.data[i];
            sum += diff * diff;
        }
        sum.sqrt()
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
    fn test_transform_inverse() {
        let t = Transform::similarity(DVec2::new(10.0, -5.0), PI / 4.0, 1.5);
        let inv = t.inverse();

        let p1 = t.apply(DVec2::new(3.0, 7.0));
        let p2 = inv.apply(p1);

        assert!(approx_eq(p2.x, 3.0));
        assert!(approx_eq(p2.y, 7.0));
    }

    #[test]
    fn test_apply_roundtrip() {
        let transforms = vec![
            Transform::translation(DVec2::new(5.0, -3.0)),
            Transform::euclidean(DVec2::new(2.0, 3.0), 0.7),
            Transform::similarity(DVec2::new(1.0, 2.0), -0.5, 1.3),
            Transform::affine([1.1, 0.2, 5.0, -0.1, 0.9, -3.0]),
        ];

        for t in transforms {
            let inv = t.inverse();
            for p in &[
                DVec2::new(0.0, 0.0),
                DVec2::new(10.0, 10.0),
                DVec2::new(-5.0, 7.0),
                DVec2::new(100.0, -50.0),
            ] {
                let p1 = t.apply(*p);
                let p2 = inv.apply(p1);
                assert!(
                    approx_eq(p2.x, p.x) && approx_eq(p2.y, p.y),
                    "Roundtrip failed for {:?}: got {:?}",
                    p,
                    p2
                );
            }
        }
    }

    #[test]
    fn test_compose_translations() {
        let t1 = Transform::translation(DVec2::new(5.0, 3.0));
        let t2 = Transform::translation(DVec2::new(2.0, -1.0));
        let composed = t1.compose(&t2);

        let p = composed.apply(DVec2::ZERO);
        // t2 first: (0,0) -> (2,-1), then t1: (2,-1) -> (7,2)
        assert!(approx_eq(p.x, 7.0));
        assert!(approx_eq(p.y, 2.0));
    }

    #[test]
    fn test_compose_rotations() {
        let t1 = Transform::euclidean(DVec2::ZERO, PI / 4.0);
        let t2 = Transform::euclidean(DVec2::ZERO, PI / 4.0);
        let composed = t1.compose(&t2);

        let p = composed.apply(DVec2::new(1.0, 0.0));
        // Two 45° rotations = 90° rotation
        assert!(approx_eq(p.x, 0.0));
        assert!(approx_eq(p.y, 1.0));
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
        let degenerate = Transform::matrix(
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
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
}
