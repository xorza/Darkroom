//! Percentile clipping: drop a fixed fraction from each end. Simple, and the only method that
//! stays meaningful on stacks too small to estimate a spread from.

use crate::error::InvalidConfigField;
use crate::stacking::combine::rejection::scratch_buffers::ScratchBuffers;

/// Configuration for percentile clipping.
///
/// Rejects the lowest and highest percentile of values.
/// Simple and effective for small stacks (< 10 frames).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PercentileClipConfig {
    /// Percentile to clip from the low end (0.0 to 50.0).
    pub low_percentile: f32,
    /// Percentile to clip from the high end (0.0 to 50.0).
    pub high_percentile: f32,
}

impl Default for PercentileClipConfig {
    fn default() -> Self {
        Self {
            low_percentile: 10.0,
            high_percentile: 10.0,
        }
    }
}

impl PercentileClipConfig {
    pub fn new(low_percentile: f32, high_percentile: f32) -> Self {
        Self {
            low_percentile,
            high_percentile,
        }
    }

    /// Validate that each end clips a sane share and that together they leave survivors.
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite(
            "low_percentile",
            "finite and between 0 and 50",
            self.low_percentile,
            |value| (0.0..=50.0).contains(&value),
        )?;
        InvalidConfigField::finite(
            "high_percentile",
            "finite and between 0 and 50",
            self.high_percentile,
            |value| (0.0..=50.0).contains(&value),
        )?;
        let total = self.low_percentile + self.high_percentile;
        InvalidConfigField::check(
            total < 100.0,
            "low_percentile + high_percentile",
            "below 100",
            total,
        )
    }

    /// Compute the surviving index range for a sorted array of length `n`.
    ///
    /// Returns the half-open range of elements to keep after clipping
    /// the lowest `low_percentile`% and highest `high_percentile`%.
    /// Guarantees at least one element survives.
    pub fn surviving_range(&self, n: usize) -> std::ops::Range<usize> {
        let low_count = ((self.low_percentile / 100.0) * n as f32).floor() as usize;
        let high_count = ((self.high_percentile / 100.0) * n as f32).floor() as usize;
        let start = low_count;
        let end = n.saturating_sub(high_count);
        if start >= end {
            let mid = n / 2;
            mid..mid + 1
        } else {
            start..end
        }
    }

    /// Partition values by percentile clipping, returning the number of survivors.
    ///
    /// Sorts values (with index co-array) and moves the surviving middle range
    /// to `values[..remaining]` and `indices[..remaining]`.
    pub(super) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        let n = values.len();
        scratch.reset_indices(n);

        if n <= 2 {
            return n;
        }

        scratch.sort_with_indices(values, n);

        let range = self.surviving_range(n);
        let count = range.len();

        if range.start > 0 {
            values.copy_within(range.clone(), 0);
            scratch.indices.copy_within(range, 0);
        }

        count
    }
}
