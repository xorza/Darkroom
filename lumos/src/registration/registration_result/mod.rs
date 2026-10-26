//! [`RegistrationResult`]: the transform a registration found, and the matches it rests on.

use crate::registration::distortion::sip::SipPolynomial;
use crate::registration::match_idxs::MatchIdxs;
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;

/// RMS error decay scale: `quality_error = exp(-rms / SCALE)`. At rms=2.0, factor ≈ 0.37.
const QUALITY_ERROR_SCALE: f64 = 2.0;
/// Inlier saturation point: `quality_count = min(inliers / SAT, 1.0)`. Full credit at 20+ inliers.
const QUALITY_INLIER_SATURATION: f64 = 20.0;

/// Corresponding stars in the reference and target inputs with their final fit residual.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StarMatch {
    pub idxs: MatchIdxs,
    /// Distance between the transformed reference star and target star, in pixels.
    pub residual: f64,
}

/// Result of image registration.
#[derive(Debug, Clone)]
pub struct RegistrationResult {
    warp: WarpTransform,
    matched_stars: Vec<StarMatch>,
    rms_error: f64,
}

impl RegistrationResult {
    /// The fit of `warp` to `matched_stars`, its RMS that of their residuals ([`rms_error`]).
    pub(crate) fn new(warp: WarpTransform, matched_stars: Vec<StarMatch>) -> Self {
        debug_assert!(
            matched_stars
                .iter()
                .all(|star_match| star_match.residual.is_finite() && star_match.residual >= 0.0)
        );
        Self {
            warp,
            rms_error: rms_error(&matched_stars),
            matched_stars,
        }
    }

    /// Computed transformation from reference coordinates to target coordinates.
    pub const fn transform(&self) -> Transform {
        self.warp.transform
    }

    /// The SIP distortion correction, when one was requested.
    pub const fn sip(&self) -> Option<&SipPolynomial> {
        self.warp.sip.as_ref()
    }

    /// The transform and its SIP correction, as a warp applies them.
    pub const fn warp_transform(&self) -> &WarpTransform {
        &self.warp
    }

    /// [`Self::warp_transform`], taken whole.
    pub fn into_warp(self) -> WarpTransform {
        self.warp
    }

    /// Corresponding stars and their residuals under the final fitted transform.
    pub fn matched_stars(&self) -> &[StarMatch] {
        &self.matched_stars
    }

    /// Number of matched stars used by the fitted transform.
    pub const fn inlier_count(&self) -> usize {
        self.matched_stars.len()
    }

    /// RMS registration error in pixels.
    pub const fn rms_error(&self) -> f64 {
        self.rms_error
    }

    /// Registration quality score from `0.0` to `1.0`: `exp(−rms / 2)` times the inlier count's
    /// share of 20. A result exists only above `min_matches` inliers, so no floor is needed here.
    pub fn quality_score(&self) -> f64 {
        let error_factor = (-self.rms_error / QUALITY_ERROR_SCALE).exp();
        let count_factor = (self.inlier_count() as f64 / QUALITY_INLIER_SATURATION).min(1.0);
        error_factor * count_factor
    }
}

/// The RMS of the matches' residuals, in pixels: computed once for each fit, which a result then
/// carries. A fit always rests on a match at least.
pub(crate) fn rms_error(matches: &[StarMatch]) -> f64 {
    debug_assert!(!matches.is_empty(), "a fit rests on a match at least");
    let sum_sq: f64 = matches
        .iter()
        .map(|star_match| star_match.residual * star_match.residual)
        .sum();
    (sum_sq / matches.len() as f64).sqrt()
}

#[cfg(test)]
mod tests;
