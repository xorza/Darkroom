//! Pixel rejection algorithms for stacking.
//!
//! This module contains various outlier rejection methods used during image stacking:
//! - Sigma clipping (Kappa-Sigma)
//! - Winsorized sigma clipping
//! - Linear fit clipping
//! - Percentile clipping
//! - Generalized Extreme Studentized Deviate (GESD)

use crate::math::statistics::{mad_f32_fast, mad_to_sigma, median_f32_fast};
use crate::math::sum::{mean_f32, weighted_mean_f32};
use crate::stacking::combine::cache::{CombinedSample, ScratchBuffers};
use statrs::distribution::{ContinuousCDF, StudentsT};

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
    pub(crate) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        reset_indices(&mut scratch.indices, values.len());

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

        sort_with_indices(
            values,
            &mut scratch.indices,
            &mut scratch.floats_b,
            &mut scratch.usize_a,
            &mut scratch.usize_b,
            n0,
        );

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
    fn no_outliers_possible(values: &[f32], min_sigma_k: f32) -> bool {
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

/// Configuration for winsorized sigma clipping.
///
/// Two-phase algorithm matching PixInsight/Siril:
/// 1. **Robust estimation**: Iteratively Winsorize with Huber's c=1.5 constant
///    until sigma converges, then apply 1.134 bias correction to get robust
///    (center, sigma) estimates.
/// 2. **Rejection**: Standard sigma clipping using the robust estimates and
///    user-specified sigma_low/sigma_high thresholds.
///
/// This is more robust for small sample sizes than standard sigma clipping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WinsorizedClipConfig {
    /// Sigma threshold for low outliers (below median).
    pub sigma_low: f32,
    /// Sigma threshold for high outliers (above median).
    pub sigma_high: f32,
}

/// Huber's constant for Winsorization boundaries.
const HUBER_C: f32 = 1.5;
/// Bias correction factor for Winsorized standard deviation.
const WINSORIZED_CORRECTION: f32 = 1.134;
/// Convergence threshold for iterative Winsorization.
const WINSORIZE_CONVERGENCE: f32 = 0.0005;
/// Maximum iterations for Winsorization convergence.
const WINSORIZE_MAX_ITER: u32 = 50;

impl Default for WinsorizedClipConfig {
    fn default() -> Self {
        Self {
            sigma_low: 2.5,
            sigma_high: 2.5,
        }
    }
}

impl WinsorizedClipConfig {
    pub fn new(sigma: f32) -> Self {
        Self {
            sigma_low: sigma,
            sigma_high: sigma,
        }
    }

    pub fn new_asymmetric(sigma_low: f32, sigma_high: f32) -> Self {
        Self {
            sigma_low,
            sigma_high,
        }
    }

    /// Phase 1: Iteratively Winsorize to get robust (center, sigma) estimates.
    ///
    /// Uses Huber's c=1.5 for Winsorization boundaries, converges when
    /// `|sigma_new - sigma_old| / sigma_old < 0.0005`. Applies 1.134 bias
    /// correction to the final sigma.
    ///
    /// `working` is sorted **once** up front: Winsorization clamps every value into
    /// `[low, high]`, a monotonic map, so a sorted buffer stays sorted across iterations.
    /// The median is then the middle element (O(1)) every pass — replacing the per-iteration
    /// quickselect + buffer copy that dominated this hot path. `winsorized_stddev` is an
    /// order-independent sum, so sorting changes neither the center nor the sigma.
    ///
    /// Returns (center, corrected_sigma).
    fn robust_estimate(&self, values: &[f32], working: &mut Vec<f32>) -> (f32, f32) {
        working.clear();
        working.extend_from_slice(values);
        working.sort_unstable_by(f32::total_cmp);

        // `select_nth_unstable`'s median (index len/2) equals the sorted element at that index,
        // so `working[mid]` reproduces the previous `median_f32_fast` result exactly.
        let mid = working.len() / 2;
        let mut center = working[mid];
        let mut sigma = winsorized_stddev(working, center) * WINSORIZED_CORRECTION;

        if sigma < f32::EPSILON {
            return (center, 0.0);
        }

        for _ in 0..WINSORIZE_MAX_ITER {
            let low_bound = center - HUBER_C * sigma;
            let high_bound = center + HUBER_C * sigma;

            // Clamp outliers to the boundary values. `low_bound <= high_bound` (sigma > 0), and
            // a monotone clamp preserves the existing sort order, so no re-sort is needed.
            for v in working.iter_mut() {
                *v = v.clamp(low_bound, high_bound);
            }

            center = working[mid];
            let sigma_new = winsorized_stddev(working, center) * WINSORIZED_CORRECTION;

            if sigma_new < f32::EPSILON {
                return (center, 0.0);
            }

            let converged = (sigma_new - sigma).abs() <= sigma * WINSORIZE_CONVERGENCE;
            sigma = sigma_new;

            if converged {
                break;
            }
        }

        (center, sigma)
    }

    /// Phase 2: Reject outliers using the robust estimates from phase 1.
    ///
    /// Standard sigma clipping with the Winsorized (center, sigma) and
    /// the user's sigma_low/sigma_high thresholds.
    pub(crate) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        reset_indices(&mut scratch.indices, values.len());

        if values.len() <= 2 {
            return values.len();
        }

        let (center, sigma) = self.robust_estimate(values, &mut scratch.floats_a);

        if sigma < f32::EPSILON {
            return values.len();
        }

        let n = values.len();
        compact_within(
            values,
            &mut scratch.indices,
            n,
            self.sigma_low * sigma,
            self.sigma_high * sigma,
            |_| center,
        )
    }
}

/// Sample standard deviation of `values` about the given `center` (not MAD) — the spread estimate
/// the Winsorized robust loop iterates on.
fn winsorized_stddev(values: &[f32], center: f32) -> f32 {
    let n = values.len() as f32;
    if n <= 1.0 {
        return 0.0;
    }
    let variance = values
        .iter()
        .map(|&v| {
            let d = v - center;
            d * d
        })
        .sum::<f32>()
        / (n - 1.0);
    variance.sqrt()
}

/// Configuration for linear fit clipping.
///
/// Fits a linear relationship between each pixel and a reference value,
/// then rejects pixels that deviate significantly from the fit.
/// Works well with images containing sky gradients.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearFitClipConfig {
    /// Sigma threshold for low outliers (below the fit).
    pub sigma_low: f32,
    /// Sigma threshold for high outliers (above the fit).
    pub sigma_high: f32,
    /// Maximum number of iterations.
    pub max_iterations: u32,
}

impl Default for LinearFitClipConfig {
    fn default() -> Self {
        Self {
            sigma_low: 3.0,
            sigma_high: 3.0,
            max_iterations: 3,
        }
    }
}

impl LinearFitClipConfig {
    pub fn new(sigma_low: f32, sigma_high: f32, max_iterations: u32) -> Self {
        Self {
            sigma_low,
            sigma_high,
            max_iterations,
        }
    }

    /// Partition values by linear fit clipping, returning the number of survivors.
    ///
    /// First pass uses median + MAD for initial rejection (robust starting point).
    /// Subsequent passes sort survivors, fit a line through `(sorted_index, value)`,
    /// compute mean absolute deviation of residuals as sigma, and reject each pixel
    /// against its own fitted value. Matches PixInsight/Siril linear fit rejection.
    ///
    /// After return, `values[..remaining]` contains surviving values and
    /// `indices[..remaining]` contains their original frame indices.
    pub(crate) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        reset_indices(&mut scratch.indices, values.len());

        if values.len() <= 3 {
            return values.len();
        }

        let mut len = values.len();

        for iteration in 0..self.max_iterations {
            if len <= 3 {
                break;
            }

            if iteration == 0 {
                // Initial pass: median + MAD sigma clipping (robust starting point)
                scratch.floats_a.clear();
                scratch.floats_a.extend_from_slice(&values[..len]);
                let center = median_f32_fast(&mut scratch.floats_a);
                let mad = mad_f32_fast(&values[..len], center, &mut scratch.floats_a);
                let sigma = mad_to_sigma(mad);

                if sigma < f32::EPSILON {
                    break;
                }

                // No early break when the seed pass rejects nothing: the linear-fit passes below
                // must still run. A trend-hidden outlier (the case LinearFit targets) sits within
                // sigma·MAD here — it's only exposed after fitting out the trend.
                len = compact_within(
                    values,
                    &mut scratch.indices,
                    len,
                    self.sigma_low * sigma,
                    self.sigma_high * sigma,
                    |_| center,
                );
            } else {
                // Subsequent passes: linear fit rejection

                // Sort remaining values with index co-array
                sort_with_indices(
                    values,
                    &mut scratch.indices,
                    &mut scratch.floats_b,
                    &mut scratch.usize_a,
                    &mut scratch.usize_b,
                    len,
                );

                // Fit line y = a + b*x through sorted values, x = sorted position
                let n = len as f32;
                let mut sum_x = 0.0f32;
                let mut sum_y = 0.0f32;
                let mut sum_xy = 0.0f32;
                let mut sum_xx = 0.0f32;

                for (i, &v) in values[..len].iter().enumerate() {
                    let x = i as f32;
                    sum_x += x;
                    sum_y += v;
                    sum_xy += x * v;
                    sum_xx += x * x;
                }

                let denom = n * sum_xx - sum_x * sum_x;
                if denom.abs() < f32::EPSILON {
                    break;
                }

                let b = (n * sum_xy - sum_x * sum_y) / denom;
                let a = (sum_y - b * sum_x) / n;

                // Sigma = mean absolute deviation of residuals from fit
                let sigma: f32 = values[..len]
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| (v - (a + b * i as f32)).abs())
                    .sum::<f32>()
                    / n;

                if sigma < f32::EPSILON {
                    break;
                }

                // Reject each pixel against its own fitted value.
                let write_idx = compact_within(
                    values,
                    &mut scratch.indices,
                    len,
                    self.sigma_low * sigma,
                    self.sigma_high * sigma,
                    |i| a + b * i as f32,
                );

                if write_idx == len {
                    break;
                }
                len = write_idx;
            }
        }

        len
    }
}

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
    pub(crate) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        let n = values.len();
        reset_indices(&mut scratch.indices, n);

        if n <= 2 {
            return n;
        }

        sort_with_indices(
            values,
            &mut scratch.indices,
            &mut scratch.floats_a,
            &mut scratch.usize_a,
            &mut scratch.usize_b,
            n,
        );

        let range = self.surviving_range(n);
        let count = range.len();

        if range.start > 0 {
            values.copy_within(range.clone(), 0);
            scratch.indices.copy_within(range, 0);
        }

        count
    }
}

/// Configuration for Generalized Extreme Studentized Deviate (GESD) test.
///
/// A rigorous statistical test for detecting multiple outliers in approximately Gaussian samples.
/// The stacking preset enables it from 15 frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GesdConfig {
    /// Significance level for the test (typically 0.05).
    pub alpha: f32,
    /// Maximum number of outliers to detect.
    /// If `None`, targets 25% of the data within Rosner's validated limits: at most two
    /// candidates below 25 samples and at most ten otherwise.
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
    pub fn new(alpha: f32, max_outliers: Option<usize>) -> Self {
        Self {
            alpha,
            max_outliers,
        }
    }

    /// Get the configured maximum or the validation-constrained automatic maximum.
    pub fn max_outliers_for_size(&self, n: usize) -> usize {
        self.max_outliers.unwrap_or_else(|| {
            let validation_cap = if n < 25 { 2 } else { 10 };
            (n / 4).min(validation_cap)
        })
    }

    /// Partition values by GESD test, returning the number of survivors.
    ///
    /// Uses Rosner's two-sided statistic: distance from the sample mean divided by sample standard
    /// deviation, with critical values from the Student-t inverse CDF.
    ///
    /// After return, `values[..remaining]` contains surviving values and
    /// `indices[..remaining]` contains their original frame indices.
    pub(crate) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        let original_len = values.len();
        reset_indices(&mut scratch.indices, original_len);

        if original_len <= 3 {
            return original_len;
        }

        let max_outliers = self
            .max_outliers_for_size(original_len)
            .min(original_len - 3);
        prepare_gesd_critical_values(self, original_len, max_outliers, scratch);
        let mut len = original_len;
        let mut mean = 0.0f64;
        let mut squared_deviations = 0.0f64;
        for (index, &value) in values.iter().enumerate() {
            let value = f64::from(value);
            let delta = value - mean;
            mean += delta / (index + 1) as f64;
            squared_deviations += delta * (value - mean);
        }

        scratch.gesd_statistics.clear();

        for _ in 0..max_outliers {
            let sample_deviation = (squared_deviations / (len - 1) as f64).sqrt();
            if sample_deviation == 0.0 {
                break;
            }

            let mut max_deviation = 0.0f64;
            let mut max_idx = 0;
            for (idx, &value) in values[..len].iter().enumerate() {
                let deviation = (f64::from(value) - mean).abs();
                if deviation > max_deviation {
                    max_deviation = deviation;
                    max_idx = idx;
                }
            }

            scratch
                .gesd_statistics
                .push(max_deviation / sample_deviation);

            let removed = f64::from(values[max_idx]);
            let next_len = len - 1;
            let next_mean = mean - (removed - mean) / next_len as f64;
            // Reverse Welford's update so each candidate needs only the extreme-value scan.
            squared_deviations =
                (squared_deviations - (removed - mean) * (removed - next_mean)).max(0.0);
            mean = next_mean;
            values.swap(max_idx, len - 1);
            scratch.indices.swap(max_idx, len - 1);
            len = next_len;
        }

        let num_outliers = scratch
            .gesd_statistics
            .iter()
            .zip(&scratch.gesd_critical_values)
            .rposition(|(statistic, critical)| statistic > critical)
            .map_or(0, |index| index + 1);
        original_len - num_outliers
    }
}

fn prepare_gesd_critical_values(
    config: &GesdConfig,
    sample_count: usize,
    max_outliers: usize,
    scratch: &mut ScratchBuffers,
) {
    if scratch.gesd_sample_count == sample_count
        && scratch.gesd_critical_values.len() == max_outliers
        && scratch.gesd_alpha_bits == config.alpha.to_bits()
    {
        return;
    }

    scratch.gesd_critical_values.clear();
    for removed in 0..max_outliers {
        let live_count = sample_count - removed;
        let live = live_count as f64;
        let probability = 1.0 - f64::from(config.alpha) / (2.0 * live);
        let distribution = StudentsT::new(0.0, 1.0, (live_count - 2) as f64)
            .expect("GESD live sample count guarantees positive degrees of freedom");
        let critical_t = distribution.inverse_cdf(probability);
        let critical =
            (live - 1.0) / (live * (1.0 + (live - 2.0) / (critical_t * critical_t))).sqrt();
        scratch.gesd_critical_values.push(critical);
    }

    scratch.gesd_sample_count = sample_count;
    scratch.gesd_alpha_bits = config.alpha.to_bits();
}

/// Reset an indices buffer to [0, 1, 2, ...n), reusing the allocation.
fn reset_indices(indices: &mut Vec<usize>, n: usize) {
    indices.clear();
    indices.extend(0..n);
}

/// Keep predicate for asymmetric sigma rejection: `diff = value − reference`, kept when it lies
/// within `[−low, high]` (the low- and high-side thresholds are applied separately).
#[inline]
fn within_threshold(diff: f32, low: f32, high: f32) -> bool {
    if diff < 0.0 {
        -diff <= low
    } else {
        diff <= high
    }
}

/// Compact in place the first `count` values (with their co-`indices`) whose deviation from
/// `reference(i)` stays within the asymmetric band `[−low, high]`, returning the survivor count.
/// Survivors keep their relative order and stay paired with their indices. `reference` is the
/// per-element comparison point — a constant `center` for sigma/winsorized clipping, or the fitted
/// `a + b·i` for linear-fit clipping.
fn compact_within(
    values: &mut [f32],
    indices: &mut [usize],
    count: usize,
    low: f32,
    high: f32,
    reference: impl Fn(usize) -> f32,
) -> usize {
    let mut write = 0;
    for read in 0..count {
        if within_threshold(values[read] - reference(read), low, high) {
            values[write] = values[read];
            indices[write] = indices[read];
            write += 1;
        }
    }
    write
}

/// Sort `values[..n]` and `indices[..n]` together by value.
/// Uses insertion sort for small N (optimal for typical 10–50 frame stacks)
/// and introsort via `sort_unstable_by` for large N to avoid O(N^2).
fn sort_with_indices(
    values: &mut [f32],
    indices: &mut [usize],
    scratch_buf: &mut Vec<f32>,
    perm: &mut Vec<usize>,
    old_indices: &mut Vec<usize>,
    n: usize,
) {
    const INSERTION_SORT_THRESHOLD: usize = 64;

    if n <= INSERTION_SORT_THRESHOLD {
        for i in 1..n {
            let mut j = i;
            while j > 0 && values[j - 1] > values[j] {
                values.swap(j - 1, j);
                indices.swap(j - 1, j);
                j -= 1;
            }
        }
    } else {
        // Build position permutation, sort by values, apply to both arrays. All scratch
        // (value copy, permutation, index copy) is caller-owned and reused — no per-pixel alloc.
        perm.clear();
        perm.extend(0..n);
        perm.sort_unstable_by(|&a, &b| values[a].total_cmp(&values[b]));

        scratch_buf.clear();
        scratch_buf.extend_from_slice(&values[..n]);
        old_indices.clear();
        old_indices.extend_from_slice(&indices[..n]);
        for (dst, &src) in perm.iter().enumerate() {
            values[dst] = scratch_buf[src];
            indices[dst] = old_indices[src];
        }
    }
}

/// MAD (median absolute deviation from `center`) of an **ascending-sorted** slice, without a
/// scratch buffer or quickselect. The absolute deviations split into two ascending runs — the
/// elements below `center` read backwards, and those at/above `center` read forwards — so a
/// two-pointer merge yields them in global ascending order. Advancing to rank `len/2` reproduces
/// `median_f32_fast` of the deviations exactly (the same upper-middle order statistic).
fn sorted_mad(sorted: &[f32], center: f32) -> f32 {
    let m = sorted.len();
    debug_assert!(m > 0);
    let split = sorted.partition_point(|&v| v < center);
    let mut l = split; // left run consumes sorted[l - 1] going down
    let mut r = split; // right run consumes sorted[r] going up
    let target = m / 2;
    let mut dev = 0.0f32;
    for _ in 0..=target {
        let left = (l > 0).then(|| center - sorted[l - 1]);
        let right = (r < m).then(|| sorted[r] - center);
        dev = match (left, right) {
            (Some(ld), Some(rd)) if ld <= rd => {
                l -= 1;
                ld
            }
            (Some(_), Some(rd)) => {
                r += 1;
                rd
            }
            (Some(ld), None) => {
                l -= 1;
                ld
            }
            (None, Some(rd)) => {
                r += 1;
                rd
            }
            (None, None) => break,
        };
    }
    dev
}

/// Pixel rejection algorithm applied before combining.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rejection {
    /// No rejection.
    None,
    /// Iterative sigma clipping from median (symmetric or asymmetric).
    SigmaClip(SigmaClipConfig),
    /// Replace outliers with boundary values (better for small stacks).
    Winsorized(WinsorizedClipConfig),
    /// Fit linear trend, reject deviants (good for gradients).
    LinearFit(LinearFitClipConfig),
    /// Clip lowest/highest percentiles.
    Percentile(PercentileClipConfig),
    /// Generalized ESD test (best for large stacks >50 frames).
    Gesd(GesdConfig),
}

/// Mean reduction plus the number of source frames retained by rejection.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MeanSample {
    pub(crate) value: f32,
    pub(crate) survivor_count: usize,
}

impl Default for Rejection {
    fn default() -> Self {
        Self::SigmaClip(SigmaClipConfig::new(2.5, 3))
    }
}

impl Rejection {
    /// Create sigma clipping with default iterations.
    pub fn sigma_clip(sigma: f32) -> Self {
        Self::SigmaClip(SigmaClipConfig::new(sigma, 3))
    }

    /// Create asymmetric sigma clipping.
    pub fn sigma_clip_asymmetric(sigma_low: f32, sigma_high: f32) -> Self {
        Self::SigmaClip(SigmaClipConfig::new_asymmetric(sigma_low, sigma_high, 3))
    }

    /// Create winsorized sigma clipping.
    pub fn winsorized(sigma: f32) -> Self {
        Self::Winsorized(WinsorizedClipConfig::new(sigma))
    }

    /// Create linear fit clipping with symmetric thresholds.
    pub fn linear_fit(sigma: f32) -> Self {
        Self::LinearFit(LinearFitClipConfig::new(sigma, sigma, 3))
    }

    /// Create percentile clipping with symmetric bounds.
    pub fn percentile(percent: f32) -> Self {
        Self::Percentile(PercentileClipConfig::new(percent, percent))
    }

    /// Create GESD with default alpha.
    pub fn gesd() -> Self {
        Self::Gesd(GesdConfig::default())
    }

    /// Partition values by rejection algorithm, returning the number of survivors.
    ///
    /// After return, `values[..remaining]` holds the surviving values and `scratch.indices`
    /// their original frame indices (kept paired). `None` does no work and returns `values.len()`.
    pub(crate) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        match self {
            Rejection::None => values.len(),
            Rejection::SigmaClip(c) => c.reject(values, scratch),
            Rejection::Winsorized(c) => c.reject(values, scratch),
            Rejection::LinearFit(c) => c.reject(values, scratch),
            Rejection::Percentile(c) => c.reject(values, scratch),
            Rejection::Gesd(c) => c.reject(values, scratch),
        }
    }

    /// Reject outliers then compute (weighted) mean.
    ///
    /// Uses index tracking to maintain correct value-weight alignment after rejection
    /// functions partition/reorder the values array.
    pub(crate) fn combine_mean(
        &self,
        values: &mut [f32],
        weights: Option<&[f32]>,
        scratch: &mut ScratchBuffers,
    ) -> f32 {
        self.combine_mean_with_survivors(values, weights, scratch)
            .value
    }

    pub(crate) fn combine_mean_with_survivors(
        &self,
        values: &mut [f32],
        weights: Option<&[f32]>,
        scratch: &mut ScratchBuffers,
    ) -> MeanSample {
        // None doesn't reorder values, so weights align directly
        if let Rejection::None = self {
            let value = match weights {
                Some(w) => weighted_mean_f32(values, w),
                None => mean_f32(values),
            };
            return MeanSample {
                value,
                survivor_count: values.len(),
            };
        }

        // Rejection variants that reorder values: use index mapping for weights
        let remaining = self.reject(values, scratch);

        let value = match weights {
            Some(w) if remaining > 0 => weighted_mean_indexed(
                &values[..remaining],
                w,
                &scratch.indices[..remaining],
                &mut scratch.floats_a,
            ),
            _ => mean_f32(&values[..remaining]),
        };
        MeanSample {
            value,
            survivor_count: remaining,
        }
    }

    /// Reject outliers, compute the weighted mean, and describe the weights of its survivors.
    pub(crate) fn combine_mean_with_quality(
        &self,
        values: &mut [f32],
        weights: &[f32],
        scratch: &mut ScratchBuffers,
    ) -> CombinedSample {
        debug_assert_eq!(values.len(), weights.len());
        if let Rejection::None = self {
            return CombinedSample::from_all(weighted_mean_f32(values, weights), weights);
        }

        let remaining = self.reject(values, scratch);
        let survivors = &scratch.indices[..remaining];
        let value = if remaining > 0 {
            weighted_mean_indexed(
                &values[..remaining],
                weights,
                survivors,
                &mut scratch.floats_a,
            )
        } else {
            0.0
        };
        CombinedSample::from_survivors(value, weights, survivors.iter().copied())
    }
}

/// Weighted mean of rejection-reordered `values`: gathers each survivor's weight
/// via `indices[i] → weights[indices[i]]` into `scratch` so values and weights
/// align, then delegates to the precision-preserving [`weighted_mean_f32`],
/// matching the unrejected branch. Returns `0.0` when the total weight is ~0.
///
/// `scratch` is a reused buffer (its prior contents are overwritten) so the
/// per-pixel combine path allocates nothing.
///
/// Preconditions: `indices.len() == values.len()`, all `indices[i] < weights.len()`.
fn weighted_mean_indexed(
    values: &[f32],
    weights: &[f32],
    indices: &[usize],
    scratch: &mut Vec<f32>,
) -> f32 {
    debug_assert_eq!(values.len(), indices.len());

    scratch.clear();
    scratch.extend(indices.iter().map(|&idx| weights[idx]));
    weighted_mean_f32(values, scratch.as_slice())
}

#[cfg(test)]
mod tests {
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    use crate::math::sum::mean_f32;

    use crate::stacking::combine::rejection::*;

    #[test]
    fn test_sigma_clip_config_default() {
        let config = SigmaClipConfig::default();
        assert!((config.sigma_low - 2.5).abs() < f32::EPSILON);
        assert!((config.sigma_high - 2.5).abs() < f32::EPSILON);
        assert_eq!(config.max_iterations, 3);
    }

    #[test]
    fn test_sigma_clip_config_new_symmetric() {
        let config = SigmaClipConfig::new(3.0, 5);
        assert!((config.sigma_low - 3.0).abs() < f32::EPSILON);
        assert!((config.sigma_high - 3.0).abs() < f32::EPSILON);
        assert_eq!(config.max_iterations, 5);
    }

    #[test]
    fn test_sigma_clip_config_new_asymmetric() {
        let config = SigmaClipConfig::new_asymmetric(2.0, 3.0, 5);
        assert!((config.sigma_low - 2.0).abs() < f32::EPSILON);
        assert!((config.sigma_high - 3.0).abs() < f32::EPSILON);
        assert_eq!(config.max_iterations, 5);
    }

    #[test]
    fn test_winsorized_config_default() {
        let config = WinsorizedClipConfig::default();
        assert!((config.sigma_low - 2.5).abs() < f32::EPSILON);
        assert!((config.sigma_high - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_linear_fit_config_default() {
        let config = LinearFitClipConfig::default();
        assert!((config.sigma_low - 3.0).abs() < f32::EPSILON);
        assert!((config.sigma_high - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_percentile_config_default() {
        let config = PercentileClipConfig::default();
        assert!((config.low_percentile - 10.0).abs() < f32::EPSILON);
        assert!((config.high_percentile - 10.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_gesd_config_default() {
        let config = GesdConfig::default();
        assert!((config.alpha - 0.05).abs() < f32::EPSILON);
        assert!(config.max_outliers.is_none());

        let automatic_cases = [
            (0, 0),
            (3, 0),
            (4, 1),
            (8, 2),
            (15, 2),
            (24, 2),
            (25, 6),
            (39, 9),
            (40, 10),
            (44, 10),
            (100, 10),
        ];
        for (sample_count, expected) in automatic_cases {
            assert_eq!(
                config.max_outliers_for_size(sample_count),
                expected,
                "automatic limit for {sample_count} samples"
            );
        }

        for (sample_count, configured) in [(15, 3), (44, 11), (100, 25)] {
            assert_eq!(
                GesdConfig::new(0.05, Some(configured)).max_outliers_for_size(sample_count),
                configured,
                "explicit limit for {sample_count} samples"
            );
        }
    }

    #[test]
    fn test_sigma_clip_removes_outlier() {
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values, &mut scratch());
        let mean = mean_f32(&values[..remaining]);
        assert!(mean < 10.0, "Expected outlier to be clipped, got {}", mean);
        assert!(remaining < 8);
    }

    #[test]
    fn test_sigma_clip_no_outliers() {
        let mut values = vec![1.0, 1.1, 1.2, 0.9, 1.0];
        let remaining = SigmaClipConfig::new(3.0, 3).reject(&mut values, &mut scratch());
        assert_eq!(remaining, 5);
    }

    #[test]
    fn test_asymmetric_sigma_clip_removes_high_outlier() {
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let remaining =
            SigmaClipConfig::new_asymmetric(4.0, 2.0, 3).reject(&mut values, &mut scratch());
        let mean = mean_f32(&values[..remaining]);
        assert!(mean < 10.0, "High outlier should be clipped, got {}", mean);
        assert!(remaining < 8);
    }

    #[test]
    fn test_asymmetric_sigma_clip_keeps_low_with_high_threshold() {
        // Conservative sigma_low (10.0) + aggressive sigma_high (2.0):
        // high outlier rejected, low outlier kept.
        let mut values = vec![-5.0, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 50.0];
        let remaining =
            SigmaClipConfig::new_asymmetric(10.0, 2.0, 5).reject(&mut values, &mut scratch());

        assert!(
            remaining >= 9,
            "Low outlier should be kept, remaining={}",
            remaining
        );
        let mean = mean_f32(&values[..remaining]);
        assert!(
            mean < 2.5,
            "Mean should be < 2.5 due to kept low outlier, got {}",
            mean
        );
    }

    #[test]
    fn test_sigma_clip_symmetric_equals_asymmetric_same_thresholds() {
        let values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let sigma = 2.5;

        let mut v1 = values.clone();
        let r1 = SigmaClipConfig::new(sigma, 3).reject(&mut v1, &mut scratch());

        let mut v2 = values;
        let r2 = SigmaClipConfig::new_asymmetric(sigma, sigma, 3).reject(&mut v2, &mut scratch());

        assert_eq!(r1, r2);
        assert!((mean_f32(&v1[..r1]) - mean_f32(&v2[..r2])).abs() < 1e-6,);
    }

    #[test]
    fn test_sorted_mad_matches_mad_f32_fast() {
        // `sorted_mad` must reproduce `mad_f32_fast` (the function the sort-once reject replaced)
        // exactly — same upper-middle order statistic of the absolute deviations. Cover odd/even
        // lengths, center inside/outside the data, duplicates, and a heavy outlier.
        let cases: &[&[f32]] = &[
            &[1.0, 2.0, 3.0, 4.0, 100.0],         // odd, outlier
            &[1.0, 2.0, 3.0, 4.0],                // even
            &[-5.0, 0.0, 0.0, 0.0, 1.0, 2.0],     // duplicates at center
            &[10.0, 10.0, 10.0],                  // constant → MAD 0
            &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7], // odd, spread
        ];
        let mut buf = vec![];
        for sorted in cases {
            // Reject always calls it at the median; also probe a couple of off-median centers.
            let mid = sorted[sorted.len() / 2];
            for &center in &[mid, sorted[0], mid + 0.05] {
                let expected = mad_f32_fast(sorted, center, &mut buf);
                let got = sorted_mad(sorted, center);
                assert_eq!(
                    got, expected,
                    "sorted_mad({sorted:?}, {center}) = {got}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn test_sigma_clip_survivor_indices_pair_with_values() {
        // After rejection, `indices[..remaining]` must be the original frame indices of the
        // surviving values, i.e. `values[i] == original[indices[i]]`. Regression for the prior
        // quickselect that reordered values without their co-indices, mis-pairing per-frame
        // weights in the noise-weighted (light) combine.
        let original = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let mut values = original.clone();
        let mut sc = scratch();
        let remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values, &mut sc);

        assert!(
            remaining < original.len(),
            "the 100.0 outlier should be rejected"
        );
        for (i, (&val, &idx)) in values[..remaining]
            .iter()
            .zip(&sc.indices[..remaining])
            .enumerate()
        {
            assert_eq!(
                val, original[idx],
                "survivor {i}: value {val} must equal original[{idx}] = {}",
                original[idx]
            );
        }
        // Frame 7 (value 100.0) is the outlier — its index must not survive.
        assert!(
            !sc.indices[..remaining].contains(&7),
            "rejected outlier's index leaked into the survivors"
        );
    }

    #[test]
    fn test_linear_fit_first_pass_uses_median_mad() {
        // Linear fit's first pass uses median + MAD (same as sigma clip).
        // With max_iterations=1, linear fit behaves identically to a single
        // sigma clip pass.
        let mut values_lf = vec![1.0, 2.0, 3.0, 4.0, 100.0, 6.0];
        let mut values_sc = values_lf.clone();

        let lf_remaining =
            LinearFitClipConfig::new(2.0, 2.0, 1).reject(&mut values_lf, &mut scratch());
        let sc_remaining = SigmaClipConfig::new(2.0, 1).reject(&mut values_sc, &mut scratch());

        // Both should reject the same outlier on the first pass
        assert_eq!(lf_remaining, sc_remaining);
    }

    #[test]
    fn test_winsorized_rejects_outlier() {
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let remaining = WinsorizedClipConfig::new(2.0).reject(&mut values, &mut scratch());
        assert!(
            remaining < 8,
            "Outlier should be rejected, got {remaining} survivors"
        );
        let mean = mean_f32(&values[..remaining]);
        assert!(mean < 10.0, "Mean of survivors should be low, got {mean}");
    }

    #[test]
    fn test_linear_fit_constant_data() {
        let mut values = vec![5.0, 5.0, 5.0, 5.0, 5.0];
        let remaining = LinearFitClipConfig::default().reject(&mut values, &mut scratch());
        assert_eq!(remaining, 5);
        assert!((mean_f32(&values[..remaining]) - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_linear_fit_rejects_outlier() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 100.0, 6.0];
        let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut scratch());
        assert!(remaining < 6);
        assert!(mean_f32(&values[..remaining]) < 20.0);
    }

    #[test]
    fn test_linear_fit_rejects_off_line_point_when_seed_pass_is_clean() {
        // The fit must run even when the median+MAD seed pass rejects nothing — otherwise an
        // off-line point hidden by a steep spread survives. Ramp 10..90 + an off-line `5`:
        // seed median≈45, MAD≈25 → sigma≈37, threshold(2.0)≈74, so the `5` (|Δ|=40) is kept and
        // the seed rejects nothing. The line fit through the sorted values has σ≈0.92 (mean |resid|),
        // threshold(2.0)≈1.84; only `5` (residual≈3.27 from the fitted line) exceeds it.
        let mut values = vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 5.0];
        let mut s = scratch();
        let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut s);

        assert_eq!(remaining, 9, "only the off-line `5` should be rejected");
        assert!(
            !s.indices[..remaining].contains(&9),
            "frame 9 (value 5.0) must be rejected, survivors: {:?}",
            &s.indices[..remaining]
        );
        // Survivors are the clean ramp 10..90 → mean 50.
        let mean = mean_f32(&values[..remaining]);
        assert!((mean - 50.0).abs() < 1e-3, "expected mean 50, got {mean}");
    }

    #[test]
    fn test_sigma_clip_rejects_outlier_in_bright_high_magnitude_data() {
        // Guards the early-exit's numerical soundness (f64 accumulation): on high-magnitude pixels
        // (~8000) with a real outlier, `no_outliers_possible` must not spuriously fire and skip
        // rejection. 14 clean values symmetric about 8000 (mean exactly 8000) + one 9000 outlier.
        let mut values = vec![
            7990.0, 8000.0, 8010.0, 7995.0, 8005.0, 8000.0, 7990.0, 8010.0, 8000.0, 7995.0, 8005.0,
            8000.0, 7990.0, 8010.0, 9000.0,
        ];
        let mut s = scratch();
        let remaining = SigmaClipConfig::new(2.5, 3).reject(&mut values, &mut s);

        assert_eq!(remaining, 14, "the 9000 outlier must be rejected");
        assert!(
            !s.indices[..remaining].contains(&14),
            "frame 14 (value 9000) must be rejected, survivors: {:?}",
            &s.indices[..remaining]
        );
        let mean = mean_f32(&values[..remaining]);
        assert!(
            (mean - 8000.0).abs() < 0.5,
            "expected mean 8000, got {mean}"
        );
    }

    #[test]
    fn test_percentile_clip() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let remaining = PercentileClipConfig::new(20.0, 20.0).reject(&mut values, &mut scratch());
        assert_eq!(remaining, 6);
        // Mean of [3, 4, 5, 6, 7, 8] = 5.5
        assert!((mean_f32(&values[..remaining]) - 5.5).abs() < 0.01);
    }

    #[test]
    fn test_gesd_removes_single_bright_outlier() {
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 100.0];
        let mut s = scratch();
        let remaining = GesdConfig::new(0.05, None).reject(&mut values, &mut s);
        assert_eq!(
            remaining, 7,
            "Exactly the bright outlier should be rejected"
        );
        assert!(
            !s.indices[..remaining].contains(&7),
            "Index 7 (100.0) must be rejected, survivors: {:?}",
            &s.indices[..remaining]
        );
    }

    #[test]
    fn test_gesd_no_outliers() {
        // Constant values — sigma=0 so no outliers detected
        let mut values = vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let remaining = GesdConfig::new(0.05, Some(3)).reject(&mut values, &mut scratch());
        assert_eq!(remaining, 8, "No outliers in constant data");
    }

    #[test]
    fn test_gesd_tiny_alpha_uses_finite_limiting_critical_value() {
        let mut values: Vec<f32> = (0..15).map(|value| value as f32).collect();
        let mut scratch = scratch();

        let remaining =
            GesdConfig::new(f32::MIN_POSITIVE, Some(3)).reject(&mut values, &mut scratch);

        assert_eq!(remaining, 15);
        let expected = 14.0 / 15.0f64.sqrt();
        assert!((scratch.gesd_critical_values[0] - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn test_gesd_keeps_tight_cluster() {
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 1.1];
        let remaining = GesdConfig::default().reject(&mut values, &mut scratch());
        assert_eq!(remaining, 8, "Tight cluster should have no rejections");
    }

    #[test]
    fn test_small_sample_handling() {
        // All algorithms should handle n=2 gracefully
        let r = SigmaClipConfig::default().reject(&mut [1.0, 2.0], &mut scratch());
        assert_eq!(r, 2);

        let r =
            SigmaClipConfig::new_asymmetric(4.0, 3.0, 3).reject(&mut [1.0, 2.0], &mut scratch());
        assert_eq!(r, 2);

        let r = WinsorizedClipConfig::default().reject(&mut [1.0, 2.0], &mut scratch());
        assert_eq!(r, 2);

        let r = LinearFitClipConfig::default().reject(&mut [1.0, 2.0], &mut scratch());
        assert_eq!(r, 2);

        let r = PercentileClipConfig::default().reject(&mut [1.0, 2.0], &mut scratch());
        assert!(r >= 1);

        let r = GesdConfig::default().reject(&mut [1.0, 2.0], &mut scratch());
        assert_eq!(r, 2);
    }

    #[test]
    fn test_sigma_clip_indices_track_survivors() {
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let mut s = scratch();
        let remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values, &mut s);

        let surviving = &s.indices[..remaining];
        assert!(
            !surviving.contains(&7),
            "Frame 7 (outlier) should not survive, survivors: {:?}",
            surviving
        );
        for &idx in surviving {
            assert!(idx < 8, "Invalid surviving index: {}", idx);
        }
    }

    #[test]
    fn test_linear_fit_indices_track_survivors() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 100.0, 6.0];
        let mut s = scratch();
        let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut s);

        let surviving = &s.indices[..remaining];
        assert!(
            !surviving.contains(&4),
            "Frame 4 (outlier) should not survive, survivors: {:?}",
            surviving
        );
        for &idx in surviving {
            assert!(idx < 6, "Invalid surviving index: {}", idx);
        }
    }

    #[test]
    fn test_percentile_indices_track_survivors() {
        // Values: [5, 1, 3, 2, 4] → sorted: [1, 2, 3, 4, 5]
        // With 20% clip on each end: clips 1 low, 1 high → survivors [2, 3, 4]
        let mut values = vec![5.0, 1.0, 3.0, 2.0, 4.0];
        let mut s = scratch();
        let remaining = PercentileClipConfig::new(20.0, 20.0).reject(&mut values, &mut s);

        assert_eq!(remaining, 3);
        let surviving = &s.indices[..remaining];
        // Original indices: 5.0→0, 1.0→1, 3.0→2, 2.0→3, 4.0→4
        // Survivors (values 2,3,4) should map to original indices 3, 2, 4
        assert!(
            !surviving.contains(&0) && !surviving.contains(&1),
            "Frames 0 (5.0) and 1 (1.0) should be clipped, survivors: {:?}",
            surviving
        );
        for &idx in surviving {
            assert!(idx < 5, "Invalid surviving index: {}", idx);
        }
    }

    #[test]
    fn test_no_rejection_preserves_all_indices() {
        let mut values = vec![1.0, 1.1, 1.2, 0.9, 1.0];
        let mut s = scratch();
        let remaining = SigmaClipConfig::new(3.0, 3).reject(&mut values, &mut s);

        assert_eq!(remaining, 5);
        let surviving = &s.indices[..remaining];
        for i in 0..5 {
            assert!(
                surviving.contains(&i),
                "Index {} should survive when no rejection occurs",
                i
            );
        }
    }

    #[test]
    fn test_rejection_constructors() {
        let r = Rejection::sigma_clip(2.0);
        assert!(
            matches!(r, Rejection::SigmaClip(c) if (c.sigma_low - 2.0).abs() < f32::EPSILON && (c.sigma_high - 2.0).abs() < f32::EPSILON)
        );

        let r = Rejection::winsorized(3.0);
        assert!(
            matches!(r, Rejection::Winsorized(c) if (c.sigma_low - 3.0).abs() < f32::EPSILON && (c.sigma_high - 3.0).abs() < f32::EPSILON)
        );

        let r = Rejection::linear_fit(2.5);
        assert!(matches!(r, Rejection::LinearFit(c)
            if (c.sigma_low - 2.5).abs() < f32::EPSILON && (c.sigma_high - 2.5).abs() < f32::EPSILON));

        let r = Rejection::percentile(15.0);
        assert!(matches!(r, Rejection::Percentile(c)
            if (c.low_percentile - 15.0).abs() < f32::EPSILON && (c.high_percentile - 15.0).abs() < f32::EPSILON));

        let r = Rejection::gesd();
        assert!(matches!(r, Rejection::Gesd(c)
            if (c.alpha - 0.05).abs() < f32::EPSILON && c.max_outliers.is_none()));
    }

    fn scratch() -> ScratchBuffers {
        ScratchBuffers::default()
    }

    #[test]
    fn test_combine_mean_none() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let mean = Rejection::None.combine_mean(&mut values, None, &mut scratch());
        assert!((mean - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_combine_mean_sigma_clip() {
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let mean = Rejection::sigma_clip(2.0).combine_mean(&mut values, None, &mut scratch());
        assert!(mean < 10.0, "Outlier should be clipped, got {}", mean);
    }

    #[test]
    fn test_weighted_percentile_uses_weights() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let weights = vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 10.0, 1.0, 1.0];

        let mean =
            Rejection::percentile(20.0).combine_mean(&mut values, Some(&weights), &mut scratch());

        assert!(
            mean > 5.5 + 0.5,
            "Weighted percentile should be pulled toward heavily weighted value 8, got {}",
            mean
        );
    }

    #[test]
    fn test_weighted_winsorized_uses_weights() {
        let mut values = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0];
        let weights = vec![10.0, 1.0, 1.0, 1.0, 1.0, 1.0];

        let mean =
            Rejection::winsorized(2.0).combine_mean(&mut values, Some(&weights), &mut scratch());

        let mut values_unwt = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0];
        let uniform_weights = vec![1.0; 6];
        let unweighted_mean = Rejection::winsorized(2.0).combine_mean(
            &mut values_unwt,
            Some(&uniform_weights),
            &mut scratch(),
        );

        assert!(
            mean < unweighted_mean,
            "Weighted winsorized (heavy on 1.0) should be less than uniform: {} vs {}",
            mean,
            unweighted_mean,
        );
    }

    #[test]
    fn test_weighted_asymmetric_sigma_clip() {
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let weights = vec![10.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];

        let mean = Rejection::sigma_clip_asymmetric(4.0, 2.0).combine_mean(
            &mut values,
            Some(&weights),
            &mut scratch(),
        );

        assert!(mean < 2.5, "Should be pulled toward 1.0, got {}", mean);
    }

    #[test]
    fn test_weighted_sigma_clip_weight_alignment() {
        let mut values = vec![2.0, 100.0, 3.0, 2.5, 2.2, 1.8, 2.8, 2.3];
        let weights = vec![10.0, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1];

        let mean =
            Rejection::sigma_clip(2.0).combine_mean(&mut values, Some(&weights), &mut scratch());

        assert!(
            (mean - 2.0).abs() < 0.25,
            "Weighted mean should be ~2.0 (dominated by frame 0, weight=10.0), got {}",
            mean
        );
    }

    #[test]
    fn test_weighted_linear_fit_weight_alignment() {
        // Tight cluster [1.0, 1.1, 1.2, 1.3, 1.4] with outlier 100.0
        // After rejection removes 100, weighted mean dominated by frame 0 (weight=10)
        let mut values = vec![1.0, 1.1, 1.2, 1.3, 100.0, 1.4];
        let weights = vec![10.0, 0.1, 0.1, 0.1, 0.1, 0.1];

        let mean =
            Rejection::linear_fit(3.0).combine_mean(&mut values, Some(&weights), &mut scratch());

        assert!(
            mean < 1.1,
            "Weighted mean should be pulled toward frame 0 (value=1.0, weight=10.0), got {}",
            mean
        );
    }

    #[test]
    fn test_weighted_gesd_weight_alignment() {
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 100.0];
        let weights = vec![10.0, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1];

        let mean = Rejection::Gesd(GesdConfig::new(0.05, Some(3))).combine_mean(
            &mut values,
            Some(&weights),
            &mut scratch(),
        );

        assert!(
            (mean - 1.0).abs() < 0.05,
            "Weighted mean should be ~1.0 (dominated by frame 0, weight=10.0), got {}",
            mean
        );
    }

    #[test]
    fn test_gesd_matches_nist_reference_example() {
        let mut values = vec![
            -0.25, 0.68, 0.94, 1.15, 1.20, 1.26, 1.26, 1.34, 1.38, 1.43, 1.49, 1.49, 1.55, 1.56,
            1.58, 1.65, 1.69, 1.70, 1.76, 1.77, 1.81, 1.91, 1.94, 1.96, 1.99, 2.06, 2.09, 2.10,
            2.14, 2.15, 2.23, 2.24, 2.26, 2.35, 2.37, 2.40, 2.47, 2.54, 2.62, 2.64, 2.90, 2.92,
            2.92, 2.93, 3.21, 3.26, 3.30, 3.59, 3.68, 4.30, 4.64, 5.34, 5.42, 6.01,
        ];
        let expected = [
            (3.118, 3.158),
            (2.942, 3.151),
            (3.179, 3.143),
            (2.810, 3.136),
            (2.815, 3.128),
            (2.848, 3.120),
            (2.279, 3.111),
            (2.310, 3.103),
            (2.101, 3.094),
            (2.067, 3.085),
        ];
        let mut scratch = scratch();

        let remaining = GesdConfig::new(0.05, Some(10)).reject(&mut values, &mut scratch);

        assert_eq!(remaining, 51);
        assert_eq!(
            scratch.indices[..remaining]
                .iter()
                .filter(|&&index| index >= 51)
                .count(),
            0
        );
        for ((statistic, critical), (expected_statistic, expected_critical)) in scratch
            .gesd_statistics
            .iter()
            .zip(&scratch.gesd_critical_values)
            .zip(expected)
        {
            assert!(
                (statistic - expected_statistic).abs() <= 0.0015,
                "expected statistic {expected_statistic}, got {statistic}"
            );
            assert!(
                (critical - expected_critical).abs() <= 0.0015,
                "expected critical value {expected_critical}, got {critical}"
            );
        }
    }

    #[test]
    fn test_gesd_is_sign_symmetric_for_asymmetric_outliers() {
        let values = vec![
            -1.4, -1.2, -1.0, -0.8, -0.6, -0.4, -0.2, 0.0, 0.2, 0.4, 0.6, 0.8, 1.0, 1.2, 1.4, -8.0,
            10.0,
        ];
        let mut original = values.clone();
        let mut mirrored: Vec<f32> = values.iter().map(|value| -value).collect();
        let config = GesdConfig::new(0.05, Some(2));
        let mut original_scratch = scratch();
        let mut mirrored_scratch = scratch();

        let original_remaining = config.reject(&mut original, &mut original_scratch);
        let mirrored_remaining = config.reject(&mut mirrored, &mut mirrored_scratch);

        assert_eq!(original_remaining, 15);
        assert_eq!(mirrored_remaining, 15);
        let mut original_survivors = original_scratch.indices[..original_remaining].to_vec();
        let mut mirrored_survivors = mirrored_scratch.indices[..mirrored_remaining].to_vec();
        original_survivors.sort_unstable();
        mirrored_survivors.sort_unstable();
        assert_eq!(original_survivors, mirrored_survivors);
        assert_eq!(original_survivors, (0..15).collect::<Vec<_>>());
    }

    #[test]
    fn test_gesd_gaussian_false_positive_rate_matches_alpha() {
        const ALPHA: f32 = 0.05;
        const TRIALS: usize = 4_000;

        let mut rng = ChaCha8Rng::seed_from_u64(0x947e_4d3a_7c16_b205);
        for sample_count in [15, 25, 50, 100] {
            let config = GesdConfig::new(ALPHA, Some(sample_count / 4));
            let mut scratch = scratch();
            let mut false_positives = 0usize;

            for _ in 0..TRIALS {
                let mut values: Vec<f32> = (0..sample_count)
                    .map(|_| standard_normal(&mut rng))
                    .collect();
                if config.reject(&mut values, &mut scratch) < sample_count {
                    false_positives += 1;
                }
            }

            let actual = false_positives as f64 / TRIALS as f64;
            let expected = f64::from(ALPHA);
            let standard_error = (expected * (1.0 - expected) / TRIALS as f64).sqrt();
            assert!(
                (actual - expected).abs() <= 5.0 * standard_error,
                "n={sample_count}: expected false-positive rate {expected}, got {actual}"
            );
        }
    }

    fn standard_normal(rng: &mut ChaCha8Rng) -> f32 {
        let u1 = rng.random::<f64>().max(f64::MIN_POSITIVE);
        let u2 = rng.random::<f64>();
        ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32
    }

    #[test]
    fn test_rejection_default_is_sigma_clip() {
        let r = Rejection::default();
        assert!(
            matches!(r, Rejection::SigmaClip(c) if (c.sigma_low - 2.5).abs() < f32::EPSILON
                && (c.sigma_high - 2.5).abs() < f32::EPSILON
                && c.max_iterations == 3)
        );
    }

    #[test]
    fn test_sigma_clip_multiple_outliers() {
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 50.0, 80.0, 100.0];
        let remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values, &mut scratch());
        // All three outliers should be removed
        for &v in &values[..remaining] {
            assert!(v < 10.0, "Outlier {} should have been clipped", v);
        }
        assert!(
            remaining <= 5,
            "Expected at most 5 survivors, got {}",
            remaining
        );
    }

    #[test]
    fn test_winsorized_no_outliers() {
        let mut values = vec![2.0, 2.1, 2.2, 1.9, 2.0];
        let remaining = WinsorizedClipConfig::new(3.0).reject(&mut values, &mut scratch());
        assert_eq!(remaining, 5, "No values should be rejected");
    }

    #[test]
    fn test_winsorized_asymmetric() {
        // Strong high outlier, mild low variation
        let mut values = vec![1.0, 1.1, 1.2, 0.9, 1.0, 50.0];
        let remaining =
            WinsorizedClipConfig::new_asymmetric(3.0, 2.0).reject(&mut values, &mut scratch());
        assert!(remaining < 6, "High outlier should be rejected");
        let mean = mean_f32(&values[..remaining]);
        assert!(mean < 5.0, "Mean without outlier should be low, got {mean}");
    }

    #[test]
    fn test_linear_fit_rejects_extreme_outlier() {
        // Linear fit uses fit-derived sigma which is tighter than median+MAD.
        // Initial pass (median+MAD) removes the gross outlier, then the fit
        // refines sigma. With max_iterations=1, only the initial pass runs.
        let mut values = vec![10.0, 10.5, 11.0, 10.2, 10.8, 10.3, 10.7, 50.0];
        let mut s = scratch();
        let remaining = LinearFitClipConfig::new(3.0, 3.0, 1).reject(&mut values, &mut s);
        assert_eq!(remaining, 7, "Only the outlier should be rejected");
        let surviving = &s.indices[..remaining];
        assert!(
            !surviving.contains(&7),
            "Frame 7 (outlier 50.0) should not survive, survivors: {:?}",
            surviving
        );
    }

    #[test]
    fn test_linear_fit_tighter_than_sigma_clip() {
        // Linear fit derives sigma from residuals of a linear fit through sorted
        // values. For well-distributed data, this sigma is tighter than median+MAD,
        // so linear fit rejects more aggressively on subsequent iterations.
        let mut values_lf = vec![1.0, 3.0, 5.0, 7.0, 50.0, 11.0, 13.0, 15.0];
        let mut values_sc = values_lf.clone();

        let lf_remaining =
            LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values_lf, &mut scratch());
        let sc_remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values_sc, &mut scratch());

        // Linear fit should reject more aggressively than sigma clip
        assert!(
            lf_remaining <= sc_remaining,
            "Linear fit (remaining={}) should be at least as aggressive as sigma clip (remaining={})",
            lf_remaining,
            sc_remaining
        );
    }

    #[test]
    fn test_surviving_range_single_element() {
        let config = PercentileClipConfig::new(10.0, 10.0);
        let range = config.surviving_range(1);
        assert_eq!(range, 0..1, "Single element must survive");
    }

    #[test]
    fn test_surviving_range_extreme_percentiles() {
        // 49% + 49% = 98% clipped — should still keep at least 1
        let config = PercentileClipConfig::new(49.0, 49.0);
        let range = config.surviving_range(5);
        assert!(!range.is_empty(), "Must keep at least one element");
        // For n=5: low_count = floor(0.49*5) = 2, high_count = floor(0.49*5) = 2
        // start=2, end=5-2=3, range = 2..3 (1 element)
        assert_eq!(range.len(), 1);
    }

    #[test]
    fn test_weighted_mean_indexed_basic() {
        // values [2, 4, 6] with weights [10, 1, 1] via identity indices
        // expected: (20 + 4 + 6) / 12 = 2.5
        let values = [2.0, 4.0, 6.0];
        let weights = [10.0, 1.0, 1.0];
        let indices = [0, 1, 2];
        let mut buf = Vec::new();
        let mean = weighted_mean_indexed(&values, &weights, &indices, &mut buf);
        assert!((mean - 2.5).abs() < 1e-6, "Expected 2.5, got {}", mean);
    }

    #[test]
    fn test_weighted_mean_indexed_reordered() {
        // Simulate rejection reordering: values were [10, 99, 20] → after rejecting idx 1,
        // survivors are values [10, 20] with indices [0, 2]
        let values = [10.0, 20.0];
        let weights = [5.0, 0.5, 1.0]; // original weights for 3 frames
        let indices = [0, 2]; // frame 0 and frame 2 survived
        let mut buf = Vec::new();
        let mean = weighted_mean_indexed(&values, &weights, &indices, &mut buf);
        // expected: (10*5 + 20*1) / (5+1) = 70/6 ≈ 11.667
        assert!(
            (mean - 70.0 / 6.0).abs() < 1e-5,
            "Expected {}, got {}",
            70.0 / 6.0,
            mean
        );
    }

    #[test]
    fn test_combine_mean_percentile_unweighted() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let mean = Rejection::percentile(20.0).combine_mean(&mut values, None, &mut scratch());
        // Clips 2 low (1,2) and 2 high (9,10), mean of [3,4,5,6,7,8] = 5.5
        assert!(
            (mean - 5.5).abs() < 0.01,
            "Unweighted percentile mean should be 5.5, got {}",
            mean
        );
    }

    #[test]
    fn test_combine_mean_none_with_weights() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let weights = vec![10.0, 1.0, 1.0, 1.0, 1.0];
        let mean = Rejection::None.combine_mean(&mut values, Some(&weights), &mut scratch());
        // Weighted mean: (10+2+3+4+5) / (10+1+1+1+1) = 24/14 ≈ 1.714
        assert!(
            (mean - 24.0 / 14.0).abs() < 1e-5,
            "Weighted mean with no rejection should be {}, got {}",
            24.0 / 14.0,
            mean
        );
    }

    #[test]
    fn test_weighted_rejection_uniform_weights_unchanged() {
        let values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let uniform_weights = vec![1.0; 8];

        let weighted = Rejection::sigma_clip(2.0).combine_mean(
            &mut values.clone(),
            Some(&uniform_weights),
            &mut scratch(),
        );

        let mut values2 = values;
        let unweighted =
            Rejection::sigma_clip(2.0).combine_mean(&mut values2, None, &mut scratch());

        assert!(
            (weighted - unweighted).abs() < 1e-5,
            "Uniform weighted should match non-weighted: {} vs {}",
            weighted,
            unweighted
        );
    }

    #[test]
    fn test_winsorized_robust_estimate_uses_stddev_not_mad() {
        // With known data, verify robust_estimate returns stddev-based sigma
        // (not MAD-based). For Gaussian data, stddev > MAD * 1.4826 is false,
        // but for uniform-like data they differ noticeably.
        let config = WinsorizedClipConfig::new(3.0);
        let values: Vec<f32> = (0..20).map(|i| 10.0 + i as f32 * 0.1).collect();
        let mut working = vec![];
        let (center, sigma) = config.robust_estimate(&values, &mut working);

        // Center should be near median (10.95)
        assert!(
            (center - 10.95).abs() < 0.2,
            "Center should be near median, got {center}"
        );
        // Sigma should be positive and reasonable (1.134 * stddev of uniform-ish data)
        assert!(sigma > 0.0, "Sigma should be positive, got {sigma}");
        assert!(
            sigma < 2.0,
            "Sigma should be reasonable for tight data, got {sigma}"
        );
    }

    #[test]
    fn test_winsorized_correction_factor_applied() {
        // Verify 1.134 correction is applied by comparing with raw stddev
        let config = WinsorizedClipConfig::new(3.0);
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let mut working = vec![];
        let (_center, sigma) = config.robust_estimate(&values, &mut working);

        // Raw stddev of 1..=10 is ~3.03. With no outliers to Winsorize,
        // sigma should be approximately 3.03 * 1.134 ≈ 3.43
        let raw_stddev = winsorized_stddev(&values, 5.5);
        let expected = raw_stddev * 1.134;
        assert!(
            (sigma - expected).abs() < 0.5,
            "Sigma {sigma} should be near {expected} (raw_stddev {raw_stddev} * 1.134)"
        );
    }

    #[test]
    fn test_winsorized_converges() {
        // With a clear outlier, verify convergence produces stable estimates
        let config = WinsorizedClipConfig::new(2.5);
        let values = vec![10.0, 10.1, 10.2, 9.9, 10.0, 10.1, 9.8, 10.3, 50.0];
        let mut working = vec![];
        let (center, sigma) = config.robust_estimate(&values, &mut working);

        // Center should be near the cluster (~10.05), not pulled toward 50
        assert!(
            (center - 10.05).abs() < 0.5,
            "Center should be near 10.05, got {center}"
        );
        // Sigma should reflect the cluster spread, not the outlier
        assert!(
            sigma < 2.0,
            "Sigma should be small (cluster spread), got {sigma}"
        );
    }

    #[test]
    fn test_winsorized_huber_constant_not_user_sigma() {
        // Verify that Winsorization boundaries use c=1.5, not user's sigma.
        // With sigma=10.0 (very permissive), phase 1 should still use c=1.5
        // for Winsorization, producing the same robust estimates.
        let config_permissive = WinsorizedClipConfig::new(10.0);
        let config_tight = WinsorizedClipConfig::new(2.0);

        // Use a mild outlier (~5σ from center) so tight rejects but permissive keeps.
        // Clean cluster ~1.0 (stddev ~0.15, corrected ~0.17), outlier at 2.0 is ~5.6σ.
        let values = vec![1.0, 1.1, 1.2, 0.9, 1.0, 1.1, 0.8, 1.3, 2.0];
        let mut w1 = vec![];
        let mut w2 = vec![];
        let (center1, sigma1) = config_permissive.robust_estimate(&values, &mut w1);
        let (center2, sigma2) = config_tight.robust_estimate(&values, &mut w2);

        // Both should produce the same robust estimates (same Huber c=1.5)
        assert!(
            (center1 - center2).abs() < 1e-4,
            "Centers should match: {center1} vs {center2}"
        );
        assert!(
            (sigma1 - sigma2).abs() < 1e-4,
            "Sigmas should match: {sigma1} vs {sigma2}"
        );

        // But rejection results should differ (permissive keeps outlier)
        let mut v1 = values.clone();
        let mut v2 = values.clone();
        let r1 = config_permissive.reject(&mut v1, &mut scratch());
        let r2 = config_tight.reject(&mut v2, &mut scratch());
        assert!(
            r1 > r2,
            "Permissive sigma should keep more values: {r1} vs {r2}"
        );
    }

    #[test]
    fn test_linear_fit_per_pixel_rejection() {
        // Construct data with a clear linear trend plus one outlier.
        // Sorted values: [1, 2, 3, 4, 5, 6, 7, 100]
        // The fit through sorted positions should approximate y = 1 + x.
        // Value 100 at position 7 has fitted value ~8, residual ~92.
        // With per-pixel rejection, this should be caught easily.
        // With single-center rejection (old bug), center ≈ fit(3.5) ≈ 4.5,
        // values 1 and 7 would be far from center too.
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 100.0];
        let mut s = scratch();
        let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut s);

        // The outlier (100.0) should be rejected
        assert!(
            remaining == 7,
            "Should reject only the outlier, got {remaining} survivors"
        );
        // All survivors should be in range [1, 7]
        for &v in &values[..remaining] {
            assert!((1.0..=7.0).contains(&v), "Unexpected survivor: {v}");
        }
    }

    #[test]
    fn test_linear_fit_sigma_is_mean_abs_dev() {
        // For a perfect linear sequence, mean absolute deviation from fit should be ~0.
        // Adding a known deviation: values = [1, 2, 3, 4, 5] + noise on last.
        // After initial median+MAD pass (no rejection for clean data),
        // the fit pass should compute sigma from mean abs dev.
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut scratch());
        // Perfect line: no rejections
        assert_eq!(remaining, 5, "Perfect line should have no rejections");
    }

    #[test]
    fn test_linear_fit_preserves_trend() {
        // Linear fit should NOT reject values that follow a trend, even if
        // they're far from the median. This was the old bug: single-center
        // rejection would reject endpoints of a steep trend.
        let mut values = vec![1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0];
        let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut scratch());
        assert_eq!(
            remaining, 8,
            "All values follow a linear trend — none should be rejected"
        );
    }

    #[test]
    fn test_linear_fit_rejects_middle_outlier() {
        // An outlier in the middle of the distribution should be caught by
        // per-pixel rejection. After sorting: [1, 2, 3, 4, 5, 50, 6, 7]
        // → sorted: [1, 2, 3, 4, 5, 6, 7, 50]. Fit: ~y = 0.71 + 0.71*x.
        // Fitted value at position 7 ≈ 5.7, residual of 50 ≈ 44.3.
        let mut values = vec![1.0, 2.0, 3.0, 50.0, 5.0, 6.0, 7.0, 4.0];
        let mut s = scratch();
        let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut s);
        assert!(remaining < 8, "Outlier 50 should be rejected");
        for &v in &values[..remaining] {
            assert!(v < 10.0, "Outlier should not survive, got {v}");
        }
    }

    #[test]
    fn test_sort_with_indices_large_n_correctness() {
        // 100 elements in reverse order → exercises the introsort path (threshold=64).
        // After sorting: values should be [0, 1, 2, ..., 99] and indices should
        // map each sorted position back to its original position.
        // Original: values[0]=99, values[1]=98, ..., values[99]=0
        // Sorted:   values[0]=0, values[1]=1, ..., values[99]=99
        // Indices:  indices[0]=99, indices[1]=98, ..., indices[99]=0
        let n = 100;
        let mut values: Vec<f32> = (0..n).rev().map(|i| i as f32).collect();
        let mut indices: Vec<usize> = (0..n).collect();
        let mut scratch_buf = Vec::new();
        let mut perm = Vec::new();
        let mut old_indices = Vec::new();

        sort_with_indices(
            &mut values,
            &mut indices,
            &mut scratch_buf,
            &mut perm,
            &mut old_indices,
            n,
        );

        for i in 0..n {
            assert_eq!(values[i], i as f32, "values[{i}] wrong");
            // Original position of value i was (n-1-i)
            assert_eq!(indices[i], n - 1 - i, "indices[{i}] wrong");
        }
    }

    #[test]
    fn test_sort_with_indices_large_n_shuffled() {
        // Deterministic shuffle: positions generated by (i*37) % 200.
        // Verifies sort + index tracking for a non-trivial permutation.
        let n = 200;
        let mut values = vec![0.0f32; n];
        let mut indices: Vec<usize> = (0..n).collect();
        // Place value (i*37 % 200) at position i
        for (i, v) in values.iter_mut().enumerate() {
            *v = ((i * 37) % n) as f32;
        }
        let original_values = values.clone();
        let mut scratch_buf = Vec::new();
        let mut perm = Vec::new();
        let mut old_indices = Vec::new();

        sort_with_indices(
            &mut values,
            &mut indices,
            &mut scratch_buf,
            &mut perm,
            &mut old_indices,
            n,
        );

        // Values must be sorted
        for i in 1..n {
            assert!(
                values[i - 1] <= values[i],
                "Not sorted at {}: {} > {}",
                i,
                values[i - 1],
                values[i]
            );
        }
        // Each index must point back to where this value came from
        for (i, (&v, &idx)) in values.iter().zip(indices.iter()).enumerate() {
            assert_eq!(
                original_values[idx], v,
                "Index tracking broken at position {i}: indices[{i}]={idx}, original[{idx}]={}, but values[{i}]={v}",
                original_values[idx]
            );
        }
    }

    #[test]
    fn test_percentile_large_stack() {
        // 100 frames (exercises introsort path). Values 0..99.
        // 10% clip each end → remove bottom 10 and top 10 → survivors 10..90 (80 values).
        // Indices should map survivors to their original positions.
        let n = 100;
        // Reverse order to test sorting
        let mut values: Vec<f32> = (0..n).rev().map(|i| i as f32).collect();
        let mut s = scratch();
        s.indices.resize(n, 0);

        let remaining = PercentileClipConfig::new(10.0, 10.0).reject(&mut values, &mut s);

        assert_eq!(remaining, 80);
        // Surviving values should be 10..90 (sorted)
        for (i, &v) in values[..80].iter().enumerate() {
            assert_eq!(v, (i + 10) as f32, "Surviving value at position {i}");
        }
        // Surviving indices should map back: value 10 was at original position 89, etc.
        for (i, (&v, &idx)) in values[..80].iter().zip(s.indices[..80].iter()).enumerate() {
            let expected_original_pos = n - 1 - (i + 10);
            assert_eq!(
                idx, expected_original_pos,
                "Index at position {i}: value {v} came from original position {expected_original_pos}"
            );
        }
    }

    #[test]
    fn test_linear_fit_large_stack() {
        // 100 frames with a linear trend (value = index) plus one outlier.
        // values[50] = 1000.0 (original position 50). Should be rejected.
        // All other values should survive.
        let n = 100;
        let mut values: Vec<f32> = (0..n).map(|i| i as f32).collect();
        values[50] = 1000.0;
        let mut s = scratch();

        let remaining = LinearFitClipConfig::new(3.0, 3.0, 3).reject(&mut values, &mut s);

        assert_eq!(
            remaining, 99,
            "Only the outlier at position 50 should be rejected"
        );
        // The outlier (1000.0) must not be among survivors
        for &v in &values[..remaining] {
            assert!(v < 100.0, "Outlier 1000.0 should not survive, got {v}");
        }
        // Original frame index 50 must not appear in survivors
        assert!(
            !s.indices[..remaining].contains(&50),
            "Frame 50 (the outlier) should be rejected"
        );
    }

    #[test]
    fn test_reset_indices_basic() {
        let mut indices = Vec::new();
        reset_indices(&mut indices, 5);
        assert_eq!(indices, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_reset_indices_reuses_allocation() {
        let mut indices = Vec::with_capacity(100);
        reset_indices(&mut indices, 5);
        assert_eq!(indices, vec![0, 1, 2, 3, 4]);
        assert!(indices.capacity() >= 100, "should preserve allocation");

        // Reset to different size — should reuse existing allocation
        reset_indices(&mut indices, 3);
        assert_eq!(indices, vec![0, 1, 2]);
        assert!(indices.capacity() >= 100);
    }

    #[test]
    fn test_reset_indices_overwrites_stale_data() {
        let mut indices = vec![99, 88, 77, 66, 55];
        reset_indices(&mut indices, 5);
        assert_eq!(indices, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_reset_indices_empty() {
        let mut indices = vec![1, 2, 3];
        reset_indices(&mut indices, 0);
        assert!(indices.is_empty());
    }

    #[test]
    fn test_no_outliers_possible_tight_cluster() {
        // 20 values all equal to 10.0 → stddev=0 → returns true
        let values = vec![10.0f32; 20];
        assert!(SigmaClipConfig::no_outliers_possible(&values, 2.5));
    }

    #[test]
    fn test_no_outliers_possible_small_spread() {
        // values = [10, 10, 10, ..., 10, 11, 9] (18×10 + 11 + 9), N=20
        // trimmed (exclude min=9, max=11): 18×10 + one of {9,11} excluded
        // Actually exclude the single min (9) and single max (11):
        //   trimmed = 18×10.0 = 180, trimmed_n = 18, trimmed_mean = 10.0
        //   trimmed variance: 18 × (10-10)² / 17 = 0
        //   stddev = 0 → returns true
        let mut values = vec![10.0f32; 18];
        values.push(11.0);
        values.push(9.0);
        assert!(SigmaClipConfig::no_outliers_possible(&values, 2.5));
    }

    #[test]
    fn test_no_outliers_possible_clear_outlier() {
        // 17×10.0 + [9.0, 11.0, 100.0], N=20
        // min=9.0, max=100.0, excluded from trimmed stats.
        // trimmed: 17×10.0 + 11.0 = 181, trimmed_n=18, trimmed_mean=181/18 ≈ 10.056
        // trimmed sum_sq = 17×100 + 121 = 1821
        // trimmed var = (1821 - 181²/18) / 17 = (1821 - 1820.056) / 17 ≈ 0.056
        // trimmed stddev ≈ 0.236
        // max_dev = |100.0 - 10.056| = 89.94
        // threshold = 2.5 × 0.236 = 0.59
        // 89.94 > 0.59 → returns false (outlier detected)
        let mut values = vec![10.0f32; 17];
        values.extend([9.0, 11.0, 100.0]);
        assert!(!SigmaClipConfig::no_outliers_possible(&values, 2.5));
    }

    #[test]
    fn test_no_outliers_possible_returns_false_for_small_n() {
        // N < 10 always returns false (trimming would distort too much)
        let values = vec![10.0f32; 5];
        assert!(!SigmaClipConfig::no_outliers_possible(&values, 2.5));

        let values = vec![10.0f32; 9];
        assert!(!SigmaClipConfig::no_outliers_possible(&values, 2.5));
    }

    #[test]
    fn test_no_outliers_possible_moderate_spread() {
        // Linearly spaced: [0, 1, 2, ..., 19], N=20
        // min=0, max=19, excluded → trimmed = [1..18], trimmed_n=18
        // trimmed_sum = 1+2+...+18 = 171, trimmed_mean = 171/18 = 9.5
        // trimmed_sum_sq = 1+4+9+...+324 = 2109
        // trimmed_var = (2109 - 171²/18) / 17 = (2109 - 1624.5) / 17 = 484.5/17 ≈ 28.5
        // trimmed_stddev ≈ 5.34
        // max_dev = max(|0 - 9.5|, |19 - 9.5|) = 9.5
        // threshold = 2.5 × 5.34 = 13.35
        // 9.5 < 13.35 → returns true (no outlier exceeds threshold)
        let values: Vec<f32> = (0..20).map(|i| i as f32).collect();
        assert!(SigmaClipConfig::no_outliers_possible(&values, 2.5));
    }

    #[test]
    fn test_no_outliers_possible_asymmetric_outliers() {
        // [1, 1.5, 2, 2.5, 3, 50, 80, 100, 1, 1] — N=10
        // min=1.0, max=100.0, excluded
        // trimmed: [1.5, 2, 2.5, 3, 50, 80, 1, 1] — n=8
        // trimmed_sum = 141, trimmed_mean = 17.625
        // Outlier 80 is still in trimmed set → large stddev
        // max_dev = |100 - 17.625| = 82.375
        // Should return false (outliers present)
        let values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 50.0, 80.0, 100.0, 1.0, 1.0];
        assert!(!SigmaClipConfig::no_outliers_possible(&values, 2.0));
    }

    #[test]
    fn test_no_outliers_possible_does_not_break_rejection() {
        // End-to-end: early exit must not prevent correct rejection.
        // Need data with non-zero MAD so sigma clipping can define a threshold.
        // 47 values at 9.0 + 47 values at 11.0 + 6 outliers = 100 values.
        // median = 11 (or 9, depending on order), MAD ≈ 1, sigma ≈ 1.48
        // threshold = 2.5 × 1.48 = 3.7 → outliers at 100+ are clearly rejected.
        let mut values: Vec<f32> = vec![9.0; 47];
        values.extend(vec![11.0; 47]);
        values.extend([100.0, 200.0, 500.0, 600.0, 700.0, 800.0]);
        let remaining = SigmaClipConfig::new(2.5, 3).reject(&mut values, &mut scratch());
        // All 6 large outliers must be rejected
        for &v in &values[..remaining] {
            assert!(v < 20.0, "Outlier {v} should have been clipped");
        }
        assert_eq!(remaining, 94);
    }

    #[test]
    fn test_no_outliers_possible_clean_data_skips_quickselect() {
        // 100×10.0 (perfectly uniform) — early exit should trigger,
        // meaning reject returns all values with no changes.
        let mut values = vec![10.0f32; 100];
        let remaining = SigmaClipConfig::new(2.5, 3).reject(&mut values, &mut scratch());
        assert_eq!(remaining, 100);
        // All values unchanged
        for &v in &values {
            assert!((v - 10.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_weighted_mean_indexed_all_zero_weights() {
        // All weights zero → should return 0.0, not NaN/Inf
        let values = [5.0f32, 10.0, 15.0];
        let weights = [0.0f32, 0.0, 0.0];
        let indices = [0, 1, 2];
        let mut buf = Vec::new();
        let result = weighted_mean_indexed(&values, &weights, &indices, &mut buf);
        assert!(
            (result - 0.0).abs() < 1e-6,
            "Should return 0.0, got {}",
            result
        );
    }

    #[test]
    fn test_weighted_mean_indexed_partial_zero_weights() {
        // values=[5, 10, 15], weights=[0, 2, 0], indices=[0, 1, 2]
        // Only middle value has nonzero weight → mean = 10*2 / 2 = 10.0
        let values = [5.0f32, 10.0, 15.0];
        let weights = [0.0f32, 2.0, 0.0];
        let indices = [0, 1, 2];
        let mut buf = Vec::new();
        let result = weighted_mean_indexed(&values, &weights, &indices, &mut buf);
        assert!((result - 10.0).abs() < 1e-6);
    }

    #[test]
    fn weighted_mean_indexed_preserves_small_increments() {
        // 0.5 sits below half the ULP of 2e7, so a naive f32 accumulation would
        // drop every increment; the wider/compensated weighted mean recovers them.
        // Weights are all 1.0, so the result is a plain mean.
        let mut values = vec![0.5_f32; 17];
        values[0] = 2.0e7;
        let weights = vec![1.0_f32; values.len()];
        let indices: Vec<usize> = (0..values.len()).collect();

        let mut buf = Vec::new();
        let mean = weighted_mean_indexed(&values, &weights, &indices, &mut buf);

        // True mean = (2e7 + 16*0.5) / 17 = 20_000_008 / 17 ≈ 1_176_471.06.
        // A naive f32 sum gives ~2e7/17 ≈ 1_176_470.59, off by 8/17 ≈ 0.47.
        let expected = (2.0e7_f64 + 8.0) / 17.0;
        assert!(
            (mean as f64 - expected).abs() < 0.1,
            "precise mean {mean} must be within 0.1 of {expected} (naive loses ~0.47)"
        );
    }
}
