//! Registration result and error types.

use crate::registration::distortion::SipPolynomial;
use crate::registration::transform::{Transform, WarpTransform};

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
            RegistrationError::StarDetection(msg) => {
                write!(f, "Star detection failed: {}", msg)
            }
        }
    }
}

impl std::error::Error for RegistrationError {}

/// Result of image registration.
#[derive(Debug, Clone)]
pub struct RegistrationResult {
    /// Computed transformation matrix.
    pub transform: Transform,

    /// SIP polynomial distortion correction (if enabled).
    /// When present, apply SIP correction to reference coordinates *before*
    /// the homography: `target = transform(sip.correct(ref))`.
    pub sip_correction: Option<SipPolynomial>,

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
        transform: Transform,
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
            sip_correction: None,
            matched_stars,
            residuals,
            rms_error,
            max_error,
            num_inliers,
            quality_score,
            elapsed_ms: 0.0,
        }
    }

    /// Create a [`WarpTransform`] bundling the transform and SIP correction.
    pub fn warp_transform(&self) -> WarpTransform {
        WarpTransform {
            transform: self.transform,
            sip: self.sip_correction.clone(),
        }
    }

    /// Set the elapsed time.
    pub fn with_elapsed(mut self, ms: f64) -> Self {
        self.elapsed_ms = ms;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec2;

    #[test]
    fn test_registration_result_new() {
        let transform = Transform::translation(DVec2::new(1.0, 2.0));
        let matches = vec![(0, 0), (1, 1), (2, 2)];
        let residuals = vec![0.1, 0.2, 0.15];

        let result = RegistrationResult::new(transform, matches, residuals);

        assert_eq!(result.num_inliers, 3);
        // rms = sqrt((0.01 + 0.04 + 0.0225) / 3) = sqrt(0.0725/3) = sqrt(0.024167)
        let expected_rms = (0.0725_f64 / 3.0).sqrt();
        assert!(
            (result.rms_error - expected_rms).abs() < 1e-10,
            "rms: expected {}, got {}",
            expected_rms,
            result.rms_error
        );
        // max_error = max(0.1, 0.2, 0.15) = 0.2
        assert!((result.max_error - 0.2).abs() < 1e-10);
        // num_inliers = 3 < 4, so quality_score = 0.0
        assert_eq!(result.quality_score, 0.0);
    }

    #[test]
    fn test_registration_result_quality_score_with_4_inliers() {
        let transform = Transform::translation(DVec2::new(1.0, 2.0));
        let matches = vec![(0, 0), (1, 1), (2, 2), (3, 3)];
        let residuals = vec![0.1, 0.2, 0.15, 0.05];

        let result = RegistrationResult::new(transform, matches, residuals);

        assert_eq!(result.num_inliers, 4);
        // rms = sqrt((0.01 + 0.04 + 0.0225 + 0.0025) / 4) = sqrt(0.075/4) = sqrt(0.01875)
        let expected_rms = (0.075_f64 / 4.0).sqrt();
        assert!((result.rms_error - expected_rms).abs() < 1e-10);
        assert!((result.max_error - 0.2).abs() < 1e-10);
        // quality = exp(-rms/2) * min(4/20, 1) = exp(-rms/2) * 0.2
        let expected_quality = (-expected_rms / 2.0).exp() * 0.2;
        assert!(
            (result.quality_score - expected_quality).abs() < 1e-10,
            "quality: expected {}, got {}",
            expected_quality,
            result.quality_score
        );
    }

    #[test]
    fn test_registration_result_empty_residuals() {
        let transform = Transform::identity();
        let result = RegistrationResult::new(transform, vec![], vec![]);
        assert_eq!(result.rms_error, 0.0);
        assert_eq!(result.max_error, 0.0);
        assert_eq!(result.num_inliers, 0);
        assert_eq!(result.quality_score, 0.0); // 0 < 4
    }

    #[test]
    fn test_registration_result_quality_saturates_at_20_inliers() {
        // With >= 20 inliers, count_factor = min(n/20, 1) = 1.0
        let transform = Transform::identity();
        let n = 25;
        let matches: Vec<(usize, usize)> = (0..n).map(|i| (i, i)).collect();
        let residuals = vec![0.1; n];

        let result = RegistrationResult::new(transform, matches, residuals);

        // rms = sqrt(25 * 0.01 / 25) = sqrt(0.01) = 0.1
        assert!((result.rms_error - 0.1).abs() < 1e-10);
        // quality = exp(-0.1/2) * min(25/20, 1) = exp(-0.05) * 1.0
        let expected_quality = (-0.05_f64).exp();
        assert!(
            (result.quality_score - expected_quality).abs() < 1e-10,
            "quality: expected {}, got {}",
            expected_quality,
            result.quality_score
        );
    }

    #[test]
    fn test_registration_result_with_elapsed() {
        let transform = Transform::identity();
        let result = RegistrationResult::new(transform, vec![], vec![]).with_elapsed(42.5);
        assert!((result.elapsed_ms - 42.5).abs() < 1e-10);
    }

    #[test]
    fn test_registration_error_display() {
        let err = RegistrationError::InsufficientStars {
            found: 5,
            required: 10,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("5"));
        assert!(msg.contains("10"));
    }
}
