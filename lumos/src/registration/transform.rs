//! Transformation matrix for image registration.

/// Supported transformation models with increasing degrees of freedom.
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
pub struct TransformMatrix {
    /// Row-major 3x3 matrix elements.
    pub data: [f64; 9],
    /// The type of transformation this matrix represents.
    pub transform_type: TransformType,
}

impl Default for TransformMatrix {
    fn default() -> Self {
        Self::identity()
    }
}

impl std::fmt::Display for TransformMatrix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (tx, ty) = self.translation_components();
        let rotation_deg = self.rotation_angle().to_degrees();
        let scale = self.scale_factor();

        match self.transform_type {
            TransformType::Translation => {
                write!(f, "Translation(dx={:.2}, dy={:.2})", tx, ty)
            }
            TransformType::Euclidean => {
                write!(
                    f,
                    "Euclidean(dx={:.2}, dy={:.2}, rot={:.3}°)",
                    tx, ty, rotation_deg
                )
            }
            TransformType::Similarity => {
                write!(
                    f,
                    "Similarity(dx={:.2}, dy={:.2}, rot={:.3}°, scale={:.4})",
                    tx, ty, rotation_deg, scale
                )
            }
            TransformType::Affine => {
                write!(
                    f,
                    "Affine(dx={:.2}, dy={:.2}, rot={:.3}°, scale={:.4})",
                    tx, ty, rotation_deg, scale
                )
            }
            TransformType::Homography => {
                write!(
                    f,
                    "Homography(dx={:.2}, dy={:.2}, rot={:.3}°, scale={:.4})",
                    tx, ty, rotation_deg, scale
                )
            }
        }
    }
}

impl TransformMatrix {
    /// Create identity transform.
    pub fn identity() -> Self {
        Self {
            data: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            transform_type: TransformType::Translation,
        }
    }

    /// Create translation transform.
    pub fn translation(dx: f64, dy: f64) -> Self {
        Self {
            data: [1.0, 0.0, dx, 0.0, 1.0, dy, 0.0, 0.0, 1.0],
            transform_type: TransformType::Translation,
        }
    }

    /// Create Euclidean transform (translation + rotation).
    pub fn euclidean(dx: f64, dy: f64, angle: f64) -> Self {
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        Self {
            data: [cos_a, -sin_a, dx, sin_a, cos_a, dy, 0.0, 0.0, 1.0],
            transform_type: TransformType::Euclidean,
        }
    }

    /// Create similarity transform (translation + rotation + uniform scale).
    pub fn similarity(dx: f64, dy: f64, angle: f64, scale: f64) -> Self {
        let cos_a = angle.cos() * scale;
        let sin_a = angle.sin() * scale;
        Self {
            data: [cos_a, -sin_a, dx, sin_a, cos_a, dy, 0.0, 0.0, 1.0],
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

    /// Create uniform scale transform.
    pub fn scale(sx: f64, sy: f64) -> Self {
        Self {
            data: [sx, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 1.0],
            transform_type: TransformType::Affine,
        }
    }

    /// Create rotation transform around a specified center point.
    ///
    /// Parameters follow the pattern (center_x, center_y, angle) for consistency
    /// with other constructors that use (position, rotation) ordering.
    pub fn rotation_around(cx: f64, cy: f64, angle: f64) -> Self {
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        // T(-cx,-cy) * R(angle) * T(cx,cy)
        let tx = cx - cos_a * cx + sin_a * cy;
        let ty = cy - sin_a * cx - cos_a * cy;
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
    /// let (target_x, target_y) = transform.apply(ref_x, ref_y);
    ///
    /// // Map a target point back to reference coordinates
    /// let (ref_x, ref_y) = transform.apply_inverse(target_x, target_y);
    /// ```
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let d = &self.data;
        let w = d[6] * x + d[7] * y + d[8];
        let x_prime = (d[0] * x + d[1] * y + d[2]) / w;
        let y_prime = (d[3] * x + d[4] * y + d[5]) / w;
        (x_prime, y_prime)
    }

    /// Apply inverse transform to map a point from TARGET coordinates to REFERENCE coordinates.
    ///
    /// This is the inverse of `apply()`. Given a point in the target image,
    /// it returns the corresponding point in the reference image.
    ///
    /// See [`apply`](Self::apply) for more details on transform direction.
    pub fn apply_inverse(&self, x: f64, y: f64) -> (f64, f64) {
        self.inverse().apply(x, y)
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

    /// Extract translation components (tx, ty).
    pub fn translation_components(&self) -> (f64, f64) {
        (self.data[2], self.data[5])
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
    fn test_identity_transform() {
        let t = TransformMatrix::identity();
        let (x, y) = t.apply(5.0, 7.0);
        assert!(approx_eq(x, 5.0));
        assert!(approx_eq(y, 7.0));
    }

    #[test]
    fn test_translation_transform() {
        let t = TransformMatrix::translation(10.0, -5.0);
        let (x, y) = t.apply(3.0, 4.0);
        assert!(approx_eq(x, 13.0));
        assert!(approx_eq(y, -1.0));
    }

    #[test]
    fn test_rotation_90_degrees() {
        let t = TransformMatrix::euclidean(0.0, 0.0, PI / 2.0);
        let (x, y) = t.apply(1.0, 0.0);
        assert!(approx_eq(x, 0.0));
        assert!(approx_eq(y, 1.0));
    }

    #[test]
    fn test_rotation_180_degrees() {
        let t = TransformMatrix::euclidean(0.0, 0.0, PI);
        let (x, y) = t.apply(1.0, 0.0);
        assert!(approx_eq(x, -1.0));
        assert!(approx_eq(y, 0.0));
    }

    #[test]
    fn test_scale_transform() {
        let t = TransformMatrix::similarity(0.0, 0.0, 0.0, 2.0);
        let (x, y) = t.apply(3.0, 4.0);
        assert!(approx_eq(x, 6.0));
        assert!(approx_eq(y, 8.0));
    }

    #[test]
    fn test_similarity_with_rotation_and_scale() {
        let t = TransformMatrix::similarity(5.0, 10.0, PI / 2.0, 2.0);
        let (x, y) = t.apply(1.0, 0.0);
        // Rotate 90° then scale 2x: (1,0) -> (0,1) -> (0,2), then translate
        assert!(approx_eq(x, 5.0));
        assert!(approx_eq(y, 12.0));
    }

    #[test]
    fn test_affine_transform() {
        // Shear transform
        let t = TransformMatrix::affine([1.0, 0.5, 0.0, 0.0, 1.0, 0.0]);
        let (x, y) = t.apply(2.0, 2.0);
        assert!(approx_eq(x, 3.0)); // 2 + 0.5*2
        assert!(approx_eq(y, 2.0));
    }

    #[test]
    fn test_transform_inverse() {
        let t = TransformMatrix::similarity(10.0, -5.0, PI / 4.0, 1.5);
        let inv = t.inverse();

        let (x1, y1) = t.apply(3.0, 7.0);
        let (x2, y2) = inv.apply(x1, y1);

        assert!(approx_eq(x2, 3.0));
        assert!(approx_eq(y2, 7.0));
    }

    #[test]
    fn test_apply_roundtrip() {
        let transforms = vec![
            TransformMatrix::translation(5.0, -3.0),
            TransformMatrix::euclidean(2.0, 3.0, 0.7),
            TransformMatrix::similarity(1.0, 2.0, -0.5, 1.3),
            TransformMatrix::affine([1.1, 0.2, 5.0, -0.1, 0.9, -3.0]),
        ];

        for t in transforms {
            let inv = t.inverse();
            for &(x, y) in &[(0.0, 0.0), (10.0, 10.0), (-5.0, 7.0), (100.0, -50.0)] {
                let (x1, y1) = t.apply(x, y);
                let (x2, y2) = inv.apply(x1, y1);
                assert!(
                    approx_eq(x2, x) && approx_eq(y2, y),
                    "Roundtrip failed for ({}, {}): got ({}, {})",
                    x,
                    y,
                    x2,
                    y2
                );
            }
        }
    }

    #[test]
    fn test_compose_translations() {
        let t1 = TransformMatrix::translation(5.0, 3.0);
        let t2 = TransformMatrix::translation(2.0, -1.0);
        let composed = t1.compose(&t2);

        let (x, y) = composed.apply(0.0, 0.0);
        // t2 first: (0,0) -> (2,-1), then t1: (2,-1) -> (7,2)
        assert!(approx_eq(x, 7.0));
        assert!(approx_eq(y, 2.0));
    }

    #[test]
    fn test_compose_rotations() {
        let t1 = TransformMatrix::euclidean(0.0, 0.0, PI / 4.0);
        let t2 = TransformMatrix::euclidean(0.0, 0.0, PI / 4.0);
        let composed = t1.compose(&t2);

        let (x, y) = composed.apply(1.0, 0.0);
        // Two 45° rotations = 90° rotation
        assert!(approx_eq(x, 0.0));
        assert!(approx_eq(y, 1.0));
    }

    #[test]
    fn test_translation_components() {
        let t = TransformMatrix::translation(7.0, -3.0);
        let (tx, ty) = t.translation_components();
        assert!(approx_eq(tx, 7.0));
        assert!(approx_eq(ty, -3.0));
    }

    #[test]
    fn test_rotation_angle() {
        let angle = 0.5;
        let t = TransformMatrix::euclidean(0.0, 0.0, angle);
        assert!(approx_eq(t.rotation_angle(), angle));
    }

    #[test]
    fn test_scale_factor() {
        let scale = 2.5;
        let t = TransformMatrix::similarity(0.0, 0.0, 0.0, scale);
        assert!(approx_eq(t.scale_factor(), scale));
    }

    #[test]
    fn test_is_valid() {
        let valid = TransformMatrix::similarity(1.0, 2.0, 0.5, 1.5);
        assert!(valid.is_valid());

        // Degenerate matrix (zero scale)
        let degenerate = TransformMatrix::matrix(
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            TransformType::Affine,
        );
        assert!(!degenerate.is_valid());
    }

    #[test]
    fn test_homography_transform() {
        // Simple homography that acts like translation
        let t = TransformMatrix::homography([1.0, 0.0, 5.0, 0.0, 1.0, 3.0, 0.0, 0.0]);
        let (x, y) = t.apply(2.0, 4.0);
        assert!(approx_eq(x, 7.0));
        assert!(approx_eq(y, 7.0));
    }

    #[test]
    fn test_homography_perspective() {
        // Homography with perspective component
        let t = TransformMatrix::homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.001, 0.0]);
        let (x, y) = t.apply(100.0, 0.0);
        // w = 0.001 * 100 + 1 = 1.1
        // x' = 100 / 1.1 ≈ 90.9
        assert!((x - 90.909).abs() < 0.01);
        assert!(approx_eq(y, 0.0));
    }
}
