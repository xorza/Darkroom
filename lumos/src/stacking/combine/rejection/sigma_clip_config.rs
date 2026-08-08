//! Iterative kappa-sigma clipping: reject beyond `k` sigma of the median, repeat.

use crate::error::InvalidConfigField;
use crate::math::statistics::mad_to_sigma;
use crate::stacking::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::stacking::combine::rejection::{sorted_mad, validate_sigma_bounds};

/// Configuration for sigma clipping.
///
/// Supports both symmetric and asymmetric thresholds. For symmetric clipping,
/// use `new()` which sets `sigma_low == sigma_high`. For asymmetric clipping
/// (e.g. aggressive rejection of bright outliers like satellites/cosmic rays),
/// use `new_asymmetric()` with separate low/high thresholds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SigmaClipConfig {
    /// Sigma threshold for low outliers (below median).
    pub sigma_low: f32,
    /// Sigma threshold for high outliers (above median).
    pub sigma_high: f32,
    /// Maximum number of iterations for iterative clipping.
    pub max_iterations: u32,
}

impl Default for SigmaClipConfig {
    fn default() -> Self {
        Self {
            sigma_low: 2.5,
            sigma_high: 2.5,
            max_iterations: 3,
        }
    }
}

impl SigmaClipConfig {
    /// Create symmetric sigma clipping (same threshold for low and high).
    pub fn new(sigma: f32, max_iterations: u32) -> Self {
        Self {
            sigma_low: sigma,
            sigma_high: sigma,
            max_iterations,
        }
    }

    /// Create asymmetric sigma clipping with separate low/high thresholds.
    pub fn new_asymmetric(sigma_low: f32, sigma_high: f32, max_iterations: u32) -> Self {
        Self {
            sigma_low,
            sigma_high,
            max_iterations,
        }
    }

    /// Validate the clip thresholds and iteration count.
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        validate_sigma_bounds(self.sigma_low, self.sigma_high)?;
        InvalidConfigField::check(
            self.max_iterations >= 1,
            "max_iterations",
            "at least 1",
            self.max_iterations as f64,
        )
    }

    /// Partition values by sigma clipping, returning the number of survivors.
    ///
    /// After return, `values[..remaining]` contains surviving values and
    /// `indices[..remaining]` contains their original frame indices.
    /// Supports both symmetric (`sigma_low == sigma_high`) and asymmetric thresholds.
    ///
    /// When rejection is actually warranted, `values` (with its co-indices) is sorted **once**.
    /// Each iteration rejects a `[center − kσ, center + kσ]` band, which on sorted data is a
    /// *contiguous* slice — so the active window shrinks from both ends (binary-searched bounds)
    /// and stays sorted. The median is then the middle element (O(1)) and the MAD a single bitonic
    /// scan ([`sorted_mad`]), replacing the two per-iteration quickselects. Survivors are compacted
    /// to the front only at the end. Sorting keeps `values[i]` paired with `indices[i]` throughout
    /// (the previous quickselect reordered values without their indices, mis-pairing weights in the
    /// weighted combine); the survivor *set* — hence count and unweighted mean — is unchanged.
    ///
    /// The cheap `no_outliers_possible` screen runs **before** sorting: clean pixels (the majority
    /// in a smooth flat/light) can't reject anything, so they skip the sort entirely.
    pub(super) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        scratch.reset_indices(values.len());

        let n0 = values.len();
        if n0 <= 2 {
            return n0;
        }

        let min_sigma = self.sigma_low.min(self.sigma_high);

        // Fast path: if no value can exceed the threshold, nothing is rejected — return without
        // paying for the sort. (Order-independent, so it's valid on the unsorted input.)
        if Self::no_outliers_possible(values, min_sigma) {
            return n0;
        }

        scratch.sort_with_indices(values, n0);

        // Active survivors are the sorted, contiguous window `values[lo..hi]`.
        let mut lo = 0usize;
        let mut hi = n0;

        for _ in 0..self.max_iterations {
            let len = hi - lo;
            if len <= 2 {
                break;
            }

            let active = &values[lo..hi];

            // Re-screen the shrunken window: once it's clean, no further iteration can reject.
            if Self::no_outliers_possible(active, min_sigma) {
                break;
            }

            let center = active[len / 2];
            let sigma = mad_to_sigma(sorted_mad(active, center));

            if sigma < f32::EPSILON {
                break;
            }

            // Keep `center − sigma_low·σ <= v <= center + sigma_high·σ`. On sorted data this is a
            // contiguous run; binary-search its inclusive bounds.
            let low_cut = center - self.sigma_low * sigma;
            let high_cut = center + self.sigma_high * sigma;
            let new_lo = lo + active.partition_point(|&v| v < low_cut);
            let new_hi = lo + active.partition_point(|&v| v <= high_cut);

            if new_lo == lo && new_hi == hi {
                break; // nothing rejected
            }
            lo = new_lo;
            hi = new_hi;
        }

        // Compact survivors to the front (the documented `[..remaining]` contract).
        let remaining = hi - lo;
        if lo > 0 {
            values.copy_within(lo..hi, 0);
            scratch.indices.copy_within(lo..hi, 0);
        }
        remaining
    }

    /// Check if no point can be rejected, using a cheap range-based estimate.
    ///
    /// Uses Welford's single-pass algorithm to compute mean and variance together
    /// with min/max tracking. Excludes the single most extreme min and max from
    /// the variance estimate for robustness. If the maximum deviation from the
    /// trimmed center is within the threshold, the full median+MAD can be skipped.
    ///
    /// Only applied for N >= 10 (below that, trimming distorts the estimate too much).
    #[inline]
    pub(super) fn no_outliers_possible(values: &[f32], min_sigma_k: f32) -> bool {
        let n = values.len();
        if n < 10 {
            return false;
        }

        // Single pass: compute sum, sum_sq, min, max. Accumulate in f64 — the variance below is
        // the cancellation-prone `E[X²] − E[X]²` form, which in f32 over many bright pixels loses
        // most significant bits (and can go negative), spuriously tripping the constant-data exit.
        let mut sum = 0.0f64;
        let mut sum_sq = 0.0f64;
        let mut min1 = f32::MAX;
        let mut max1 = f32::MIN;
        for &v in values {
            sum += v as f64;
            sum_sq += (v as f64) * (v as f64);
            if v < min1 {
                min1 = v;
            }
            if v > max1 {
                max1 = v;
            }
        }
        let (min1, max1) = (min1 as f64, max1 as f64);

        // Trimmed mean and variance: exclude the single most extreme min and max
        let trimmed_n = (n - 2) as f64;
        let trimmed_sum = sum - min1 - max1;
        let trimmed_mean = trimmed_sum / trimmed_n;
        let trimmed_sum_sq = sum_sq - min1 * min1 - max1 * max1;
        // Var = E[X²] - E[X]² with Bessel's correction
        let variance = (trimmed_sum_sq - trimmed_sum * trimmed_sum / trimmed_n) / (trimmed_n - 1.0);

        if variance < f32::EPSILON as f64 {
            // Trimmed data is constant. The full path would compute MAD=0, sigma=0
            // and break without rejecting. Early exit matches that behavior.
            return true;
        }

        let stddev = variance.sqrt();

        // Check: can any value exceed the threshold from the trimmed center?
        let max_dev = (max1 - trimmed_mean).abs().max((min1 - trimmed_mean).abs());
        max_dev <= min_sigma_k as f64 * stddev
    }
}
