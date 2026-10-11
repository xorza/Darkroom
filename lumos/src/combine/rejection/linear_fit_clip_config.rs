//! Linear-fit clipping: regress the sorted samples on their expected Gaussian positions, and clip
//! about the fitted centre in units of the fitted σ.

use crate::combine::rejection::normal_scores::NormalScores;
use crate::combine::rejection::pass::{Pass, Proposal};
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::combine::rejection::validate_max_passes;
use crate::error::InvalidConfigField;

/// Configuration for linear fit clipping.
///
/// The first pass is a sigma clip about the median. Each later pass fits the line `μ + σ·z` through
/// the samples still kept, sorted, against `z`, their normal scores among all of the pixel's
/// samples: a normal Q-Q plot. The rejected samples keep their places in the ranking, so the fit is
/// a censored regression, and the slope stays an estimate of σ however much the ends lost. The
/// pass then keeps the samples within `sigma` of `μ`, in units of that σ.
///
/// Siril's linear fit regresses the sorted samples on their rank instead, with the mean absolute
/// residual as the unit. Sorted Gaussian samples follow the normal quantile curve, not a line in
/// rank, so its rejection rate on clean data grows with the frame count. On the normal scores the
/// line is exact, and `sigma` means the same thing in every pass and at every count. The fit uses
/// every kept sample, so its σ is less noisy than the MAD's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearFitClipConfig {
    /// How far either side of the fitted centre a value may sit, in sigma.
    pub sigma: SigmaBounds,
    /// The most passes to run, the first clip included. The clip stops earlier when a fitted pass
    /// rejects nothing.
    pub max_passes: u32,
}

impl Default for LinearFitClipConfig {
    fn default() -> Self {
        Self {
            sigma: SigmaBounds::symmetric(3.0),
            max_passes: 4,
        }
    }
}

impl LinearFitClipConfig {
    /// Clipping to `sigma` about the fitted line, for at most `max_passes` passes, the first clip
    /// about the median among them.
    pub const fn new(sigma: SigmaBounds, max_passes: u32) -> Self {
        Self { sigma, max_passes }
    }

    /// Validate the clip thresholds and pass count.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        self.sigma.validate()?;
        validate_max_passes(self.max_passes)
    }

    pub(crate) fn narrow(&self, pass: &Pass<'_>, scores: &mut NormalScores) -> Proposal {
        if pass.index == 0 {
            return pass.clip_about_median(self.sigma, scores);
        }
        let fit = pass.rank_fit(scores);
        pass.keep(self.sigma, fit.centre, pass.floored(fit))
    }
}
