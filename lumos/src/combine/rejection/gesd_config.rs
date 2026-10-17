//! Generalized Extreme Studentized Deviate: test for up to `r` outliers against Student-t critical
//! values.

use std::ops::Range;

use crate::combine::rejection::pass::{Narrowing, Pass, Proposal};
use crate::combine::rejection::scratch_buffers::MethodScratch;
use crate::config_value::ConfigValue;
use crate::error::InvalidConfigField;
use crate::math::statistics::spread::Spread;

/// Configuration for the Generalized Extreme Studentized Deviate test (Rosner 1983).
///
/// A test for up to `r` outliers in approximately Gaussian samples. It removes the sample farthest
/// from the mean `r` times, and the outliers are the removals up to the last one whose statistic
/// passes its critical value. The stacking preset enables it from 15 frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GesdConfig {
    /// Significance level for the test (typically 0.05), in `(0, 1)`: at 0 the critical value is
    /// the Samuelson bound, which no sample reaches, and the test never rejects.
    pub alpha: f32,
    /// The most outliers to test for, at least 1. `None` tests for 30% of the samples, as Siril and
    /// PixInsight do: the test resists masking only when `r` reaches the true outlier count.
    pub max_outliers: Option<usize>,
}

impl Default for GesdConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl GesdConfig {
    /// A significance of 5%, and up to 30% of the samples tested.
    pub const DEFAULT: Self = Self {
        alpha: 0.05,
        max_outliers: None,
    };

    pub const fn new(alpha: f32, max_outliers: Option<usize>) -> Self {
        Self {
            alpha,
            max_outliers,
        }
    }

    /// Validate the significance level and the outlier cap: either at its bound disables the test.
    pub(crate) fn validate(self) -> Result<(), InvalidConfigField> {
        ConfigValue::new("alpha", self.alpha)
            .finite_and("finite and in (0, 1)", |value| value > 0.0 && value < 1.0)?;
        match self.max_outliers {
            Some(max_outliers) => ConfigValue::count("max_outliers", max_outliers)
                .must_be("at least 1", max_outliers >= 1),
            None => Ok(()),
        }
    }

    /// The configured maximum, or `⌊0.3·n⌋`.
    pub(super) const fn max_outliers_for_size(self, n: usize) -> usize {
        match self.max_outliers {
            Some(max_outliers) => max_outliers,
            None => 3 * n / 10,
        }
    }

    /// The test, over the whole window: one pass. The outliers are the removals up to the last
    /// one whose statistic passes its critical value.
    pub(crate) fn narrow(self, pass: &Pass<'_>, scratch: &mut MethodScratch) -> Narrowing {
        let tests = self.tests(pass);
        if tests == 0 {
            return Narrowing::Settled;
        }
        let gesd = &mut scratch.gesd;
        gesd.prepare(self.alpha);
        let mut kept = pass.window.clone();
        let centre = self.removals(pass, tests, |removal| {
            if removal.statistic > gesd.critical_value(removal.live) {
                kept = removal.window;
            }
        });
        Narrowing::Window(Proposal {
            window: kept,
            centre,
        })
    }

    /// How many removals the test makes: the cap, and at most `n − min_survivors` and `n − 2`,
    /// since the critical value of a removal from `L` samples has `L − 2` degrees of freedom.
    fn tests(self, pass: &Pass<'_>) -> usize {
        let n = pass.window.len();
        self.max_outliers_for_size(n)
            .min(n - pass.min_survivors)
            .min(n.saturating_sub(2))
    }

    /// Remove the sample farthest from the mean `tests` times, handing each removal to `visit`;
    /// the mean of the whole window.
    ///
    /// The sample farthest from the mean of a sorted window is one of its ends, so each removal
    /// narrows the window. The sample deviation is floored by [`Spread::floored`], so a window of
    /// tied integers is not read as one with no spread.
    fn removals(self, pass: &Pass<'_>, tests: usize, mut visit: impl FnMut(Removal)) -> f32 {
        let n = pass.window.len();
        let mut mean = 0.0f64;
        let mut squared_deviations = 0.0f64;
        for (index, &value) in pass.samples().iter().enumerate() {
            let value = f64::from(value);
            let delta = value - mean;
            mean += delta / (index + 1) as f64;
            squared_deviations += delta * (value - mean);
        }
        let centre = mean as f32;

        // The model σ at the first mean: the removals move the mean by a few σ at most, and the
        // floor is a level the model sets, not a fit to the samples.
        let floor = pass.model_sigma(centre);
        let mut window = pass.window.clone();
        for removed in 0..tests {
            let live = n - removed;
            let deviation = Spread {
                centre: mean as f32,
                sigma: (squared_deviations / (live - 1) as f64).sqrt() as f32,
            }
            .floored(floor);
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
            visit(Removal {
                live,
                statistic: high.max(low) / f64::from(deviation),
                window: window.clone(),
            });
            let removed_value = f64::from(removed_value);
            let next_mean = mean - (removed_value - mean) / (live - 1) as f64;
            // Welford's update in reverse, so each candidate needs only the two ends.
            squared_deviations = (squared_deviations
                - (removed_value - mean) * (removed_value - next_mean))
                .max(0.0);
            mean = next_mean;
        }
        centre
    }
}

/// One removal of the test: the samples it was made from, its statistic, and the window it left.
#[derive(Debug)]
struct Removal {
    live: usize,
    statistic: f64,
    window: Range<usize>,
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::combine::rejection::gesd_config::GesdConfig;
    use crate::combine::rejection::pass::Pass;

    impl GesdConfig {
        /// Each removal's statistic over `pass`, in order: what a test compares with the critical
        /// values a reference prints.
        pub(crate) fn statistics(&self, pass: &Pass<'_>) -> Vec<f64> {
            let mut statistics = Vec::new();
            self.removals(pass, self.tests(pass), |removal| {
                statistics.push(removal.statistic);
            });
            statistics
        }
    }
}
