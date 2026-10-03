//! Trimming: drop a fixed share of the sorted samples from each end, and average the rest.

use crate::combine::rejection::pass::{Pass, Proposal};
use crate::error::InvalidConfigField;
use crate::math::statistics::spread::Spread;

/// Configuration for a trimmed mean.
///
/// Drops `⌊p·n/100⌋` samples from each end of the `n` sorted samples. It measures no spread, so it
/// works on stacks too small to estimate one from, but it drops samples from a clean pixel too, and
/// it drops nothing until `p·n` reaches 100. Siril's percentile clipping is a different method: it
/// rejects by the deviation from the median as a fraction of the median.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrimConfig {
    /// The share to drop from the low end, in percent, from 0 to 50.
    pub low_percent: f32,
    /// The share to drop from the high end, in percent, from 0 to 50.
    pub high_percent: f32,
}

/// How many samples a [`TrimConfig`] drops from each end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrimCounts {
    pub(crate) low: usize,
    pub(crate) high: usize,
}

impl Default for TrimConfig {
    fn default() -> Self {
        Self {
            low_percent: 10.0,
            high_percent: 10.0,
        }
    }
}

impl TrimConfig {
    pub const fn new(low_percent: f32, high_percent: f32) -> Self {
        Self {
            low_percent,
            high_percent,
        }
    }

    /// Validate that each end drops a sane share and that together they leave survivors.
    pub(crate) fn validate(self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite(
            "low_percent",
            "finite and between 0 and 50",
            self.low_percent,
            |value| (0.0..=50.0).contains(&value),
        )?;
        InvalidConfigField::finite(
            "high_percent",
            "finite and between 0 and 50",
            self.high_percent,
            |value| (0.0..=50.0).contains(&value),
        )?;
        let total = self.low_percent + self.high_percent;
        InvalidConfigField::check(
            total < 100.0,
            "low_percent + high_percent",
            "below 100",
            total,
        )
    }

    /// The counts for `n` samples, exact. `p·n` is exact in f64, a 24-bit significand times an
    /// integer below 2²⁹, and so are the multiples of 100 near it; only the quotient rounds, so the
    /// multiples settle its floor. `(p/100)·n` in f32 gives 62 for 42% of 150.
    #[expect(
        clippy::cast_sign_loss,
        reason = "validate holds both percentages in [0, 50]"
    )]
    pub(crate) fn counts(self, n: usize) -> TrimCounts {
        debug_assert!(n < 1 << 29);
        let count = |percent: f32| {
            let product = f64::from(percent) * n as f64;
            let mut count = (product / 100.0).floor();
            if count * 100.0 > product {
                count -= 1.0;
            } else if (count + 1.0) * 100.0 <= product {
                count += 1.0;
            }
            count as usize
        };
        TrimCounts {
            low: count(self.low_percent),
            high: count(self.high_percent),
        }
    }

    /// The trim, over the whole window: one pass. The two counts leave at least one sample, since
    /// validation holds their shares below 100% together; the driver raises that to its minimum.
    pub(crate) fn narrow(self, pass: &Pass<'_>) -> Proposal {
        let TrimCounts { low, high } = self.counts(pass.window.len());
        debug_assert!(low + high < pass.window.len());
        Proposal {
            window: pass.window.start + low..pass.window.end - high,
            centre: Spread::median_of_sorted(pass.samples()),
        }
    }
}
