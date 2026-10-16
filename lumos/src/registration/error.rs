//! Why a registration found no transform.

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use glam::DVec2;

use crate::registration::transform::TransformType;

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

/// A star measurement registration reads, beside the position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarField {
    /// The FWHM, which sets the matching scale: finite.
    Fwhm,
    /// The position's measurement σ, which the final fit divides each residual by: finite and
    /// positive.
    PositionSigma,
}

impl StarField {
    /// What the field must be.
    pub const fn rule(self) -> &'static str {
        match self {
            Self::Fwhm => "finite",
            Self::PositionSigma => "finite and positive",
        }
    }
}

impl Display for StarField {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Fwhm => "FWHM",
            Self::PositionSigma => "position σ",
        })
    }
}

/// Why RANSAC produced no model: no hypothesis gathered a minimal sample's worth of inliers in
/// the iterations it ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "RANSAC found no inliers (iterations: {iterations}, best inlier count: {best_inlier_count})"
)]
pub struct RansacFailure {
    /// Number of iterations completed.
    pub iterations: usize,
    /// The most inliers any hypothesis gathered, below a minimal sample.
    pub best_inlier_count: usize,
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

/// Why a registration found no transform.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RegistrationError {
    /// Not enough stars detected.
    #[error("too few stars detected: found {found}, need {required}")]
    TooFewStars { found: usize, required: usize },
    /// Too few of a catalog's stars are unsaturated for the final fit, which reads no saturated
    /// star's position.
    #[error("{catalog} catalog has {found} unsaturated stars, and the fit needs {required}")]
    TooFewUnsaturatedStars {
        catalog: RegistrationCatalog,
        found: usize,
        required: usize,
    },
    /// A star has a non-finite position.
    #[error("{catalog} star {index} position must be finite, got ({}, {})", position.x, position.y)]
    InvalidStarPosition {
        catalog: RegistrationCatalog,
        index: usize,
        position: DVec2,
    },
    /// A star has a measurement registration cannot use: see [`StarField::rule`].
    #[error("{catalog} star {index} {field} must be {}, got {value}", field.rule())]
    InvalidStarField {
        catalog: RegistrationCatalog,
        index: usize,
        field: StarField,
        value: f64,
    },
    /// No matching star patterns found.
    #[error("no matching star patterns found between images")]
    NoMatchingPatterns,
    /// The fit, after match recovery, rests on fewer star pairs than
    /// [`min_matches`](crate::RegistrationConfig) asks for.
    #[error("too few inliers: found {found}, need {required}")]
    TooFewInliers { found: usize, required: usize },
    /// RANSAC found no transformation.
    #[error(transparent)]
    RansacFailed(#[from] RansacFailure),
    /// Registration accuracy too low.
    #[error("registration accuracy too low: {rms_error:.3} pixels (max: {max_allowed:.3})")]
    AccuracyTooLow { rms_error: f64, max_allowed: f64 },
    /// Not enough matched points are available for a stable SIP fit.
    #[error("too few points for a SIP fit: found {found}, need {required}")]
    TooFewSipPoints { found: usize, required: usize },
    /// The final fit's matched pairs do not determine the model.
    #[error("the matched pairs do not determine a {model:?} transform")]
    DegenerateFit { model: TransformType },
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
