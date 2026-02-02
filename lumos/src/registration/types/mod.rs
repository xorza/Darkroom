//! Core types for image registration.

#[cfg(test)]
mod tests;

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

/// A matched star pair between reference and target.
#[derive(Debug, Clone, Copy)]
pub struct StarMatch {
    /// Index in reference star list.
    pub ref_idx: usize,
    /// Index in target star list.
    pub target_idx: usize,
    /// Number of votes from triangle matching.
    pub votes: usize,
    /// Match confidence (0.0 - 1.0).
    pub confidence: f64,
}

/// Reason for RANSAC failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RansacFailureReason {
    /// No inliers found after all iterations.
    NoInliersFound,
    /// Point set is degenerate (collinear, coincident, etc.).
    DegeneratePointSet,
    /// Matrix computation failed (singular matrix).
    SingularMatrix,
    /// Found some inliers but not enough to meet threshold.
    InsufficientInliers,
}

impl std::fmt::Display for RansacFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RansacFailureReason::NoInliersFound => write!(f, "no inliers found"),
            RansacFailureReason::DegeneratePointSet => write!(f, "degenerate point set"),
            RansacFailureReason::SingularMatrix => write!(f, "singular matrix"),
            RansacFailureReason::InsufficientInliers => write!(f, "insufficient inliers"),
        }
    }
}

/// Registration error types.
#[derive(Debug, Clone)]
pub enum RegistrationError {
    /// Not enough stars detected.
    InsufficientStars { found: usize, required: usize },
    /// No matching star patterns found.
    NoMatchingPatterns,
    /// RANSAC failed to find valid transformation.
    RansacFailed {
        /// The reason for failure.
        reason: RansacFailureReason,
        /// Number of iterations completed.
        iterations: usize,
        /// Best inlier count achieved (may be 0).
        best_inlier_count: usize,
    },
    /// Registration accuracy too low.
    AccuracyTooLow { rms_error: f64, max_allowed: f64 },
    /// Images have incompatible dimensions.
    DimensionMismatch,
    /// Star detection failed.
    StarDetection(String),
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistrationError::InsufficientStars { found, required } => {
                write!(
                    f,
                    "Insufficient stars detected: found {}, need {}",
                    found, required
                )
            }
            RegistrationError::NoMatchingPatterns => {
                write!(f, "No matching star patterns found between images")
            }
            RegistrationError::RansacFailed {
                reason,
                iterations,
                best_inlier_count,
            } => {
                write!(
                    f,
                    "RANSAC failed: {} (iterations: {}, best inlier count: {})",
                    reason, iterations, best_inlier_count
                )
            }
            RegistrationError::AccuracyTooLow {
                rms_error,
                max_allowed,
            } => {
                write!(
                    f,
                    "Registration accuracy too low: {:.3} pixels (max: {:.3})",
                    rms_error, max_allowed
                )
            }
            RegistrationError::DimensionMismatch => {
                write!(f, "Images have incompatible dimensions")
            }
            RegistrationError::StarDetection(msg) => {
                write!(f, "Star detection failed: {}", msg)
            }
        }
    }
}

impl std::error::Error for RegistrationError {}
