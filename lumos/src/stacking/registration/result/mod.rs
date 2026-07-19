//! Registration result and error types.

use glam::DVec2;

use crate::stacking::registration::distortion::sip::SipFitResult;
use crate::stacking::registration::transform::{Transform, WarpTransform};

/// Minimum inlier count for a meaningful quality score (below this the fit is unreliable).
const QUALITY_MIN_INLIERS: usize = 4;
/// RMS error decay scale: `quality_error = exp(-rms / SCALE)`. At rms=2.0, factor ≈ 0.37.
const QUALITY_ERROR_SCALE: f64 = 2.0;
/// Inlier saturation point: `quality_count = min(inliers / SAT, 1.0)`. Full credit at 20+ inliers.
const QUALITY_INLIER_SATURATION: f64 = 20.0;

/// Input catalog supplied to image registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationCatalog {
    /// Stars detected in the reference image.
    Reference,
    /// Stars detected in the image being aligned.
    Target,
}

impl std::fmt::Display for RegistrationCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistrationCatalog::Reference => f.write_str("reference"),
            RegistrationCatalog::Target => f.write_str("target"),
        }
    }
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
    /// A star has a non-finite position.
    InvalidStarPosition {
        catalog: RegistrationCatalog,
        index: usize,
        position: DVec2,
    },
    /// A star has a non-finite FWHM.
    InvalidStarFwhm {
        catalog: RegistrationCatalog,
        index: usize,
        value: f32,
    },
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
    /// A configuration parameter is outside its valid range.
    InvalidConfig(String),
    /// Reference and target point counts differ for SIP fitting.
    SipPointCountMismatch { reference: usize, target: usize },
    /// Not enough matched points are available for a stable SIP fit.
    InsufficientSipPoints { found: usize, required: usize },
    /// The SIP polynomial system is singular.
    SingularSipSystem,
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
            RegistrationError::InvalidStarPosition {
                catalog,
                index,
                position,
            } => {
                write!(
                    f,
                    "{catalog} star {index} position must be finite, got ({}, {})",
                    position.x, position.y
                )
            }
            RegistrationError::InvalidStarFwhm {
                catalog,
                index,
                value,
            } => {
                write!(f, "{catalog} star {index} FWHM must be finite, got {value}")
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
            RegistrationError::InvalidConfig(msg) => {
                write!(f, "Invalid configuration: {}", msg)
            }
            RegistrationError::SipPointCountMismatch { reference, target } => {
                write!(
                    f,
                    "SIP point count mismatch: {} reference points, {} target points",
                    reference, target
                )
            }
            RegistrationError::InsufficientSipPoints { found, required } => {
                write!(
                    f,
                    "Insufficient points for SIP fit: found {}, need {}",
                    found, required
                )
            }
            RegistrationError::SingularSipSystem => {
                write!(f, "SIP fit failed: singular polynomial system")
            }
        }
    }
}

impl std::error::Error for RegistrationError {}

/// Corresponding stars in the reference and target inputs with their final fit residual.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StarMatch {
    /// Index into the reference star slice.
    pub reference: usize,
    /// Index into the target star slice.
    pub target: usize,
    /// Distance between the transformed reference star and target star, in pixels.
    pub residual: f64,
}

/// Result of image registration.
#[derive(Debug, Clone)]
pub struct RegistrationResult {
    transform: Transform,
    sip_fit: Option<SipFitResult>,
    matched_stars: Vec<StarMatch>,
    elapsed_ms: f64,
}

impl RegistrationResult {
    pub(crate) fn new(
        transform: Transform,
        sip_fit: Option<SipFitResult>,
        matched_stars: Vec<StarMatch>,
    ) -> Self {
        debug_assert!(
            matched_stars
                .iter()
                .all(|star_match| star_match.residual.is_finite() && star_match.residual >= 0.0)
        );
        Self {
            transform,
            sip_fit,
            matched_stars,
            elapsed_ms: 0.0,
        }
    }

    /// Computed transformation from reference coordinates to target coordinates.
    pub fn transform(&self) -> Transform {
        self.transform
    }

    /// SIP fit and its diagnostics, when nonlinear distortion correction was requested.
    pub fn sip_fit(&self) -> Option<&SipFitResult> {
        self.sip_fit.as_ref()
    }

    /// Corresponding stars and their residuals under the final fitted transform.
    pub fn matched_stars(&self) -> &[StarMatch] {
        &self.matched_stars
    }

    /// Number of matched stars used by the fitted transform.
    pub fn num_inliers(&self) -> usize {
        self.matched_stars.len()
    }

    /// RMS registration error in pixels.
    pub fn rms_error(&self) -> f64 {
        if self.matched_stars.is_empty() {
            0.0
        } else {
            let sum_sq: f64 = self
                .matched_stars
                .iter()
                .map(|star_match| star_match.residual * star_match.residual)
                .sum();
            (sum_sq / self.matched_stars.len() as f64).sqrt()
        }
    }

    /// Maximum residual error in pixels.
    pub fn max_error(&self) -> f64 {
        self.matched_stars
            .iter()
            .map(|star_match| star_match.residual)
            .fold(0.0, f64::max)
    }

    /// Registration quality score from `0.0` to `1.0`.
    pub fn quality_score(&self) -> f64 {
        let num_inliers = self.num_inliers();
        if num_inliers < QUALITY_MIN_INLIERS {
            0.0
        } else {
            let error_factor = (-self.rms_error() / QUALITY_ERROR_SCALE).exp();
            let count_factor = (num_inliers as f64 / QUALITY_INLIER_SATURATION).min(1.0);
            error_factor * count_factor
        }
    }

    /// Registration processing time in milliseconds.
    pub fn elapsed_ms(&self) -> f64 {
        self.elapsed_ms
    }

    /// Create a [`WarpTransform`] bundling the transform and SIP correction.
    pub fn warp_transform(&self) -> WarpTransform {
        WarpTransform {
            transform: self.transform,
            sip: self.sip_fit.as_ref().map(|r| r.polynomial.clone()),
        }
    }

    /// Set the elapsed time.
    pub(crate) fn with_elapsed(mut self, ms: f64) -> Self {
        self.elapsed_ms = ms;
        self
    }
}

#[cfg(test)]
mod tests;
