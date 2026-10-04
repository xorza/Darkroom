//! Linear-fit clipping: regress the sorted samples on their expected Gaussian positions, and clip
//! about the fitted centre in units of the fitted σ.

use crate::combine::rejection::normal_scores::NormalScores;
use crate::combine::rejection::pass::{Pass, Proposal};
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::combine::rejection::validate_max_iterations;
use crate::error::InvalidConfigField;
use crate::math::statistics::spread::Spread;

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
    /// The most fitted passes to run after the first clip. The clip stops earlier when a pass
    /// rejects nothing.
    pub max_iterations: u32,
}

impl Default for LinearFitClipConfig {
    fn default() -> Self {
        Self {
            sigma: SigmaBounds::symmetric(3.0),
            max_iterations: 3,
        }
    }
}

impl LinearFitClipConfig {
    pub const fn new(sigma_low: f32, sigma_high: f32, max_iterations: u32) -> Self {
        Self {
            sigma: SigmaBounds::asymmetric(sigma_low, sigma_high),
            max_iterations,
        }
    }

    /// Validate the clip thresholds and iteration count.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        self.sigma.validate()?;
        validate_max_iterations(self.max_iterations)
    }

    /// The first clip, and then the fitted ones.
    pub(crate) const fn passes(&self) -> usize {
        1 + self.max_iterations as usize
    }

    pub(crate) fn narrow(&self, pass: &Pass<'_>, scores: &mut NormalScores) -> Proposal {
        if pass.index == 0 {
            return pass.clip_about_median(self.sigma);
        }
        let fit = Self::fit(pass, scores);
        pass.keep(self.sigma, fit.centre, fit.floored(pass.background))
    }

    /// The least-squares line through the kept samples against their normal scores: the intercept
    /// is the centre and the slope the σ. Sums run in f64 about the means, so neither cancels.
    pub(crate) fn fit(pass: &Pass<'_>, scores: &mut NormalScores) -> Spread {
        let scores = &scores.of_count(pass.sorted.len())[pass.window.clone()];
        let samples = pass.samples();
        debug_assert!(samples.len() >= 2);
        let count = samples.len() as f64;
        let score_mean = scores.iter().sum::<f64>() / count;
        let sample_mean = samples.iter().map(|&v| f64::from(v)).sum::<f64>() / count;
        let mut score_squares = 0.0f64;
        let mut products = 0.0f64;
        for (&score, &sample) in scores.iter().zip(samples) {
            let score = score - score_mean;
            score_squares += score * score;
            products += score * (f64::from(sample) - sample_mean);
        }
        let slope = products / score_squares;
        Spread {
            centre: (sample_mean - slope * score_mean) as f32,
            sigma: slope as f32,
        }
    }
}
