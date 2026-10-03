//! Generalized Extreme Studentized Deviate: test for up to `r` outliers against Student-t critical
//! values.

use crate::combine::rejection::pass::{Pass, Proposal};
use crate::combine::rejection::scratch_buffers::GesdScratch;
use crate::error::InvalidConfigField;
use crate::math::statistics::spread::Spread;

/// Configuration for the Generalized Extreme Studentized Deviate test (Rosner 1983).
///
/// A test for up to `r` outliers in approximately Gaussian samples. It removes the sample farthest
/// from the mean `r` times, and the outliers are the removals up to the last one whose statistic
/// passes its critical value. The stacking preset enables it from 15 frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GesdConfig {
    /// Significance level for the test (typically 0.05).
    pub alpha: f32,
    /// The most outliers to test for. `None` tests for 30% of the samples, as Siril and PixInsight
    /// do: the test resists masking only when `r` reaches the true outlier count.
    pub max_outliers: Option<usize>,
}

impl Default for GesdConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            max_outliers: None,
        }
    }
}

impl GesdConfig {
    pub const fn new(alpha: f32, max_outliers: Option<usize>) -> Self {
        Self {
            alpha,
            max_outliers,
        }
    }

    /// Validate the significance level.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite("GESD alpha", "finite and in [0, 1)", self.alpha, |value| {
            (0.0..1.0).contains(&value)
        })
    }

    /// The configured maximum, or `⌊0.3·n⌋`.
    pub const fn max_outliers_for_size(&self, n: usize) -> usize {
        match self.max_outliers {
            Some(max_outliers) => max_outliers,
            None => 3 * n / 10,
        }
    }

    /// The test, over the whole window: one pass.
    ///
    /// The sample farthest from the mean of a sorted window is one of its ends, so each removal
    /// narrows the window. The number of removals is capped by `n − min_survivors`, and by `n − 2`:
    /// the critical value of a removal from `L` samples has `L − 2` degrees of freedom. The sample
    /// deviation is floored by [`Spread::floored`], so a window of tied integers is not read as one
    /// with no spread.
    pub(crate) fn narrow(&self, pass: &Pass<'_>, gesd: &mut GesdScratch) -> Option<Proposal> {
        let n = pass.window.len();
        let tests = self
            .max_outliers_for_size(n)
            .min(n - pass.min_survivors)
            .min(n.saturating_sub(2));
        if tests == 0 {
            return None;
        }
        gesd.prepare(self.alpha);
        gesd.statistics.clear();

        let mut mean = 0.0f64;
        let mut squared_deviations = 0.0f64;
        for (index, &value) in pass.samples().iter().enumerate() {
            let value = f64::from(value);
            let delta = value - mean;
            mean += delta / (index + 1) as f64;
            squared_deviations += delta * (value - mean);
        }
        let centre = mean as f32;

        let mut window = pass.window.clone();
        let mut kept = pass.window.clone();
        for removed in 0..tests {
            let live = n - removed;
            let deviation = Spread {
                centre: mean as f32,
                sigma: (squared_deviations / (live - 1) as f64).sqrt() as f32,
            }
            .floored(pass.background);
            let low = mean - f64::from(pass.sorted[window.start]);
            let high = f64::from(pass.sorted[window.end - 1]) - mean;
            // A tie removes the higher sample, so the lower position is the one kept.
            let removed_value = if high >= low {
                window.end -= 1;
                pass.sorted[window.end]
            } else {
                window.start += 1;
                pass.sorted[window.start - 1]
            };
            let statistic = high.max(low) / f64::from(deviation);
            gesd.statistics.push(statistic);
            if statistic > gesd.critical_value(live) {
                kept = window.clone();
            }
            let removed_value = f64::from(removed_value);
            let next_mean = mean - (removed_value - mean) / (live - 1) as f64;
            // Welford's update in reverse, so each candidate needs only the two ends.
            squared_deviations = (squared_deviations
                - (removed_value - mean) * (removed_value - next_mean))
                .max(0.0);
            mean = next_mean;
        }
        Some(Proposal {
            window: kept,
            centre,
        })
    }
}
