//! Core types for image registration.

#[cfg(test)]
mod tests;

#[cfg(feature = "bench")]
pub mod bench;

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

    /// Create translation transform (alias for `translation`).
    pub fn from_translation(dx: f64, dy: f64) -> Self {
        Self::translation(dx, dy)
    }

    /// Create uniform scale transform.
    pub fn from_scale(sx: f64, sy: f64) -> Self {
        Self {
            data: [sx, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 1.0],
            transform_type: TransformType::Affine,
        }
    }

    /// Create rotation transform around a specified center point.
    pub fn from_rotation_around(angle: f64, cx: f64, cy: f64) -> Self {
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
    pub fn from_matrix(data: [f64; 9], transform_type: TransformType) -> Self {
        Self {
            data,
            transform_type,
        }
    }

    /// Apply transform to a point, returning (x', y').
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let d = &self.data;
        let w = d[6] * x + d[7] * y + d[8];
        let x_prime = (d[0] * x + d[1] * y + d[2]) / w;
        let y_prime = (d[3] * x + d[4] * y + d[5]) / w;
        (x_prime, y_prime)
    }

    /// Alias for `apply` - transform a point.
    #[inline]
    pub fn transform_point(&self, x: f64, y: f64) -> (f64, f64) {
        self.apply(x, y)
    }

    /// Apply inverse transform to a point.
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

/// Registration configuration.
#[derive(Debug, Clone)]
pub struct RegistrationConfig {
    /// Maximum transformation type to consider.
    pub transform_type: TransformType,

    /// Use phase correlation for coarse alignment first.
    pub use_phase_correlation: bool,

    /// RANSAC maximum iterations.
    pub ransac_iterations: usize,
    /// RANSAC inlier threshold in pixels.
    pub ransac_threshold: f64,
    /// RANSAC target confidence (0.0 - 1.0).
    pub ransac_confidence: f64,

    /// Minimum stars required for matching.
    pub min_stars_for_matching: usize,
    /// Maximum stars to use (brightest N).
    pub max_stars_for_matching: usize,
    /// Triangle side ratio tolerance.
    pub triangle_tolerance: f64,

    /// Refine transformation using star centroids.
    pub refine_with_centroids: bool,
    /// Maximum refinement iterations.
    pub max_refinement_iterations: usize,

    /// Minimum matched star pairs required.
    pub min_matched_stars: usize,
    /// Maximum acceptable RMS error in pixels.
    pub max_residual_pixels: f64,
}

impl Default for RegistrationConfig {
    fn default() -> Self {
        Self {
            transform_type: TransformType::Similarity,
            use_phase_correlation: false,
            ransac_iterations: 1000,
            ransac_threshold: 2.0,
            ransac_confidence: 0.999,
            min_stars_for_matching: 10,
            max_stars_for_matching: 200,
            triangle_tolerance: 0.01,
            refine_with_centroids: true,
            max_refinement_iterations: 10,
            min_matched_stars: 6,
            max_residual_pixels: 1.0,
        }
    }
}

impl RegistrationConfig {
    /// Create a new builder for configuration.
    pub fn builder() -> RegistrationConfigBuilder {
        RegistrationConfigBuilder::new()
    }

    /// Validate configuration parameters.
    pub fn validate(&self) {
        assert!(
            self.ransac_iterations > 0,
            "RANSAC iterations must be positive"
        );
        assert!(
            self.ransac_threshold > 0.0,
            "RANSAC threshold must be positive"
        );
        assert!(
            (0.0..=1.0).contains(&self.ransac_confidence),
            "RANSAC confidence must be in [0, 1]"
        );
        assert!(
            self.min_stars_for_matching >= 3,
            "Need at least 3 stars for triangle matching"
        );
        assert!(
            self.max_stars_for_matching >= self.min_stars_for_matching,
            "max_stars must be >= min_stars"
        );
        assert!(
            self.triangle_tolerance > 0.0 && self.triangle_tolerance < 1.0,
            "Triangle tolerance must be in (0, 1)"
        );
        assert!(
            self.min_matched_stars >= self.transform_type.min_points(),
            "min_matched_stars must be >= transform minimum points"
        );
        assert!(
            self.max_residual_pixels > 0.0,
            "max_residual must be positive"
        );
    }
}

/// Builder for RegistrationConfig.
#[derive(Debug, Clone)]
pub struct RegistrationConfigBuilder {
    config: RegistrationConfig,
}

impl RegistrationConfigBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            config: RegistrationConfig::default(),
        }
    }

    /// Set translation-only mode.
    pub fn translation_only(mut self) -> Self {
        self.config.transform_type = TransformType::Translation;
        self
    }

    /// Enable rotation detection.
    pub fn with_rotation(mut self) -> Self {
        self.config.transform_type = TransformType::Euclidean;
        self
    }

    /// Enable rotation and scale detection.
    pub fn with_scale(mut self) -> Self {
        self.config.transform_type = TransformType::Similarity;
        self
    }

    /// Enable full affine transformation.
    pub fn full_affine(mut self) -> Self {
        self.config.transform_type = TransformType::Affine;
        self
    }

    /// Enable full homography transformation.
    pub fn full_homography(mut self) -> Self {
        self.config.transform_type = TransformType::Homography;
        self
    }

    /// Enable/disable phase correlation for coarse alignment.
    pub fn use_phase_correlation(mut self, enable: bool) -> Self {
        self.config.use_phase_correlation = enable;
        self
    }

    /// Set RANSAC iterations.
    pub fn ransac_iterations(mut self, n: usize) -> Self {
        self.config.ransac_iterations = n;
        self
    }

    /// Set RANSAC inlier threshold in pixels.
    pub fn ransac_threshold(mut self, pixels: f64) -> Self {
        self.config.ransac_threshold = pixels;
        self
    }

    /// Set RANSAC confidence level.
    pub fn ransac_confidence(mut self, confidence: f64) -> Self {
        self.config.ransac_confidence = confidence;
        self
    }

    /// Set maximum stars to use for matching.
    pub fn max_stars(mut self, n: usize) -> Self {
        self.config.max_stars_for_matching = n;
        self
    }

    /// Set minimum stars required.
    pub fn min_stars(mut self, n: usize) -> Self {
        self.config.min_stars_for_matching = n;
        self
    }

    /// Set triangle matching tolerance.
    pub fn triangle_tolerance(mut self, tol: f64) -> Self {
        self.config.triangle_tolerance = tol;
        self
    }

    /// Set minimum matched stars required.
    pub fn min_matched_stars(mut self, n: usize) -> Self {
        self.config.min_matched_stars = n;
        self
    }

    /// Set maximum acceptable residual error.
    pub fn max_residual(mut self, pixels: f64) -> Self {
        self.config.max_residual_pixels = pixels;
        self
    }

    /// Enable/disable centroid refinement.
    pub fn refine_with_centroids(mut self, enable: bool) -> Self {
        self.config.refine_with_centroids = enable;
        self
    }

    /// Build the configuration.
    pub fn build(self) -> RegistrationConfig {
        self.config.validate();
        self.config
    }
}

impl Default for RegistrationConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of image registration.
#[derive(Debug, Clone)]
pub struct RegistrationResult {
    /// Computed transformation matrix.
    pub transform: TransformMatrix,

    /// Matched star pairs as (reference_idx, target_idx).
    pub matched_stars: Vec<(usize, usize)>,

    /// Per-match residuals in pixels.
    pub residuals: Vec<f64>,

    /// RMS registration error in pixels.
    pub rms_error: f64,

    /// Maximum residual error in pixels.
    pub max_error: f64,

    /// Number of RANSAC inliers.
    pub num_inliers: usize,

    /// Registration quality score (0.0 - 1.0).
    pub quality_score: f64,

    /// Processing time in milliseconds.
    pub elapsed_ms: f64,
}

impl RegistrationResult {
    /// Create a new registration result.
    pub fn new(
        transform: TransformMatrix,
        matched_stars: Vec<(usize, usize)>,
        residuals: Vec<f64>,
    ) -> Self {
        let rms_error = if residuals.is_empty() {
            0.0
        } else {
            let sum_sq: f64 = residuals.iter().map(|r| r * r).sum();
            (sum_sq / residuals.len() as f64).sqrt()
        };

        let max_error = residuals
            .iter()
            .copied()
            .fold(0.0, |a, b| if a > b { a } else { b });

        let num_inliers = matched_stars.len();

        // Simple quality score based on RMS error and match count
        let quality_score = if num_inliers < 4 {
            0.0
        } else {
            let error_factor = (-rms_error / 2.0).exp();
            let count_factor = (num_inliers as f64 / 20.0).min(1.0);
            error_factor * count_factor
        };

        Self {
            transform,
            matched_stars,
            residuals,
            rms_error,
            max_error,
            num_inliers,
            quality_score,
            elapsed_ms: 0.0,
        }
    }

    /// Set the elapsed time.
    pub fn with_elapsed(mut self, ms: f64) -> Self {
        self.elapsed_ms = ms;
        self
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

/// Registration error types.
#[derive(Debug, Clone)]
pub enum RegistrationError {
    /// Not enough stars detected.
    InsufficientStars { found: usize, required: usize },
    /// No matching star patterns found.
    NoMatchingPatterns,
    /// RANSAC failed to find valid transformation.
    RansacFailed,
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
            RegistrationError::RansacFailed => {
                write!(f, "RANSAC failed to find valid transformation")
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
