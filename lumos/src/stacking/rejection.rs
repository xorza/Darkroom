//! Pixel rejection algorithms for stacking.
//!
//! This module contains various outlier rejection methods used during image stacking:
//! - Sigma clipping (Kappa-Sigma)
//! - Winsorized sigma clipping
//! - Linear fit clipping
//! - Percentile clipping
//! - Generalized Extreme Studentized Deviate (GESD)

use crate::math::{self, mad_f32_with_scratch, mad_to_sigma};
use crate::stacking::cache::ScratchBuffers;

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
        assert!(sigma > 0.0, "Sigma must be positive");
        assert!(max_iterations > 0, "Max iterations must be at least 1");
        Self {
            sigma_low: sigma,
            sigma_high: sigma,
            max_iterations,
        }
    }

    /// Create asymmetric sigma clipping with separate low/high thresholds.
    pub fn new_asymmetric(sigma_low: f32, sigma_high: f32, max_iterations: u32) -> Self {
        assert!(sigma_low > 0.0, "Sigma low must be positive");
        assert!(sigma_high > 0.0, "Sigma high must be positive");
        assert!(max_iterations > 0, "Max iterations must be at least 1");
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
    pub(crate) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        reset_indices(&mut scratch.indices, values.len());

        if values.len() <= 2 {
            return values.len();
        }

        let mut len = values.len();

        for _ in 0..self.max_iterations {
            if len <= 2 {
                break;
            }

            let active = &mut values[..len];

            let center = math::median_f32_mut(active);
            let mad = mad_f32_with_scratch(&values[..len], center, &mut scratch.floats_a);
            let sigma = mad_to_sigma(mad);

            if sigma < f32::EPSILON {
                break;
            }

            let low_threshold = self.sigma_low * sigma;
            let high_threshold = self.sigma_high * sigma;

            let mut write_idx = 0;
            for read_idx in 0..len {
                let diff = values[read_idx] - center;
                let keep = if diff < 0.0 {
                    -diff <= low_threshold
                } else {
                    diff <= high_threshold
                };
                if keep {
                    values[write_idx] = values[read_idx];
                    scratch.indices[write_idx] = scratch.indices[read_idx];
                    write_idx += 1;
                }
            }

            if write_idx == len {
                break;
            }
            len = write_idx;
        }

        len
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
        assert!(sigma > 0.0, "Sigma must be positive");
        Self {
            sigma_low: sigma,
            sigma_high: sigma,
        }
    }

    pub fn new_asymmetric(sigma_low: f32, sigma_high: f32) -> Self {
        assert!(sigma_low > 0.0, "Sigma low must be positive");
        assert!(sigma_high > 0.0, "Sigma high must be positive");
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
    /// Returns (center, corrected_sigma).
    fn robust_estimate(
        &self,
        values: &[f32],
        working: &mut Vec<f32>,
        scratch: &mut Vec<f32>,
    ) -> (f32, f32) {
        working.clear();
        working.extend_from_slice(values);

        // Initial estimates
        scratch.clear();
        scratch.extend_from_slice(working);
        let mut center = math::median_f32_mut(scratch);
        let mut sigma = winsorized_stddev(working, center) * WINSORIZED_CORRECTION;

        if sigma < f32::EPSILON {
            return (center, 0.0);
        }

        for _ in 0..WINSORIZE_MAX_ITER {
            let low_bound = center - HUBER_C * sigma;
            let high_bound = center + HUBER_C * sigma;

            // Winsorize: clamp outliers to boundary values
            for v in working.iter_mut() {
                if *v < low_bound {
                    *v = low_bound;
                } else if *v > high_bound {
                    *v = high_bound;
                }
            }

            // Recompute center (median of winsorized values)
            scratch.clear();
            scratch.extend_from_slice(working);
            center = math::median_f32_mut(scratch);

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

        let (center, sigma) =
            self.robust_estimate(values, &mut scratch.floats_a, &mut scratch.floats_b);

        if sigma < f32::EPSILON {
            return values.len();
        }

        let low_threshold = self.sigma_low * sigma;
        let high_threshold = self.sigma_high * sigma;

        let mut write_idx = 0;
        for read_idx in 0..values.len() {
            let diff = values[read_idx] - center;
            let keep = if diff < 0.0 {
                -diff <= low_threshold
            } else {
                diff <= high_threshold
            };
            if keep {
                values[write_idx] = values[read_idx];
                scratch.indices[write_idx] = scratch.indices[read_idx];
                write_idx += 1;
            }
        }

        write_idx
    }
}

/// Compute standard deviation from mean (not MAD) for Winsorized values.
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
        assert!(sigma_low > 0.0, "Sigma low must be positive");
        assert!(sigma_high > 0.0, "Sigma high must be positive");
        assert!(max_iterations > 0, "Max iterations must be at least 1");
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
                let center = math::median_f32_mut(&mut scratch.floats_a);
                let mad = mad_f32_with_scratch(&values[..len], center, &mut scratch.floats_a);
                let sigma = mad_to_sigma(mad);

                if sigma < f32::EPSILON {
                    break;
                }

                let low_threshold = self.sigma_low * sigma;
                let high_threshold = self.sigma_high * sigma;

                let mut write_idx = 0;
                for read_idx in 0..len {
                    let diff = values[read_idx] - center;
                    let keep = if diff < 0.0 {
                        -diff <= low_threshold
                    } else {
                        diff <= high_threshold
                    };
                    if keep {
                        values[write_idx] = values[read_idx];
                        scratch.indices[write_idx] = scratch.indices[read_idx];
                        write_idx += 1;
                    }
                }

                if write_idx == len {
                    break;
                }
                len = write_idx;
            } else {
                // Subsequent passes: linear fit rejection

                // Sort remaining values with index co-array
                sort_with_indices(values, &mut scratch.indices, &mut scratch.floats_b, len);

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

                let low_threshold = self.sigma_low * sigma;
                let high_threshold = self.sigma_high * sigma;

                // Reject each pixel against its own fitted value
                let mut write_idx = 0;
                for read_idx in 0..len {
                    let fitted = a + b * read_idx as f32;
                    let diff = values[read_idx] - fitted;
                    let keep = if diff < 0.0 {
                        -diff <= low_threshold
                    } else {
                        diff <= high_threshold
                    };
                    if keep {
                        values[write_idx] = values[read_idx];
                        scratch.indices[write_idx] = scratch.indices[read_idx];
                        write_idx += 1;
                    }
                }

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
        assert!(
            (0.0..=50.0).contains(&low_percentile),
            "Low percentile must be between 0 and 50"
        );
        assert!(
            (0.0..=50.0).contains(&high_percentile),
            "High percentile must be between 0 and 50"
        );
        assert!(
            low_percentile + high_percentile < 100.0,
            "Total clipping must be less than 100%"
        );
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

        sort_with_indices(values, &mut scratch.indices, &mut scratch.floats_a, n);

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
/// A rigorous statistical test for detecting multiple outliers.
/// Best for large datasets (> 50 frames).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GesdConfig {
    /// Significance level for the test (typically 0.05).
    pub alpha: f32,
    /// Maximum number of outliers to detect.
    /// If None, uses 25% of data size.
    pub max_outliers: Option<usize>,
    /// Relaxation factor for pixels below the center (>= 1.0).
    /// Multiplies sigma when computing test statistics for dark outliers,
    /// making the test more tolerant of low-side deviations.
    /// Default: 1.5 (matching PixInsight).
    pub low_relaxation: f32,
}

impl Default for GesdConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            max_outliers: None,
            low_relaxation: 1.5,
        }
    }
}

impl GesdConfig {
    pub fn new(alpha: f32, max_outliers: Option<usize>) -> Self {
        assert!((0.0..1.0).contains(&alpha), "Alpha must be between 0 and 1");
        Self {
            alpha,
            max_outliers,
            low_relaxation: 1.5,
        }
    }

    /// Create GESD config with custom low relaxation factor.
    pub fn with_low_relaxation(mut self, low_relaxation: f32) -> Self {
        assert!(low_relaxation >= 1.0, "Low relaxation must be >= 1.0");
        self.low_relaxation = low_relaxation;
        self
    }

    /// Get maximum outliers, defaulting to 25% of data size.
    pub fn max_outliers_for_size(&self, n: usize) -> usize {
        self.max_outliers.unwrap_or(n / 4)
    }

    /// Partition values by GESD test, returning the number of survivors.
    ///
    /// Uses median + MAD for robust test statistics (matching PixInsight),
    /// with t-distribution critical values via inverse normal approximation.
    ///
    /// After return, `values[..remaining]` contains surviving values and
    /// `indices[..remaining]` contains their original frame indices.
    pub(crate) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        reset_indices(&mut scratch.indices, values.len());

        let original_len = values.len();

        if values.len() <= 3 {
            return values.len();
        }

        let max_outliers = self
            .max_outliers_for_size(original_len)
            .min(original_len - 3);
        let mut len = original_len;

        // Phase 1: Compute test statistics for each candidate outlier
        scratch.floats_b.clear();

        for _ in 0..max_outliers {
            if len <= 3 {
                break;
            }

            // Compute median of remaining values
            scratch.floats_a.clear();
            scratch.floats_a.extend_from_slice(&values[..len]);
            let median = math::median_f32_mut(&mut scratch.floats_a);

            // Compute MAD
            let mad = mad_f32_with_scratch(&values[..len], median, &mut scratch.floats_a);
            let sigma = mad_to_sigma(mad);

            if sigma < f32::EPSILON {
                break;
            }

            // Find the most extreme value from median, applying asymmetric
            // relaxation: dark pixels (below median) use sigma * low_relaxation,
            // making them harder to reject.
            let mut max_r = 0.0f32;
            let mut max_idx = 0;
            for (idx, &v) in values[..len].iter().enumerate() {
                let diff = v - median;
                let effective_sigma = if diff < 0.0 {
                    sigma * self.low_relaxation
                } else {
                    sigma
                };
                let r = diff.abs() / effective_sigma;
                if r > max_r {
                    max_r = r;
                    max_idx = idx;
                }
            }

            scratch.floats_b.push(max_r);

            // Tentatively remove the most deviant value
            values.swap(max_idx, len - 1);
            scratch.indices.swap(max_idx, len - 1);
            len -= 1;
        }

        // Phase 2: Determine actual outlier count using critical values (backward scan)
        let num_candidates = scratch.floats_b.len();
        let mut num_outliers = 0;

        for i in (0..num_candidates).rev() {
            let ni = (len + num_candidates - i) as f32; // effective n at step i
            let p = 1.0 - self.alpha / (2.0 * (ni - i as f32));
            let t_crit = inverse_normal_approx(p);

            let numerator = (ni - i as f32 - 1.0) * t_crit;
            let denominator = ((ni - i as f32 - 2.0 + t_crit * t_crit) * (ni - i as f32)).sqrt();
            let lambda = numerator / denominator;

            if scratch.floats_b[i] > lambda {
                num_outliers = i + 1;
                break;
            }
        }

        // The outliers were swapped to the end during phase 1.
        // Return the number of survivors.
        len + num_candidates - num_outliers
    }
}

/// Approximate inverse of standard normal CDF.
/// Uses Abramowitz and Stegun approximation.
fn inverse_normal_approx(p: f32) -> f32 {
    // Constrain p to valid range
    let p = p.clamp(0.0001, 0.9999);

    // Use symmetry: if p > 0.5, compute for 1-p and negate
    let (p_adj, sign) = if p > 0.5 { (1.0 - p, 1.0) } else { (p, -1.0) };

    // Rational approximation coefficients
    let t = (-2.0 * p_adj.ln()).sqrt();

    // Coefficients for approximation
    let c0 = 2.515517;
    let c1 = 0.802853;
    let c2 = 0.010328;
    let d1 = 1.432788;
    let d2 = 0.189269;
    let d3 = 0.001308;

    let numerator = c0 + c1 * t + c2 * t * t;
    let denominator = 1.0 + d1 * t + d2 * t * t + d3 * t * t * t;

    sign * (t - numerator / denominator)
}

/// Reset an indices buffer to [0, 1, 2, ...n), reusing the allocation.
fn reset_indices(indices: &mut Vec<usize>, n: usize) {
    indices.clear();
    indices.extend(0..n);
}

/// Sort `values[..n]` and `indices[..n]` together by value.
/// Uses insertion sort for small N (optimal for typical 10–50 frame stacks)
/// and introsort via `sort_unstable_by` for large N to avoid O(N^2).
fn sort_with_indices(
    values: &mut [f32],
    indices: &mut [usize],
    scratch_buf: &mut Vec<f32>,
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
        // Build position permutation, sort by values, apply to both arrays.
        // Reuse scratch_buf for the value copy; allocate perm on stack-like Vec.
        let mut perm: Vec<usize> = (0..n).collect();
        perm.sort_unstable_by(|&a, &b| values[a].partial_cmp(&values[b]).unwrap());

        scratch_buf.clear();
        scratch_buf.extend_from_slice(&values[..n]);
        let old_indices: Vec<usize> = indices[..n].to_vec();
        for (dst, &src) in perm.iter().enumerate() {
            values[dst] = scratch_buf[src];
            indices[dst] = old_indices[src];
        }
    }
}

// ============================================================================
// Rejection Enum — dispatches to algorithm implementations above
// ============================================================================

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
        Self::Gesd(GesdConfig::new(0.05, None))
    }

    /// Partition values by rejection algorithm, returning the number of survivors.
    ///
    /// After return, `values[..remaining]` contains surviving values.
    /// For `Winsorized`, no partitioning occurs (returns `values.len()`).
    pub(crate) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        match self {
            Rejection::None => values.len(),
            Rejection::SigmaClip(c) => c.reject(values, scratch),
            Rejection::Winsorized(c) => c.reject(values, scratch),
            Rejection::LinearFit(c) => c.reject(values, scratch),
            Rejection::Gesd(c) => c.reject(values, scratch),
            Rejection::Percentile(c) => c.reject(values, scratch),
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
        // None doesn't reorder values, so weights align directly
        if let Rejection::None = self {
            return match weights {
                Some(w) => math::weighted_mean_f32(values, w),
                None => math::mean_f32(values),
            };
        }

        // Rejection variants that reorder values: use index mapping for weights
        let remaining = self.reject(values, scratch);

        match weights {
            Some(w) if remaining > 0 => {
                weighted_mean_indexed(&values[..remaining], w, &scratch.indices[..remaining])
            }
            _ => math::mean_f32(&values[..remaining]),
        }
    }
}

/// Compute weighted mean using index mapping.
///
/// `indices[i]` maps `values[i]` to `weights[indices[i]]`, maintaining correct
/// alignment after rejection functions have reordered the values array.
///
/// Preconditions: `values` is non-empty, `indices.len() == values.len()`,
/// all `indices[i] < weights.len()`, and total weight > 0.
fn weighted_mean_indexed(values: &[f32], weights: &[f32], indices: &[usize]) -> f32 {
    debug_assert!(!values.is_empty());
    debug_assert_eq!(values.len(), indices.len());

    let mut sum = 0.0f32;
    let mut weight_sum = 0.0f32;

    for (&v, &idx) in values.iter().zip(indices.iter()) {
        let w = weights[idx];
        sum += v * w;
        weight_sum += w;
    }

    sum / weight_sum
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== Config Construction Tests ==========

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
    #[should_panic(expected = "Sigma must be positive")]
    fn test_sigma_clip_config_zero_sigma() {
        SigmaClipConfig::new(0.0, 3);
    }

    #[test]
    #[should_panic(expected = "Sigma low must be positive")]
    fn test_sigma_clip_config_zero_sigma_low() {
        SigmaClipConfig::new_asymmetric(0.0, 3.0, 3);
    }

    #[test]
    #[should_panic(expected = "Sigma high must be positive")]
    fn test_sigma_clip_config_zero_sigma_high() {
        SigmaClipConfig::new_asymmetric(3.0, 0.0, 3);
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
    #[should_panic(expected = "Low percentile must be between 0 and 50")]
    fn test_percentile_config_invalid_low() {
        PercentileClipConfig::new(60.0, 10.0);
    }

    #[test]
    fn test_gesd_config_default() {
        let config = GesdConfig::default();
        assert!((config.alpha - 0.05).abs() < f32::EPSILON);
        assert!(config.max_outliers.is_none());
        assert!((config.low_relaxation - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_gesd_config_max_outliers_for_size() {
        let config = GesdConfig::default();
        assert_eq!(config.max_outliers_for_size(100), 25);

        let config_explicit = GesdConfig::new(0.05, Some(10));
        assert_eq!(config_explicit.max_outliers_for_size(100), 10);
    }

    // ========== Algorithm Tests ==========

    #[test]
    fn test_sigma_clip_removes_outlier() {
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values, &mut scratch());
        let mean = math::mean_f32(&values[..remaining]);
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
        let mean = math::mean_f32(&values[..remaining]);
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
        let mean = math::mean_f32(&values[..remaining]);
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
        assert!((math::mean_f32(&v1[..r1]) - math::mean_f32(&v2[..r2])).abs() < 1e-6,);
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
        let mean = math::mean_f32(&values[..remaining]);
        assert!(mean < 10.0, "Mean of survivors should be low, got {mean}");
    }

    #[test]
    fn test_linear_fit_constant_data() {
        let mut values = vec![5.0, 5.0, 5.0, 5.0, 5.0];
        let remaining = LinearFitClipConfig::default().reject(&mut values, &mut scratch());
        assert_eq!(remaining, 5);
        assert!((math::mean_f32(&values[..remaining]) - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_linear_fit_rejects_outlier() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 100.0, 6.0];
        let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut scratch());
        assert!(remaining < 6);
        assert!(math::mean_f32(&values[..remaining]) < 20.0);
    }

    #[test]
    fn test_percentile_clip() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let remaining = PercentileClipConfig::new(20.0, 20.0).reject(&mut values, &mut scratch());
        assert_eq!(remaining, 6);
        // Mean of [3, 4, 5, 6, 7, 8] = 5.5
        assert!((math::mean_f32(&values[..remaining]) - 5.5).abs() < 0.01);
    }

    #[test]
    fn test_gesd_removes_single_bright_outlier() {
        // n=8, max_outliers=default(25%=2), relax=1.0 (symmetric).
        // Hand-computed: step 0 removes 100.0 (r=667.8), step 1 tries 1.2
        // (r=1.35 < lambda=1.63 → keep). Phase 2 confirms 1 outlier. Result: 7.
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 100.0];
        let mut s = scratch();
        let remaining = GesdConfig::new(0.05, None)
            .with_low_relaxation(1.0)
            .reject(&mut values, &mut s);
        assert_eq!(
            remaining, 7,
            "Exactly the bright outlier should be rejected"
        );
        assert!(
            !s.indices[..remaining].contains(&7),
            "Index 7 (100.0) must be rejected, survivors: {:?}",
            &s.indices[..remaining]
        );
        for &v in &values[..remaining] {
            assert!(v < 2.0, "Outlier {v} should not survive");
        }
    }

    #[test]
    fn test_gesd_no_outliers() {
        // Constant values — sigma=0 so no outliers detected
        let mut values = vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let remaining = GesdConfig::new(0.05, Some(3)).reject(&mut values, &mut scratch());
        assert_eq!(remaining, 8, "No outliers in constant data");
    }

    #[test]
    fn test_gesd_keeps_tight_cluster() {
        // Tight cluster, no real outliers. max_outliers=default(25%=2).
        // Hand-computed: step 0 tries 1.2 (r=1.35 < lambda=1.84), step 1
        // tries 0.8 (r=0.90 < lambda=1.63). Phase 2: both keep. Result: 8.
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 1.1];
        let remaining = GesdConfig::default().reject(&mut values, &mut scratch());
        assert_eq!(remaining, 8, "Tight cluster should have no rejections");
    }

    #[test]
    fn test_inverse_normal_approx() {
        let z_95 = inverse_normal_approx(0.95);
        assert!((z_95 - 1.645).abs() < 0.05);

        let z_975 = inverse_normal_approx(0.975);
        assert!((z_975 - 1.96).abs() < 0.05);

        // Symmetry check
        let z_05 = inverse_normal_approx(0.05);
        assert!((z_05 + z_95).abs() < 0.1);
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

    // ========== Index Tracking Tests ==========

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
    fn test_gesd_indices_track_survivors() {
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 100.0];
        let mut s = scratch();
        let remaining = GesdConfig::new(0.05, Some(3)).reject(&mut values, &mut s);

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

    // ========== Rejection Enum Tests ==========

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

    // ========== Rejection::combine_mean Tests ==========

    fn scratch() -> ScratchBuffers {
        ScratchBuffers {
            indices: vec![],
            floats_a: vec![],
            floats_b: vec![],
        }
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

    // ========== Weight-Value Alignment Tests ==========

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
    fn test_gesd_relaxation_correctness() {
        // Cluster of 10 around 10.0, dark pixel at 9.6 (idx 10), bright at 50.0 (idx 11).
        // Hand-computed with our median+MAD+A&S inverse normal:
        //   Initial (n=12): median=10.0, MAD=0.125, sigma=0.1853
        //   Phase 1, step 0: 50.0 removed (r=215.8 >> any lambda)
        //   Phase 1, step 1: after removing 50.0 (n=11), sigma=0.1483
        //     relax=1.0: r = 0.4/0.1483 = 2.698 > lambda(2.005) → candidate
        //     relax=1.5: r = 0.4/(0.1483×1.5) = 1.799 < lambda(2.005) → not candidate
        //   Phase 2 backward: relax=1.0 rejects dark; relax=1.5 does not.
        let cluster = vec![10.0, 10.1, 9.9, 10.0, 10.2, 9.8, 10.05, 9.95, 10.15, 9.85];

        // With default relaxation (1.5): dark pixel survives, only bright rejected
        let mut v1: Vec<f32> = cluster.iter().copied().chain([9.6, 50.0]).collect();
        let mut s1 = scratch();
        let r1 = GesdConfig::new(0.05, Some(3)).reject(&mut v1, &mut s1);
        let surv1: Vec<usize> = s1.indices[..r1].to_vec();
        assert_eq!(r1, 11, "relax=1.5: only bright outlier rejected");
        assert!(
            surv1.contains(&10),
            "relax=1.5: dark pixel must survive: {:?}",
            surv1
        );
        assert!(!surv1.contains(&11), "relax=1.5: bright must be rejected");

        // Without relaxation: both outliers rejected, survivors = clean cluster
        let mut v2: Vec<f32> = cluster.iter().copied().chain([9.6, 50.0]).collect();
        let mut s2 = scratch();
        let r2 = GesdConfig::new(0.05, Some(3))
            .with_low_relaxation(1.0)
            .reject(&mut v2, &mut s2);
        let surv2: Vec<usize> = s2.indices[..r2].to_vec();
        assert_eq!(r2, 10, "relax=1.0: both outliers rejected");
        assert!(
            !surv2.contains(&10),
            "relax=1.0: dark pixel must be rejected"
        );
        assert!(!surv2.contains(&11), "relax=1.0: bright must be rejected");

        // Verify surviving values match the clean cluster exactly
        let mut got: Vec<f32> = v2[..r2].to_vec();
        got.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut want = cluster.clone();
        want.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(got, want, "survivors must be exactly the clean cluster");
    }

    #[test]
    fn test_gesd_relaxation_boundary() {
        // dark=9.5 is too extreme: with relax=1.5, r=2.248 > lambda=2.005
        // so even default relaxation cannot save it — both outliers rejected.
        let cluster = vec![10.0, 10.1, 9.9, 10.0, 10.2, 9.8, 10.05, 9.95, 10.15, 9.85];
        let mut v: Vec<f32> = cluster.iter().copied().chain([9.5, 50.0]).collect();
        let mut s = scratch();
        let remaining = GesdConfig::new(0.05, Some(3)).reject(&mut v, &mut s);
        assert_eq!(
            remaining, 10,
            "dark=9.5 too extreme, both rejected even with relax=1.5"
        );
    }

    #[test]
    fn test_gesd_relaxation_symmetry() {
        // With relaxation=1.0, equidistant dark/bright outliers must produce
        // identical survivor counts (no asymmetric bias).
        let config = GesdConfig::new(0.05, Some(2)).with_low_relaxation(1.0);

        let mut v_dark = vec![10.0, 10.1, 9.9, 10.0, 10.2, 9.8, 10.1, 0.0];
        let mut s1 = scratch();
        let r_dark = config.reject(&mut v_dark, &mut s1);

        let mut v_bright = vec![10.0, 10.1, 9.9, 10.0, 10.2, 9.8, 10.1, 20.0];
        let mut s2 = scratch();
        let r_bright = config.reject(&mut v_bright, &mut s2);

        assert_eq!(r_dark, r_bright, "relax=1.0 must be symmetric");
    }

    #[test]
    fn test_gesd_relaxation_does_not_affect_bright() {
        // Relaxation only applies to below-median pixels. A bright-only outlier
        // must be rejected identically regardless of relaxation value.
        let data: Vec<f32> = vec![
            10.0, 10.1, 9.9, 10.0, 10.2, 9.8, 10.05, 9.95, 10.15, 9.85, 50.0,
        ];
        for relax in [1.0, 1.5, 3.0, 5.0] {
            let mut v = data.clone();
            let mut s = scratch();
            let r = GesdConfig::new(0.05, Some(3))
                .with_low_relaxation(relax)
                .reject(&mut v, &mut s);
            assert_eq!(
                r, 10,
                "relax={}: bright outlier must always be rejected",
                relax
            );
            assert!(
                !s.indices[..r].contains(&10),
                "relax={}: idx 10 must be rejected",
                relax
            );
        }
    }

    #[test]
    fn test_gesd_combined_mean_with_relaxation() {
        // Verify final pixel value: cluster + dark=9.6 survives, bright=50.0 rejected.
        // Expected mean = (sum_of_cluster + 9.6) / 11
        let cluster = [
            10.0f32, 10.1, 9.9, 10.0, 10.2, 9.8, 10.05, 9.95, 10.15, 9.85,
        ];
        let expected_mean = (cluster.iter().sum::<f32>() + 9.6) / 11.0;

        let mut values: Vec<f32> = cluster.iter().copied().chain([9.6, 50.0]).collect();
        let mean = Rejection::Gesd(GesdConfig::new(0.05, Some(3))).combine_mean(
            &mut values,
            None,
            &mut scratch(),
        );
        assert!(
            (mean - expected_mean).abs() < 0.001,
            "mean should be {:.4}, got {:.4}",
            expected_mean,
            mean
        );
    }

    #[test]
    #[should_panic(expected = "Low relaxation must be >= 1.0")]
    fn test_gesd_relaxation_below_one_panics() {
        GesdConfig::new(0.05, None).with_low_relaxation(0.5);
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
        let mean = math::mean_f32(&values[..remaining]);
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
        let mean = weighted_mean_indexed(&values, &weights, &indices);
        assert!((mean - 2.5).abs() < 1e-6, "Expected 2.5, got {}", mean);
    }

    #[test]
    fn test_weighted_mean_indexed_reordered() {
        // Simulate rejection reordering: values were [10, 99, 20] → after rejecting idx 1,
        // survivors are values [10, 20] with indices [0, 2]
        let values = [10.0, 20.0];
        let weights = [5.0, 0.5, 1.0]; // original weights for 3 frames
        let indices = [0, 2]; // frame 0 and frame 2 survived
        let mean = weighted_mean_indexed(&values, &weights, &indices);
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

    // ========== Winsorized Correctness Tests ==========

    #[test]
    fn test_winsorized_robust_estimate_uses_stddev_not_mad() {
        // With known data, verify robust_estimate returns stddev-based sigma
        // (not MAD-based). For Gaussian data, stddev > MAD * 1.4826 is false,
        // but for uniform-like data they differ noticeably.
        let config = WinsorizedClipConfig::new(3.0);
        let values: Vec<f32> = (0..20).map(|i| 10.0 + i as f32 * 0.1).collect();
        let mut working = vec![];
        let mut scratch_buf = vec![];
        let (center, sigma) = config.robust_estimate(&values, &mut working, &mut scratch_buf);

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
        let mut scratch_buf = vec![];
        let (_center, sigma) = config.robust_estimate(&values, &mut working, &mut scratch_buf);

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
        let mut scratch_buf = vec![];
        let (center, sigma) = config.robust_estimate(&values, &mut working, &mut scratch_buf);

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
        let mut s1 = vec![];
        let mut w2 = vec![];
        let mut s2 = vec![];
        let (center1, sigma1) = config_permissive.robust_estimate(&values, &mut w1, &mut s1);
        let (center2, sigma2) = config_tight.robust_estimate(&values, &mut w2, &mut s2);

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

    // ========== Linear Fit Correctness Tests ==========

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

    // ========== Large-N Sort Tests (introsort path, N > 64) ==========

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

        sort_with_indices(&mut values, &mut indices, &mut scratch_buf, n);

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

        sort_with_indices(&mut values, &mut indices, &mut scratch_buf, n);

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
}
