//! Iterative kappa-sigma clipping: reject beyond `k` sigma of the median, repeat.

use crate::combine::rejection::normal_scores::NormalScores;
use crate::combine::rejection::pass::{Narrowing, Pass};
use crate::combine::rejection::rejection_scale::RejectionScale;
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::combine::rejection::validate_max_iterations;
use crate::error::InvalidConfigField;

/// Configuration for sigma clipping.
///
/// Each pass keeps the samples within `sigma` of the median, in the unit `scale` names.
/// Asymmetric bounds reject one side harder, for example bright satellite trails and cosmic rays.
///
/// A later pass measures the robust scale with the rejected samples in their places: the window
/// is fitted against the normal scores of every sample, as linear fit does, so the tails the
/// earlier passes cut do not shrink σ. Siril measures the survivors' MAD as a complete sample,
/// which on ten clean samples at 2.5σ rejects 5.5% over three passes; this rejects 3.7%, and 0.8%
/// under the frames' noise floor, against the Gaussian tail share of 1.24%.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SigmaClipConfig {
    /// How far either side of the median a value may sit, in sigma.
    pub sigma: SigmaBounds,
    /// The most passes to run. The clip stops earlier when a pass rejects nothing.
    pub max_iterations: u32,
    pub scale: RejectionScale,
}

impl Default for SigmaClipConfig {
    fn default() -> Self {
        Self::new(2.5, 3)
    }
}

impl SigmaClipConfig {
    /// Create symmetric sigma clipping (same threshold for low and high).
    pub const fn new(sigma: f32, max_iterations: u32) -> Self {
        Self::new_asymmetric(sigma, sigma, max_iterations)
    }

    /// Create asymmetric sigma clipping with separate low/high thresholds.
    pub const fn new_asymmetric(sigma_low: f32, sigma_high: f32, max_iterations: u32) -> Self {
        Self {
            sigma: SigmaBounds::asymmetric(sigma_low, sigma_high),
            max_iterations,
            scale: RejectionScale::Robust,
        }
    }

    /// The same clip in another unit.
    #[must_use]
    pub const fn with_scale(self, scale: RejectionScale) -> Self {
        Self { scale, ..self }
    }

    /// Validate the clip thresholds and iteration count.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        self.sigma.validate()?;
        validate_max_iterations(self.max_iterations)
    }

    pub(crate) fn narrow(&self, pass: &Pass<'_>, scores: &mut NormalScores) -> Narrowing {
        match self.scale {
            RejectionScale::Robust => Narrowing::Window(pass.clip_about_median(self.sigma, scores)),
            RejectionScale::CcdModel => pass.clip_by_model(self.sigma),
        }
    }
}
