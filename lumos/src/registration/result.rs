//! Registration result and error types.

use crate::registration::distortion::SipFitResult;
use crate::registration::transform::{Transform, WarpTransform};

/// Minimum inlier count for a meaningful quality score (below this the fit is unreliable).
const QUALITY_MIN_INLIERS: usize = 4;
/// RMS error decay scale: `quality_error = exp(-rms / SCALE)`. At rms=2.0, factor ≈ 0.37.
const QUALITY_ERROR_SCALE: f64 = 2.0;
/// Inlier saturation point: `quality_count = min(inliers / SAT, 1.0)`. Full credit at 20+ inliers.
const QUALITY_INLIER_SATURATION: f64 = 20.0;

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

    /// SIP fit quality diagnostics (if SIP was fitted).
    /// When present, `sip_fit.polynomial` provides the distortion correction:
    /// `target = transform(sip.correct(ref))`.
    pub sip_fit: Option<SipFitResult>,

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

        // Quality score: product of error and count factors in [0, 1].
        // - error_factor = exp(-rms/SCALE): exponential decay, 1.0 at rms=0, ~0.37 at rms=2px
        // - count_factor = min(inliers/SAT, 1): linear ramp, saturates at 20 matches
        // Below QUALITY_MIN_INLIERS the fit is unreliable so score is zero.
        let quality_score = if num_inliers < QUALITY_MIN_INLIERS {
            0.0
        } else {
            let error_factor = (-rms_error / QUALITY_ERROR_SCALE).exp();
            let count_factor = (num_inliers as f64 / QUALITY_INLIER_SATURATION).min(1.0);
            error_factor * count_factor
        };

        Self {
            transform,
            sip_fit: None,
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
            sip: self.sip_fit.as_ref().map(|r| r.polynomial.clone()),
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
    fn test_registration_error_display_insufficient_stars() {
        let err = RegistrationError::InsufficientStars {
            found: 5,
            required: 10,
        };
        let msg = format!("{}", err);
        assert_eq!(msg, "Insufficient stars detected: found 5, need 10");
    }

    #[test]
    fn test_registration_error_display_no_matching_patterns() {
        let err = RegistrationError::NoMatchingPatterns;
        let msg = format!("{}", err);
        assert_eq!(msg, "No matching star patterns found between images");
    }

    #[test]
    fn test_registration_error_display_ransac_failed() {
        let err = RegistrationError::RansacFailed {
            reason: RansacFailureReason::NoInliersFound,
            iterations: 1000,
            best_inlier_count: 0,
        };
        let msg = format!("{}", err);
        assert_eq!(
            msg,
            "RANSAC failed: no inliers found (iterations: 1000, best inlier count: 0)"
        );
    }

    #[test]
    fn test_registration_error_display_accuracy_too_low() {
        let err = RegistrationError::AccuracyTooLow {
            rms_error: 5.123,
            max_allowed: 2.0,
        };
        let msg = format!("{}", err);
        assert_eq!(
            msg,
            "Registration accuracy too low: 5.123 pixels (max: 2.000)"
        );
    }

    #[test]
    fn test_registration_error_display_star_detection() {
        let err = RegistrationError::StarDetection("threshold too high".to_string());
        let msg = format!("{}", err);
        assert_eq!(msg, "Star detection failed: threshold too high");
    }

    #[test]
    fn test_ransac_failure_reason_display() {
        assert_eq!(
            format!("{}", RansacFailureReason::NoInliersFound),
            "no inliers found"
        );
        assert_eq!(
            format!("{}", RansacFailureReason::DegeneratePointSet),
            "degenerate point set"
        );
        assert_eq!(
            format!("{}", RansacFailureReason::SingularMatrix),
            "singular matrix"
        );
        assert_eq!(
            format!("{}", RansacFailureReason::InsufficientInliers),
            "insufficient inliers"
        );
    }

    #[test]
    fn test_warp_transform_from_result() {
        let transform = Transform::translation(DVec2::new(5.0, -3.0));
        let result = RegistrationResult::new(
            transform,
            vec![(0, 0), (1, 1), (2, 2), (3, 3), (4, 4)],
            vec![0.1, 0.2, 0.15, 0.05, 0.12],
        );

        let wt = result.warp_transform();
        assert!(!wt.has_sip());

        // WarpTransform should apply the same transform
        let p = wt.apply(DVec2::new(10.0, 20.0));
        // Translation (5, -3): (10,20) -> (15, 17)
        assert!((p.x - 15.0).abs() < 1e-10);
        assert!((p.y - 17.0).abs() < 1e-10);
    }

    #[test]
    fn test_quality_score_formula() {
        // Verify the exact quality score formula:
        // quality = exp(-rms/2) * min(n/20, 1)
        //
        // Case 1: 10 inliers, rms = 0.5
        //   count_factor = 10/20 = 0.5
        //   error_factor = exp(-0.5/2) = exp(-0.25)
        //   quality = exp(-0.25) * 0.5
        let transform = Transform::identity();
        let n = 10;
        let matches: Vec<(usize, usize)> = (0..n).map(|i| (i, i)).collect();
        // All residuals = 0.5 => rms = 0.5
        let residuals = vec![0.5; n];
        let result = RegistrationResult::new(transform, matches, residuals);

        let expected_rms = 0.5; // sqrt(n * 0.25 / n) = 0.5
        assert!((result.rms_error - expected_rms).abs() < 1e-10);

        let expected_quality = (-0.25_f64).exp() * 0.5;
        assert!(
            (result.quality_score - expected_quality).abs() < 1e-10,
            "Expected quality {}, got {}",
            expected_quality,
            result.quality_score
        );

        // Case 2: 30 inliers, rms = 1.0
        //   count_factor = min(30/20, 1) = 1.0 (saturated)
        //   error_factor = exp(-1.0/2) = exp(-0.5)
        //   quality = exp(-0.5) * 1.0
        let n2 = 30;
        let matches2: Vec<(usize, usize)> = (0..n2).map(|i| (i, i)).collect();
        let residuals2 = vec![1.0; n2];
        let result2 = RegistrationResult::new(transform, matches2, residuals2);

        let expected_quality2 = (-0.5_f64).exp();
        assert!(
            (result2.quality_score - expected_quality2).abs() < 1e-10,
            "Expected quality {}, got {}",
            expected_quality2,
            result2.quality_score
        );

        // Higher error => lower quality (Case 2 has higher RMS than Case 1)
        // Case 1 quality (count_factor=0.5) vs Case 2 (count_factor=1.0)
        // Case 1: exp(-0.25)*0.5 = 0.3894
        // Case 2: exp(-0.5)*1.0 = 0.6065
        // Case 2 > Case 1 because count_factor dominates at low error
        assert!(result2.quality_score > result.quality_score);
    }
}
