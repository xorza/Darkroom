//! [`RegistrationResult`]: the transform a registration found, and the matches it rests on.

use crate::registration::match_idxs::MatchIdxs;
use crate::registration::warp_transform::WarpTransform;

/// Corresponding stars in the reference and target inputs with their final fit residual.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StarMatch {
    pub idxs: MatchIdxs,
    /// Distance between the transformed reference star and target star, in pixels.
    pub residual: f64,
}

/// The warp a registration found, the matches it rests on, and their RMS residual.
#[derive(Debug, Clone)]
pub struct RegistrationResult {
    warp: WarpTransform,
    matches: Vec<StarMatch>,
    rms_error_px: f64,
}

impl RegistrationResult {
    /// The fit of `warp` to `matches`, its RMS that of their residuals.
    pub(crate) fn new(warp: WarpTransform, matches: Vec<StarMatch>) -> Self {
        debug_assert!(!matches.is_empty(), "a fit rests on a match at least");
        debug_assert!(
            matches
                .iter()
                .all(|star_match| star_match.residual.is_finite() && star_match.residual >= 0.0)
        );
        let sum_sq: f64 = matches
            .iter()
            .map(|star_match| star_match.residual * star_match.residual)
            .sum();
        Self {
            warp,
            rms_error_px: (sum_sq / matches.len() as f64).sqrt(),
            matches,
        }
    }

    /// The linear transform from reference coordinates to target coordinates and its SIP
    /// correction, as a warp applies them.
    pub const fn warp(&self) -> &WarpTransform {
        &self.warp
    }

    /// [`Self::warp`], taken whole.
    pub fn into_warp(self) -> WarpTransform {
        self.warp
    }

    /// The matched stars the warp was fitted to, with their residuals under it.
    pub fn matches(&self) -> &[StarMatch] {
        &self.matches
    }

    /// The RMS of the matches' residuals, in target pixels.
    pub const fn rms_error_px(&self) -> f64 {
        self.rms_error_px
    }
}

#[cfg(test)]
mod tests;
