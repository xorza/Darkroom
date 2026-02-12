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
    pub fn reject(&self, values: &mut [f32], indices: &mut Vec<usize>) -> usize {
        debug_assert!(!values.is_empty());

        reset_indices(indices, values.len());

        if values.len() <= 2 {
            return values.len();
        }

        let mut len = values.len();
        let mut scratch = Vec::with_capacity(len);

        for _ in 0..self.max_iterations {
            if len <= 2 {
                break;
            }

            let active = &mut values[..len];

            let center = math::median_f32_mut(active);
            let mad = mad_f32_with_scratch(&values[..len], center, &mut scratch);
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
                    indices[write_idx] = indices[read_idx];
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
/// Unlike standard sigma clipping which removes outliers,
/// winsorized clipping replaces them with the boundary value (mean ± sigma*stddev).
/// This is more robust for small sample sizes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WinsorizedClipConfig {
    /// Number of standard deviations for clipping threshold.
    pub sigma: f32,
    /// Maximum number of iterations for iterative clipping.
    pub max_iterations: u32,
}

impl Default for WinsorizedClipConfig {
    fn default() -> Self {
        Self {
            sigma: 2.5,
            max_iterations: 3,
        }
    }
}

impl WinsorizedClipConfig {
    pub fn new(sigma: f32, max_iterations: u32) -> Self {
        assert!(sigma > 0.0, "Sigma must be positive");
        assert!(max_iterations > 0, "Max iterations must be at least 1");
        Self {
            sigma,
            max_iterations,
        }
    }

    /// Apply winsorization: replace outliers with boundary values.
    ///
    /// Iteratively computes median and std dev, then clamps values to
    /// `[median - sigma*stddev, median + sigma*stddev]`.
    ///
    /// Returns `working` filled with the winsorized copy. Does NOT modify `values`.
    /// `scratch` is used for median/MAD computation.
    pub fn winsorize<'a>(
        &self,
        values: &[f32],
        working: &'a mut Vec<f32>,
        scratch: &mut Vec<f32>,
    ) -> &'a [f32] {
        debug_assert!(!values.is_empty());

        working.clear();
        working.extend_from_slice(values);

        if values.len() <= 2 {
            return working;
        }

        for _ in 0..self.max_iterations {
            // Copy into scratch buffer for median (which sorts in-place)
            scratch.clear();
            scratch.extend_from_slice(working);
            let center = math::median_f32_mut(scratch);
            let mad = mad_f32_with_scratch(working, center, scratch);
            let sigma = mad_to_sigma(mad);

            if sigma < f32::EPSILON {
                break;
            }

            let low_bound = center - self.sigma * sigma;
            let high_bound = center + self.sigma * sigma;

            let mut changed = false;
            for v in working.iter_mut() {
                if *v < low_bound {
                    *v = low_bound;
                    changed = true;
                } else if *v > high_bound {
                    *v = high_bound;
                    changed = true;
                }
            }

            if !changed {
                break;
            }
        }

        working
    }
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
    /// After return, `values[..remaining]` contains surviving values and
    /// `indices[..remaining]` contains their original frame indices.
    pub fn reject(&self, values: &mut [f32], indices: &mut Vec<usize>) -> usize {
        debug_assert!(!values.is_empty());

        reset_indices(indices, values.len());

        if values.len() <= 3 {
            return values.len();
        }

        let mut len = values.len();

        for _ in 0..self.max_iterations {
            if len <= 3 {
                break;
            }

            // Fit line y = a + b*x using least squares
            // x values are the original indices
            let n = len as f32;
            let mut sum_x = 0.0f32;
            let mut sum_y = 0.0f32;
            let mut sum_xy = 0.0f32;
            let mut sum_xx = 0.0f32;

            for i in 0..len {
                let x = indices[i] as f32;
                let y = values[i];
                sum_x += x;
                sum_y += y;
                sum_xy += x * y;
                sum_xx += x * x;
            }

            let denom = n * sum_xx - sum_x * sum_x;
            if denom.abs() < f32::EPSILON {
                break;
            }

            let b = (n * sum_xy - sum_x * sum_y) / denom;
            let a = (sum_y - b * sum_x) / n;

            // Compute std dev of residuals
            let mut sum_residual_sq = 0.0f32;
            for i in 0..len {
                let predicted = a + b * indices[i] as f32;
                let residual = values[i] - predicted;
                sum_residual_sq += residual * residual;
            }

            let std_dev = (sum_residual_sq / n).sqrt();
            if std_dev < f32::EPSILON {
                break;
            }

            // Clip based on residuals from fit
            let low_threshold = self.sigma_low * std_dev;
            let high_threshold = self.sigma_high * std_dev;

            let mut write_idx = 0;
            for read_idx in 0..len {
                let predicted = a + b * indices[read_idx] as f32;
                let residual = values[read_idx] - predicted;

                let keep = if residual < 0.0 {
                    -residual <= low_threshold
                } else {
                    residual <= high_threshold
                };

                if keep {
                    values[write_idx] = values[read_idx];
                    indices[write_idx] = indices[read_idx];
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
    pub fn reject(&self, values: &mut [f32], indices: &mut Vec<usize>) -> usize {
        debug_assert!(!values.is_empty());

        let n = values.len();
        reset_indices(indices, n);

        if n <= 2 {
            return n;
        }

        // Insertion sort with index co-array — optimal for small pixel stacks (< 50 frames)
        for i in 1..n {
            let mut j = i;
            while j > 0 && values[j - 1] > values[j] {
                values.swap(j - 1, j);
                indices.swap(j - 1, j);
                j -= 1;
            }
        }

        let range = self.surviving_range(n);
        let count = range.len();

        if range.start > 0 {
            values.copy_within(range.clone(), 0);
            indices.copy_within(range, 0);
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
        assert!((0.0..1.0).contains(&alpha), "Alpha must be between 0 and 1");
        Self {
            alpha,
            max_outliers,
        }
    }

    /// Get maximum outliers, defaulting to 25% of data size.
    pub fn max_outliers_for_size(&self, n: usize) -> usize {
        self.max_outliers.unwrap_or(n / 4)
    }

    /// Partition values by GESD test, returning the number of survivors.
    ///
    /// After return, `values[..remaining]` contains surviving values and
    /// `indices[..remaining]` contains their original frame indices.
    pub fn reject(&self, values: &mut [f32], indices: &mut Vec<usize>) -> usize {
        debug_assert!(!values.is_empty());

        reset_indices(indices, values.len());

        let original_len = values.len();

        if values.len() <= 3 {
            return values.len();
        }

        let max_outliers = self
            .max_outliers_for_size(original_len)
            .min(original_len - 3);
        let mut len = original_len;

        for i in 0..max_outliers {
            if len <= 3 {
                break;
            }

            let active = &values[..len];
            let mean = math::mean_f32(active);
            let variance = math::sum_squared_diff(active, mean) / len as f32;
            let std_dev = variance.sqrt();

            if std_dev < f32::EPSILON {
                break;
            }

            // Find the most extreme value
            let mut max_deviation = 0.0f32;
            let mut max_idx = 0;
            for (idx, &v) in active.iter().enumerate() {
                let deviation = (v - mean).abs();
                if deviation > max_deviation {
                    max_deviation = deviation;
                    max_idx = idx;
                }
            }

            // Compute test statistic R_i
            let r_i = max_deviation / std_dev;

            // Compute critical value using approximation
            // For large n, critical value ≈ t_{p,n-i-1} * (n-i-1) / sqrt((n-i-1+t²)(n-i))
            let n = len as f32;
            let p = 1.0 - self.alpha / (2.0 * (n - i as f32));

            // Approximation for t-distribution critical value
            // Using simple approximation: t ≈ inverse_normal(p) for large df
            let t_crit = inverse_normal_approx(p);

            let numerator = (n - i as f32 - 1.0) * t_crit;
            let denominator = ((n - i as f32) * (n - i as f32 - 2.0 + t_crit * t_crit)).sqrt();
            let critical = numerator / denominator;

            // If test statistic exceeds critical value, remove the outlier
            if r_i > critical {
                // Remove by swapping with last element
                values.swap(max_idx, len - 1);
                indices.swap(max_idx, len - 1);
                len -= 1;
            } else {
                // No more outliers found
                break;
            }
        }

        len
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
        Self::Winsorized(WinsorizedClipConfig::new(sigma, 3))
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
            Rejection::None | Rejection::Winsorized(_) => values.len(),
            Rejection::SigmaClip(c) => c.reject(values, &mut scratch.indices),
            Rejection::LinearFit(c) => c.reject(values, &mut scratch.indices),
            Rejection::Gesd(c) => c.reject(values, &mut scratch.indices),
            Rejection::Percentile(c) => c.reject(values, &mut scratch.indices),
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
        // None and Winsorized don't reorder values, so weights align directly
        match self {
            Rejection::None => {
                return match weights {
                    Some(w) => math::weighted_mean_f32(values, w),
                    None => math::mean_f32(values),
                };
            }
            Rejection::Winsorized(config) => {
                let winsorized =
                    config.winsorize(values, &mut scratch.floats_a, &mut scratch.floats_b);
                return match weights {
                    Some(w) => math::weighted_mean_f32(winsorized, w),
                    None => math::mean_f32(winsorized),
                };
            }
            _ => {}
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
fn weighted_mean_indexed(values: &[f32], weights: &[f32], indices: &[usize]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }

    let mut sum = 0.0f32;
    let mut weight_sum = 0.0f32;

    for (i, &v) in values.iter().enumerate() {
        let w = weights[indices[i]];
        sum += v * w;
        weight_sum += w;
    }

    if weight_sum > f32::EPSILON {
        sum / weight_sum
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
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
        assert!((config.sigma - 2.5).abs() < f32::EPSILON);
        assert_eq!(config.max_iterations, 3);
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
        let mut indices = vec![];
        let remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values, &mut indices);
        let mean = math::mean_f32(&values[..remaining]);
        assert!(mean < 10.0, "Expected outlier to be clipped, got {}", mean);
        assert!(remaining < 8);
    }

    #[test]
    fn test_sigma_clip_no_outliers() {
        let mut values = vec![1.0, 1.1, 1.2, 0.9, 1.0];
        let mut indices = vec![];
        let remaining = SigmaClipConfig::new(3.0, 3).reject(&mut values, &mut indices);
        assert_eq!(remaining, 5);
    }

    #[test]
    fn test_asymmetric_sigma_clip_removes_high_outlier() {
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let mut indices = vec![];
        let remaining =
            SigmaClipConfig::new_asymmetric(4.0, 2.0, 3).reject(&mut values, &mut indices);
        let mean = math::mean_f32(&values[..remaining]);
        assert!(mean < 10.0, "High outlier should be clipped, got {}", mean);
        assert!(remaining < 8);
    }

    #[test]
    fn test_asymmetric_sigma_clip_keeps_low_with_high_threshold() {
        // Conservative sigma_low (10.0) + aggressive sigma_high (2.0):
        // high outlier rejected, low outlier kept.
        let mut values = vec![-5.0, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 50.0];
        let mut indices = vec![];
        let remaining =
            SigmaClipConfig::new_asymmetric(10.0, 2.0, 5).reject(&mut values, &mut indices);

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
        let mut idx1 = vec![];
        let r1 = SigmaClipConfig::new(sigma, 3).reject(&mut v1, &mut idx1);

        let mut v2 = values;
        let mut idx2 = vec![];
        let r2 = SigmaClipConfig::new_asymmetric(sigma, sigma, 3).reject(&mut v2, &mut idx2);

        assert_eq!(r1, r2);
        assert!((math::mean_f32(&v1[..r1]) - math::mean_f32(&v2[..r2])).abs() < 1e-6,);
    }

    #[test]
    fn test_sigma_clip_differs_from_linear_fit() {
        // Linear trend: linear fit follows the trend, sigma clip uses median.
        let mut values_sigma = vec![1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0, 17.0, 19.0];
        let mut values_linear = values_sigma.clone();
        let mut indices = vec![];

        let sigma_remaining = SigmaClipConfig::new(1.0, 3).reject(&mut values_sigma, &mut indices);
        let linear_remaining =
            LinearFitClipConfig::new(1.0, 1.0, 3).reject(&mut values_linear, &mut indices);

        // Linear fit follows the trend perfectly, no rejections
        assert_eq!(
            linear_remaining, 10,
            "Linear fit should keep all points on a perfect line"
        );
        // Sigma clips from median, rejects points far from center
        assert!(
            sigma_remaining < 10,
            "Sigma clip from median should reject points far from center"
        );
    }

    #[test]
    fn test_winsorize() {
        let values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let mut working = vec![];
        let mut scratch = vec![];
        let winsorized =
            WinsorizedClipConfig::new(2.0, 3).winsorize(&values, &mut working, &mut scratch);
        let mean = math::mean_f32(winsorized);
        assert!(mean < 20.0, "Outlier should be winsorized, got {}", mean);
        assert_eq!(winsorized.len(), 8);
    }

    #[test]
    fn test_linear_fit_constant_data() {
        let mut values = vec![5.0, 5.0, 5.0, 5.0, 5.0];
        let mut indices = vec![];
        let remaining = LinearFitClipConfig::default().reject(&mut values, &mut indices);
        assert_eq!(remaining, 5);
        assert!((math::mean_f32(&values[..remaining]) - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_linear_fit_rejects_outlier() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 100.0, 6.0];
        let mut indices = vec![];
        let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut indices);
        assert!(remaining < 6);
        assert!(math::mean_f32(&values[..remaining]) < 20.0);
    }

    #[test]
    fn test_percentile_clip() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let mut indices = vec![];
        let remaining = PercentileClipConfig::new(20.0, 20.0).reject(&mut values, &mut indices);
        assert_eq!(remaining, 6);
        // Mean of [3, 4, 5, 6, 7, 8] = 5.5
        assert!((math::mean_f32(&values[..remaining]) - 5.5).abs() < 0.01);
    }

    #[test]
    fn test_gesd_removes_outliers() {
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 100.0];
        let mut indices = vec![];
        let remaining = GesdConfig::new(0.05, Some(3)).reject(&mut values, &mut indices);
        assert!(remaining < 8);
        assert!(math::mean_f32(&values[..remaining]) < 10.0);
    }

    #[test]
    fn test_gesd_no_outliers() {
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 1.1];
        let mut indices = vec![];
        let remaining = GesdConfig::new(0.05, Some(3)).reject(&mut values, &mut indices);
        assert!(remaining >= 7);
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
        let mut indices = vec![];

        let r = SigmaClipConfig::default().reject(&mut [1.0, 2.0], &mut indices);
        assert_eq!(r, 2);

        let r = SigmaClipConfig::new_asymmetric(4.0, 3.0, 3).reject(&mut [1.0, 2.0], &mut indices);
        assert_eq!(r, 2);

        let mut working = vec![];
        let mut scratch = vec![];
        let w = WinsorizedClipConfig::default().winsorize(&[1.0, 2.0], &mut working, &mut scratch);
        assert_eq!(w.len(), 2);

        let r = LinearFitClipConfig::default().reject(&mut [1.0, 2.0], &mut indices);
        assert_eq!(r, 2);

        let r = PercentileClipConfig::default().reject(&mut [1.0, 2.0], &mut indices);
        assert!(r >= 1);

        let r = GesdConfig::default().reject(&mut [1.0, 2.0], &mut indices);
        assert_eq!(r, 2);
    }

    // ========== Index Tracking Tests ==========

    #[test]
    fn test_sigma_clip_indices_track_survivors() {
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let mut indices = vec![];
        let remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values, &mut indices);

        let surviving = &indices[..remaining];
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
        let mut indices = vec![];
        let remaining = GesdConfig::new(0.05, Some(3)).reject(&mut values, &mut indices);

        let surviving = &indices[..remaining];
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
        let mut indices = vec![];
        let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut indices);

        let surviving = &indices[..remaining];
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
        let mut indices = vec![];
        let remaining = PercentileClipConfig::new(20.0, 20.0).reject(&mut values, &mut indices);

        assert_eq!(remaining, 3);
        let surviving = &indices[..remaining];
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
        let mut indices = vec![];
        let remaining = SigmaClipConfig::new(3.0, 3).reject(&mut values, &mut indices);

        assert_eq!(remaining, 5);
        let surviving = &indices[..remaining];
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
        assert!(matches!(r, Rejection::Winsorized(c) if (c.sigma - 3.0).abs() < f32::EPSILON));

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
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 100.0, 6.0];
        let weights = vec![10.0, 0.1, 0.1, 0.1, 0.1, 0.1];

        let mean =
            Rejection::linear_fit(2.0).combine_mean(&mut values, Some(&weights), &mut scratch());

        assert!(
            mean < 2.0,
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
        let mut indices = vec![];
        let remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values, &mut indices);
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
    fn test_winsorize_no_outliers() {
        let values = vec![2.0, 2.1, 2.2, 1.9, 2.0];
        let mut working = vec![];
        let mut scratch_buf = vec![];
        let winsorized =
            WinsorizedClipConfig::new(3.0, 3).winsorize(&values, &mut working, &mut scratch_buf);
        assert_eq!(winsorized.len(), 5);
        // Values should pass through unchanged
        for (orig, &w) in values.iter().zip(winsorized.iter()) {
            assert!(
                (orig - w).abs() < f32::EPSILON,
                "Value {} should be unchanged, got {}",
                orig,
                w
            );
        }
    }

    #[test]
    fn test_winsorize_does_not_modify_original() {
        let values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let original = values.clone();
        let mut working = vec![];
        let mut scratch_buf = vec![];
        WinsorizedClipConfig::new(2.0, 3).winsorize(&values, &mut working, &mut scratch_buf);
        assert_eq!(values, original, "Original values should not be modified");
    }

    #[test]
    fn test_linear_fit_with_trend_plus_outlier() {
        // Perfect linear trend y = 1 + 2*x, with one outlier
        let mut values = vec![1.0, 3.0, 5.0, 7.0, 50.0, 11.0, 13.0, 15.0];
        let mut indices = vec![];
        let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut indices);
        // The outlier at index 4 (value 50.0, expected 9.0) should be rejected
        assert_eq!(remaining, 7, "Only the outlier should be rejected");
        let surviving = &indices[..remaining];
        assert!(
            !surviving.contains(&4),
            "Frame 4 (outlier) should not survive, survivors: {:?}",
            surviving
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
    fn test_weighted_mean_indexed_zero_weights() {
        // When all weights are zero, should fall back to simple mean
        let values = [2.0, 4.0, 6.0];
        let weights = [0.0, 0.0, 0.0];
        let indices = [0, 1, 2];
        let mean = weighted_mean_indexed(&values, &weights, &indices);
        assert!(
            (mean - 4.0).abs() < f32::EPSILON,
            "Zero-weight fallback should give simple mean 4.0, got {}",
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
}
