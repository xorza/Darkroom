//! Pixel rejection algorithms for stacking.
//!
//! This module contains various outlier rejection methods used during image stacking:
//! - Sigma clipping (Kappa-Sigma)
//! - Winsorized sigma clipping
//! - Linear fit clipping
//! - Percentile clipping
//! - Generalized Extreme Studentized Deviate (GESD)

use crate::error::InvalidConfigField;
use crate::math::statistics::{mad_f32_fast, mad_to_sigma, median_f32_fast};
use crate::math::sum::weighted_mean_f32;
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

    /// Validate the clip thresholds and iteration count.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
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
    fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
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

        sort_with_indices(values, scratch, n0);

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

    /// Validate the clip thresholds.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        validate_sigma_bounds(self.sigma_low, self.sigma_high)
    }

    /// Phase 1: Iteratively Winsorize to get a robust [`WinsorizedEstimate`].
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
    fn robust_estimate(&self, values: &[f32], working: &mut Vec<f32>) -> WinsorizedEstimate {
        working.clear();
        working.extend_from_slice(values);
        working.sort_unstable_by(f32::total_cmp);

        // `select_nth_unstable`'s median (index len/2) equals the sorted element at that index,
        // so `working[mid]` reproduces the previous `median_f32_fast` result exactly.
        let mid = working.len() / 2;
        let mut center = working[mid];
        let mut sigma = winsorized_stddev(working, center) * WINSORIZED_CORRECTION;

        if sigma < f32::EPSILON {
            return WinsorizedEstimate { center, sigma: 0.0 };
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
                return WinsorizedEstimate { center, sigma: 0.0 };
            }

            let converged = (sigma_new - sigma).abs() <= sigma * WINSORIZE_CONVERGENCE;
            sigma = sigma_new;

            if converged {
                break;
            }
        }

        WinsorizedEstimate { center, sigma }
    }

    /// Phase 2: Reject outliers using the robust estimate from phase 1.
    ///
    /// Standard sigma clipping with the [`WinsorizedEstimate`] and the user's
    /// sigma_low/sigma_high thresholds.
    fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        reset_indices(&mut scratch.indices, values.len());

        if values.len() <= 2 {
            return values.len();
        }

        let estimate = self.robust_estimate(values, &mut scratch.floats_a);

        if estimate.sigma < f32::EPSILON {
            return values.len();
        }

        let n = values.len();
        compact_within(
            values,
            &mut scratch.indices,
            n,
            self.sigma_low * estimate.sigma,
            self.sigma_high * estimate.sigma,
            |_| estimate.center,
        )
    }
}

/// Location and spread from [`WinsorizedClipConfig::robust_estimate`].
///
/// Deliberately not a [`MedianMad`](crate::math::statistics::MedianMad): `sigma` is a
/// bias-corrected sample standard deviation about the Winsorized center — see
/// [`winsorized_stddev`] — so it is already in Gaussian units, and sending it through that
/// type's MAD rescale would silently inflate every Winsorized threshold by 1.4826.
#[derive(Debug, Clone, Copy)]
struct WinsorizedEstimate {
    center: f32,
    sigma: f32,
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

    /// Validate the clip thresholds and iteration count.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        validate_sigma_bounds(self.sigma_low, self.sigma_high)?;
        InvalidConfigField::check(
            self.max_iterations >= 1,
            "max_iterations",
            "at least 1",
            self.max_iterations as f64,
        )
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
    fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
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
                sort_with_indices(values, scratch, len);

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

    /// Validate that each end clips a sane share and that together they leave survivors.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
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
    fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        let n = values.len();
        reset_indices(&mut scratch.indices, n);

        if n <= 2 {
            return n;
        }

        sort_with_indices(values, scratch, n);

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

    /// Validate the significance level.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite("GESD alpha", "finite and in [0, 1)", self.alpha, |value| {
            (0.0..1.0).contains(&value)
        })
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
    fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
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

/// Both clip thresholds must be finite and positive: they scale a spread estimate, so a
/// non-positive one inverts the keep-band and a non-finite one empties it.
fn validate_sigma_bounds(sigma_low: f32, sigma_high: f32) -> Result<(), InvalidConfigField> {
    InvalidConfigField::finite("sigma_low", "finite and positive", sigma_low, |value| {
        value > 0.0
    })?;
    InvalidConfigField::finite("sigma_high", "finite and positive", sigma_high, |value| {
        value > 0.0
    })
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
fn sort_with_indices(values: &mut [f32], scratch: &mut ScratchBuffers, n: usize) {
    // Destructured under the role each generic buffer plays here. No caller reads the f32 scratch
    // after this returns, so which one it borrows is this function's business rather than a
    // parameter every call site has to get right.
    let ScratchBuffers {
        indices,
        floats_b: scratch_buf,
        usize_a: perm,
        usize_b: old_indices,
        ..
    } = scratch;

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

    /// Validate the held configuration, if any.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        match self {
            Self::None => Ok(()),
            Self::SigmaClip(config) => config.validate(),
            Self::Winsorized(config) => config.validate(),
            Self::LinearFit(config) => config.validate(),
            Self::Percentile(config) => config.validate(),
            Self::Gesd(config) => config.validate(),
        }
    }

    /// Partition values by rejection algorithm, returning the number of survivors.
    ///
    /// After return, `values[..remaining]` holds the surviving values and `scratch.indices`
    /// their original frame indices (kept paired). `None` does no work and returns `values.len()`.
    fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        match self {
            Rejection::None => values.len(),
            Rejection::SigmaClip(c) => c.reject(values, scratch),
            Rejection::Winsorized(c) => c.reject(values, scratch),
            Rejection::LinearFit(c) => c.reject(values, scratch),
            Rejection::Percentile(c) => c.reject(values, scratch),
            Rejection::Gesd(c) => c.reject(values, scratch),
        }
    }

    /// Reject outliers, then reduce the survivors to their weighted mean.
    ///
    /// The one reduction entry point: `values` and `weights` are the samples actually reaching
    /// this pixel, and the returned sample always carries its survivor count. `measure_quality`
    /// asks for the survivors' effective weight as well — skipped when no output plane will read
    /// it, since it is a second pass over the frames at every pixel.
    ///
    /// Rejection reorders `values`, so weights are re-paired through `scratch.indices` rather
    /// than by position.
    pub(crate) fn combine_mean(
        &self,
        values: &mut [f32],
        weights: &[f32],
        scratch: &mut ScratchBuffers,
        measure_quality: bool,
    ) -> CombinedSample {
        debug_assert_eq!(values.len(), weights.len());
        if let Rejection::None = self {
            let value = weighted_mean_f32(values, weights);
            return if measure_quality {
                CombinedSample::from_all(value, weights)
            } else {
                CombinedSample::value_only(value, values.len())
            };
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
        if measure_quality {
            CombinedSample::from_survivors(value, weights, remaining, survivors.iter().copied())
        } else {
            CombinedSample::value_only(value, remaining)
        }
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
mod tests;
