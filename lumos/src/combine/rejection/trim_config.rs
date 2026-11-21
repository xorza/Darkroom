//! Trimming: drop a fixed share of the sorted samples from each end, and average the rest.

use crate::combine::rejection::pass::{Narrowing, Pass, Proposal};
use crate::combine::rejection::percent;
use crate::combine::rejection::scratch_buffers::MethodScratch;
use crate::config_field::ConfigField;
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
    /// The share to drop from each end, in percent.
    pub percent: TrimBounds,
}

/// The shares a trim drops from the two ends of the sorted samples, in percent, each from 0 to 50
/// and together below 100: a pair, as [`SigmaBounds`](crate::SigmaBounds) is, so a low share
/// cannot reach the trim as its high one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrimBounds {
    /// The share to drop from the low end.
    pub low: f32,
    /// The share to drop from the high end.
    pub high: f32,
}

impl TrimBounds {
    /// The same share from each end.
    pub const fn symmetric(percent: f32) -> Self {
        Self {
            low: percent,
            high: percent,
        }
    }

    /// Separate shares, as for a trim of the bright end alone.
    pub const fn asymmetric(low: f32, high: f32) -> Self {
        Self { low, high }
    }
}

/// How many samples a [`TrimConfig`] drops from each end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrimCounts {
    pub(crate) low: usize,
    pub(crate) high: usize,
}

impl Default for TrimConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TrimConfig {
    /// 10% from each end.
    pub const DEFAULT: Self = Self {
        percent: TrimBounds::symmetric(10.0),
    };

    /// Validate that each end drops a sane share and that together they leave survivors.
    pub(crate) fn validate(self) -> Result<(), InvalidConfigField> {
        let TrimBounds { low, high } = self.percent;
        ConfigField::new("percent.low", low)
            .finite_and("finite and between 0 and 50", |value| {
                (0.0..=50.0).contains(&value)
            })?;
        ConfigField::new("percent.high", high)
            .finite_and("finite and between 0 and 50", |value| {
                (0.0..=50.0).contains(&value)
            })?;
        let total = low + high;
        ConfigField::new("percent.low + percent.high", total).must_be("below 100", total < 100.0)
    }

    /// The counts for `n` samples, exact.
    pub(crate) fn counts(self, n: usize) -> TrimCounts {
        TrimCounts {
            low: percent::share_of(self.percent.low, n),
            high: percent::share_of(self.percent.high, n),
        }
    }

    /// The trim, over the whole window: one pass. The two counts leave at least one sample, since
    /// validation holds their shares below 100% together; the driver raises that to its minimum.
    pub(crate) fn narrow(self, pass: &Pass<'_>, _: &mut MethodScratch) -> Narrowing {
        let TrimCounts { low, high } = self.counts(pass.window.len());
        debug_assert!(low + high < pass.window.len());
        Narrowing::Window(Proposal {
            window: pass.window.start + low..pass.window.end - high,
            centre: Spread::median_of_sorted(pass.samples()),
        })
    }
}
