//! The final registration fit: from a hypothesis transform, every usable pair of the full catalogs,
//! weighted by its positional variance and robustly, under a gate that shrinks with the fit.
//!
//! RANSAC's consensus is the brightest stars a sample agreed on, judged at a noise scale of half
//! the FWHM. The fit the frame is warped by deserves every star and every star's precision: each
//! pass matches the full catalogs through the current transform, weights each pair by the inverse
//! of its variance `|det J|·σ_ref² + σ_target²` from the stars' position σ, takes a Cauchy weight
//! on its normalized residual, and refits; the gate starts at the recovery radius and then
//! follows the residuals' own scale. Saturated stars, whose flat tops centre poorly, take no part.
//! A SIP correction is fitted together with the transform in every pass, never after it.

pub(crate) mod fit_catalogs;
mod homography_refinement;
mod weighted_pairs;

use crate::registration::distortion::sip::SipModel;
use crate::registration::registration_result::StarMatch;
use crate::registration::transform::TransformType;
use crate::registration::transform::linear_model::LinearModel;
use crate::registration::warp_transform::WarpTransform;

/// What the final fit fits: a transform alone, or one up to affine under a SIP correction applied
/// before it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FitModel {
    Plain(TransformType),
    WithSip(LinearModel, SipModel),
}

impl FitModel {
    /// The transform's model.
    pub(crate) fn transform_type(self) -> TransformType {
        match self {
            Self::Plain(transform_type) => transform_type,
            Self::WithSip(linear, _) => linear.into(),
        }
    }

    /// The model's free parameters: the transform's, and the SIP correction's two per term.
    pub(crate) fn parameter_count(self) -> usize {
        self.transform_type().parameter_count()
            + match self {
                Self::Plain(_) => 0,
                Self::WithSip(_, sip) => 2 * sip.order.term_count(),
            }
    }

    /// The pairs the model needs: the transform's minimal sample, or the SIP fit's floor.
    fn required_pairs(self) -> usize {
        let minimal = self.transform_type().min_points();
        match self {
            Self::Plain(_) => minimal,
            Self::WithSip(_, sip) => sip.order.required_points().max(minimal),
        }
    }
}

/// The final fit of one model.
#[derive(Debug)]
pub(crate) struct FinalFit {
    pub(crate) warp: WarpTransform,
    /// The pairs the warp was fitted to, with their residuals in target pixels.
    pub(crate) matches: Vec<StarMatch>,
    /// The robust scale of the normalized residuals, at least 1: how far the residuals spread
    /// beyond what the stars' position σ claims.
    pub(crate) scale: f64,
    /// The RMS of the matches' residuals, in target pixels.
    pub(crate) rms_error: f64,
}

/// Why a final fit produced no transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalFitFailure {
    /// Fewer pairs passed the gate than the transform needs.
    TooFewPairs { found: usize, required: usize },
    /// Fewer pairs passed the gate than the SIP correction needs.
    TooFewSipPairs { found: usize, required: usize },
    /// The pairs do not determine the model.
    Degenerate,
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
