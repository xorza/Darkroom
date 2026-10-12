//! Why a registration found no transform.

use glam::DVec2;

use crate::error::InvalidConfigField;
use crate::registration::transform::TransformType;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

/// Input catalog supplied to image registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationCatalog {
    /// Stars detected in the reference image.
    Reference,
    /// Stars detected in the image being aligned.
    Target,
}

impl Display for RegistrationCatalog {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reference => f.write_str("reference"),
            Self::Target => f.write_str("target"),
        }
    }
}

/// Reason for RANSAC failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RansacFailureReason {
    /// Fewer candidate matches than the model's minimal sample: RANSAC never ran.
    TooFewMatches,
    /// No hypothesis gathered a minimal sample's worth of inliers in the iterations run.
    NoInliersFound,
}

impl Display for RansacFailureReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewMatches => write!(f, "too few matches to sample"),
            Self::NoInliersFound => write!(f, "no inliers found"),
        }
    }
}

/// One model `Auto` tried and the reason it did not produce a fit.
///
/// Boxed because a model's failure is itself a [`RegistrationError`] — without it the enum would be
/// infinitely sized.
#[derive(Debug, Clone)]
pub struct FailedModel {
    /// The model that was attempted.
    pub model: TransformType,
    /// Why it failed.
    pub error: Box<RegistrationError>,
}

impl Display for FailedModel {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.model, self.error)
    }
}

/// The failures on one line: a dropped frame logs its error as a single tracing field, so a message
/// spanning lines would be unreadable where it is actually read.
fn joined(failures: &[FailedModel]) -> String {
    failures
        .iter()
        .map(FailedModel::to_string)
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
    /// A star has a non-finite measurement: its FWHM, flux, SNR, peak, sharpness, eccentricity,
    /// roundness or position σ.
    #[error("{catalog} star {index} {field} must be finite, got {value}")]
    InvalidStarField {
        catalog: RegistrationCatalog,
        index: usize,
        field: &'static str,
        value: f64,
    },
    /// No matching star patterns found.
    #[error("No matching star patterns found between images")]
    NoMatchingPatterns,
    /// The fit, after match recovery, rests on fewer star pairs than
    /// [`min_matches`](crate::RegistrationConfig) asks for.
    #[error("Too few inliers: found {found}, need {required}")]
    TooFewInliers { found: usize, required: usize },
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
    /// A configuration parameter is outside its valid range.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(#[from] InvalidConfigField),
    /// Reference and target point counts differ for SIP fitting.
    #[error("SIP point count mismatch: {reference} reference points, {target} target points")]
    SipPointCountMismatch { reference: usize, target: usize },
    /// Not enough matched points are available for a stable SIP fit.
    #[error("Insufficient points for SIP fit: found {found}, need {required}")]
    InsufficientSipPoints { found: usize, required: usize },
    /// The final fit's matched pairs do not determine the model.
    #[error("the matched pairs do not determine a {model:?} transform")]
    DegenerateFit { model: TransformType },
    /// The SIP polynomial system is singular.
    #[error("SIP fit failed: singular polynomial system")]
    SingularSipSystem,
    /// Every model `Auto` tries failed, each with its own reason.
    ///
    /// Distinct from any single model's error because the models fail independently: RANSAC
    /// estimates the model it was given, so a homography that cannot be estimated says nothing
    /// about whether Euclidean could, and no single reason stands in for the others.
    ///
    /// Only reachable when *no* model produced a fit — any model that fit is a candidate.
    #[error("no transform model fit: {}", joined(failures))]
    EveryModelFailed {
        /// Every model tried, from the fewest degrees of freedom to the most.
        failures: Vec<FailedModel>,
    },
}
