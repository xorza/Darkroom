//! Registration result and error types.

use crate::stacking::registration::triangle::voting::MatchIndices;

use glam::DVec2;

use crate::error::InvalidConfigField;
use crate::stacking::registration::distortion::sip::SipFitResult;
use crate::stacking::registration::transform::{Transform, TransformType, WarpTransform};

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

/// One model the `Auto` ladder tried and the reason it did not produce a fit.
///
/// Boxed because a rung's failure is itself a [`RegistrationError`] — without it the enum would be
/// infinitely sized.
#[derive(Debug, Clone)]
pub struct FailedRung {
    /// The model that was attempted.
    pub model: TransformType,
    /// Why it failed.
    pub error: Box<RegistrationError>,
}

impl std::fmt::Display for FailedRung {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.model, self.error)
    }
}

/// The rungs on one line: a dropped frame logs its error as a single tracing field, so a message
/// spanning lines would be unreadable where it is actually read.
fn joined(failures: &[FailedRung]) -> String {
    failures
        .iter()
        .map(FailedRung::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Registration error types.
///
/// `position` and `catalog` reach the messages below through [`RegistrationCatalog`]'s and
/// [`DVec2`]'s own formatting, which is why those two keep hand-written `Display` impls rather
/// than deriving one they would only use here.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RegistrationError {
    /// Not enough stars detected.
    #[error("Insufficient stars detected: found {found}, need {required}")]
    InsufficientStars { found: usize, required: usize },
    /// A star has a non-finite position.
    #[error("{catalog} star {index} position must be finite, got ({}, {})", position.x, position.y)]
    InvalidStarPosition {
        catalog: RegistrationCatalog,
        index: usize,
        position: DVec2,
    },
    /// A star has a non-finite FWHM.
    #[error("{catalog} star {index} FWHM must be finite, got {value}")]
    InvalidStarFwhm {
        catalog: RegistrationCatalog,
        index: usize,
        value: f32,
    },
    /// No matching star patterns found.
    #[error("No matching star patterns found between images")]
    NoMatchingPatterns,
    /// RANSAC failed to find valid transformation.
    #[error(
        "RANSAC failed: {reason} (iterations: {iterations}, best inlier count: {best_inlier_count})"
    )]
    RansacFailed {
        /// The reason for failure.
        reason: RansacFailureReason,
        /// Number of iterations completed.
        iterations: usize,
        /// Best inlier count achieved (may be 0).
        best_inlier_count: usize,
    },
    /// Registration accuracy too low.
    #[error("Registration accuracy too low: {rms_error:.3} pixels (max: {max_allowed:.3})")]
    AccuracyTooLow { rms_error: f64, max_allowed: f64 },
    /// Star detection failed.
    #[error("Star detection failed: {0}")]
    StarDetection(String),
    /// A configuration parameter is outside its valid range.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(#[from] InvalidConfigField),
    /// Reference and target point counts differ for SIP fitting.
    #[error("SIP point count mismatch: {reference} reference points, {target} target points")]
    SipPointCountMismatch { reference: usize, target: usize },
    /// Not enough matched points are available for a stable SIP fit.
    #[error("Insufficient points for SIP fit: found {found}, need {required}")]
    InsufficientSipPoints { found: usize, required: usize },
    /// The SIP polynomial system is singular.
    #[error("SIP fit failed: singular polynomial system")]
    SingularSipSystem,
    /// Every model on the `Auto` ladder failed, each with its own reason.
    ///
    /// Distinct from any single rung's error because the rungs fail independently: RANSAC estimates
    /// the model it was given, and SIP is fit on that model's inlier set, so a homography that
    /// cannot be estimated says nothing about whether Euclidean could. Reporting only the last
    /// rung's failure made "the ladder had nothing to offer" indistinguishable from "homography
    /// specifically failed", and hid the other three reasons.
    ///
    /// Only reachable when *no* rung produced a fit — a rung that fit is returned rather than
    /// discarded, even when a later one fails.
    #[error("no transform model fit: {}", joined(failures))]
    AutoLadderExhausted {
        /// Every rung tried, in ladder order.
        failures: Vec<FailedRung>,
    },
}

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

impl StarMatch {
    /// A pair with the residual measured against a fitted transform.
    pub(crate) fn measured(indices: MatchIndices, residual: f64) -> Self {
        Self {
            reference: indices.reference,
            target: indices.target,
            residual,
        }
    }
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
