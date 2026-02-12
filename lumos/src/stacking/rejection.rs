//! Pixel rejection algorithms for stacking.
//!
//! This module contains various outlier rejection methods used during image stacking:
//! - Sigma clipping (Kappa-Sigma)
//! - Winsorized sigma clipping
//! - Linear fit clipping
//! - Percentile clipping
//! - Generalized Extreme Studentized Deviate (GESD)

use crate::math::{self, mad_f32_with_scratch, mad_to_sigma};

/// Configuration for sigma clipping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SigmaClipConfig {
    /// Number of standard deviations for clipping threshold.
    pub sigma: f32,
    /// Maximum number of iterations for iterative clipping.
    pub max_iterations: u32,
}

impl Eq for SigmaClipConfig {}

impl Default for SigmaClipConfig {
    fn default() -> Self {
        Self {
            sigma: 2.5,
            max_iterations: 3,
        }
    }
}

impl SigmaClipConfig {
    pub fn new(sigma: f32, max_iterations: u32) -> Self {
        assert!(sigma > 0.0, "Sigma must be positive");
        assert!(max_iterations > 0, "Max iterations must be at least 1");
        Self {
            sigma,
            max_iterations,
        }
    }
}

/// Configuration for asymmetric sigma clipping.
///
/// Like standard sigma clipping but with separate thresholds for low and high outliers.
/// Useful when bright outliers (satellites, cosmic rays) need aggressive rejection
/// while faint outliers should be treated conservatively.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AsymmetricSigmaClipConfig {
    /// Sigma threshold for low outliers (below median).
    pub sigma_low: f32,
    /// Sigma threshold for high outliers (above median).
    pub sigma_high: f32,
    /// Maximum number of iterations for iterative clipping.
    pub max_iterations: u32,
}

impl Default for AsymmetricSigmaClipConfig {
    fn default() -> Self {
        Self {
            sigma_low: 4.0,
            sigma_high: 3.0,
            max_iterations: 3,
        }
    }
}

impl AsymmetricSigmaClipConfig {
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

impl Eq for WinsorizedClipConfig {}

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
}

// ============================================================================
// Rejection Algorithm Implementations
// ============================================================================

/// Result of a rejection operation.
#[derive(Debug, Clone, Copy)]
pub struct RejectionResult {
    /// The computed value after rejection.
    pub value: f32,
    /// Number of values remaining after rejection.
    pub remaining_count: usize,
}

impl SigmaClipConfig {
    /// Partition values by sigma clipping, returning the number of survivors.
    ///
    /// After return, `values[..remaining]` contains surviving values and
    /// `indices[..remaining]` contains their original frame indices.
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

            // Use median as center - robust to outliers
            let center = math::median_f32_mut(active);
            // Use MAD-based scale estimate (robust to outliers, unlike stddev)
            let mad = mad_f32_with_scratch(&values[..len], center, &mut scratch);
            let sigma = mad_to_sigma(mad);

            if sigma < f32::EPSILON {
                break;
            }

            let threshold = self.sigma * sigma;

            // Partition: move kept values and indices to front
            let mut write_idx = 0;
            for read_idx in 0..len {
                if (values[read_idx] - center).abs() <= threshold {
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

    /// Sigma-clipped mean: reject outliers then compute mean of survivors.
    pub fn clipped_mean(&self, values: &mut [f32], indices: &mut Vec<usize>) -> RejectionResult {
        let remaining = self.reject(values, indices);
        RejectionResult {
            value: math::mean_f32(&values[..remaining]),
            remaining_count: remaining,
        }
    }
}

impl AsymmetricSigmaClipConfig {
    /// Partition values by asymmetric sigma clipping, returning the number of survivors.
    ///
    /// After return, `values[..remaining]` contains surviving values and
    /// `indices[..remaining]` contains their original frame indices.
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

            // Partition: move kept values and indices to front
            let mut write_idx = 0;
            for read_idx in 0..len {
                let diff = values[read_idx] - center;
                let keep = if diff < 0.0 {
                    diff.abs() <= low_threshold
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

    /// Asymmetric sigma-clipped mean: reject outliers then compute mean of survivors.
    pub fn clipped_mean(&self, values: &mut [f32], indices: &mut Vec<usize>) -> RejectionResult {
        let remaining = self.reject(values, indices);
        RejectionResult {
            value: math::mean_f32(&values[..remaining]),
            remaining_count: remaining,
        }
    }
}

impl WinsorizedClipConfig {
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

impl LinearFitClipConfig {
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

            // Compute residuals and their std dev
            let mut sum_residual_sq = 0.0f32;
            for i in 0..len {
                let x = indices[i] as f32;
                let predicted = a + b * x;
                let residual = values[i] - predicted;
                sum_residual_sq += residual * residual;
            }

            let std_dev = (sum_residual_sq / n).sqrt();
            if std_dev < f32::EPSILON {
                break;
            }

            // Clip based on residuals from fit
            let mut write_idx = 0;
            for read_idx in 0..len {
                let x = indices[read_idx] as f32;
                let predicted = a + b * x;
                let residual = values[read_idx] - predicted;

                let keep = if residual < 0.0 {
                    residual.abs() <= self.sigma_low * std_dev
                } else {
                    residual <= self.sigma_high * std_dev
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

    /// Linear fit clipped mean: reject outliers then compute mean of survivors.
    pub fn clipped_mean(&self, values: &mut [f32], indices: &mut Vec<usize>) -> RejectionResult {
        let remaining = self.reject(values, indices);
        RejectionResult {
            value: math::mean_f32(&values[..remaining]),
            remaining_count: remaining,
        }
    }
}

impl PercentileClipConfig {
    /// Partition values by percentile clipping, returning the number of survivors.
    ///
    /// Sorts values and moves the surviving middle range to `values[..remaining]`.
    pub fn reject(&self, values: &mut [f32]) -> usize {
        debug_assert!(!values.is_empty());

        if values.len() <= 2 {
            return values.len();
        }

        let n = values.len();

        // Sort for percentile computation
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Calculate indices to keep
        let low_count = ((self.low_percentile / 100.0) * n as f32).floor() as usize;
        let high_count = ((self.high_percentile / 100.0) * n as f32).floor() as usize;

        let start = low_count;
        let end = n.saturating_sub(high_count);

        // Ensure we have at least one value
        let (start, end) = if start >= end {
            let mid = n / 2;
            (mid, mid + 1)
        } else {
            (start, end)
        };

        // Move survivors to front
        if start > 0 {
            values.copy_within(start..end, 0);
        }

        end - start
    }

    /// Percentile clipped mean: reject outliers then compute mean of survivors.
    pub fn clipped_mean(&self, values: &mut [f32]) -> RejectionResult {
        let remaining = self.reject(values);
        RejectionResult {
            value: math::mean_f32(&values[..remaining]),
            remaining_count: remaining,
        }
    }
}

impl GesdConfig {
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

    /// GESD clipped mean: reject outliers then compute mean of survivors.
    pub fn clipped_mean(&self, values: &mut [f32], indices: &mut Vec<usize>) -> RejectionResult {
        let remaining = self.reject(values, indices);
        RejectionResult {
            value: math::mean_f32(&values[..remaining]),
            remaining_count: remaining,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    // ========== SigmaClipConfig Tests ==========

    #[test]
    fn test_sigma_clip_config_default() {
        let config = SigmaClipConfig::default();
        assert!((config.sigma - 2.5).abs() < f32::EPSILON);
        assert_eq!(config.max_iterations, 3);
    }

    #[test]
    fn test_sigma_clip_config_new() {
        let config = SigmaClipConfig::new(3.0, 5);
        assert!((config.sigma - 3.0).abs() < f32::EPSILON);
        assert_eq!(config.max_iterations, 5);
    }

    #[test]
    #[should_panic(expected = "Sigma must be positive")]
    fn test_sigma_clip_config_zero_sigma() {
        SigmaClipConfig::new(0.0, 3);
    }

    // ========== WinsorizedClipConfig Tests ==========

    #[test]
    fn test_winsorized_config_default() {
        let config = WinsorizedClipConfig::default();
        assert!((config.sigma - 2.5).abs() < f32::EPSILON);
        assert_eq!(config.max_iterations, 3);
    }

    // ========== LinearFitClipConfig Tests ==========

    #[test]
    fn test_linear_fit_config_default() {
        let config = LinearFitClipConfig::default();
        assert!((config.sigma_low - 3.0).abs() < f32::EPSILON);
        assert!((config.sigma_high - 3.0).abs() < f32::EPSILON);
    }

    // ========== PercentileClipConfig Tests ==========

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

    // ========== GesdConfig Tests ==========

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

    /// Helper: create identity indices for a given length.
    fn make_indices(len: usize) -> Vec<usize> {
        (0..len).collect()
    }

    #[test]
    fn test_sigma_clipped_mean_removes_outlier() {
        // Use data with spread (non-zero MAD) plus a clear outlier
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let mut indices = make_indices(values.len());
        let config = SigmaClipConfig::new(2.0, 3);
        let result = config.clipped_mean(&mut values, &mut indices);
        assert!(
            result.value < 10.0,
            "Expected outlier to be clipped, got {}",
            result.value
        );
        assert!(result.remaining_count < 8);
    }

    #[test]
    fn test_sigma_clipped_mean_no_outliers() {
        let mut values = vec![1.0, 1.1, 1.2, 0.9, 1.0];
        let mut indices = make_indices(values.len());
        let config = SigmaClipConfig::new(3.0, 3);
        let result = config.clipped_mean(&mut values, &mut indices);
        assert_eq!(result.remaining_count, 5);
    }

    #[test]
    fn test_winsorize() {
        let values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let config = WinsorizedClipConfig::new(2.0, 3);
        let mut working = vec![];
        let mut scratch = vec![];
        let winsorized = config.winsorize(&values, &mut working, &mut scratch);
        let mean = math::mean_f32(winsorized);
        // Winsorized should have lower mean than with full outlier
        assert!(mean < 20.0, "Outlier should be winsorized, got {}", mean);
        // All values retained (just modified)
        assert_eq!(winsorized.len(), 8);
    }

    #[test]
    fn test_linear_fit_clipped_mean_constant_data() {
        let mut values = vec![5.0, 5.0, 5.0, 5.0, 5.0];
        let mut indices = make_indices(values.len());
        let config = LinearFitClipConfig::default();
        let result = config.clipped_mean(&mut values, &mut indices);
        assert!((result.value - 5.0).abs() < 0.01);
        assert_eq!(result.remaining_count, 5);
    }

    #[test]
    fn test_linear_fit_clipped_mean_linear_trend() {
        // Linear trend with one outlier
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 100.0, 6.0];
        let mut indices = make_indices(values.len());
        let config = LinearFitClipConfig::new(2.0, 2.0, 3);
        let result = config.clipped_mean(&mut values, &mut indices);
        // Should reject the 100.0 outlier
        assert!(result.remaining_count < 6);
        assert!(result.value < 20.0);
    }

    #[test]
    fn test_percentile_clipped_mean() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let config = PercentileClipConfig::new(20.0, 20.0);
        let result = config.clipped_mean(&mut values);
        // Should clip 2 from each end (20% of 10)
        assert_eq!(result.remaining_count, 6);
        // Mean of [3, 4, 5, 6, 7, 8] = 5.5
        assert!((result.value - 5.5).abs() < 0.01);
    }

    #[test]
    fn test_gesd_mean_removes_outliers() {
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 100.0];
        let mut indices = make_indices(values.len());
        let config = GesdConfig::new(0.05, Some(3));
        let result = config.clipped_mean(&mut values, &mut indices);
        // Should detect and remove the 100.0 outlier
        assert!(result.remaining_count < 8);
        assert!(result.value < 10.0);
    }

    #[test]
    fn test_gesd_mean_no_outliers() {
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 1.1];
        let mut indices = make_indices(values.len());
        let config = GesdConfig::new(0.05, Some(3));
        let result = config.clipped_mean(&mut values, &mut indices);
        // Clean data should have few or no rejections
        assert!(result.remaining_count >= 7);
    }

    #[test]
    fn test_inverse_normal_approx() {
        // Test known values
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
        // All algorithms should handle small samples gracefully
        let values = vec![1.0, 2.0];

        let sigma_result = SigmaClipConfig::default()
            .clipped_mean(&mut values.clone(), &mut make_indices(values.len()));
        assert_eq!(sigma_result.remaining_count, 2);

        let mut working = vec![];
        let mut scratch = vec![];
        let winsorized =
            WinsorizedClipConfig::default().winsorize(&values, &mut working, &mut scratch);
        assert_eq!(winsorized.len(), 2);

        let linear_result = LinearFitClipConfig::default()
            .clipped_mean(&mut values.clone(), &mut make_indices(values.len()));
        assert_eq!(linear_result.remaining_count, 2);

        let percentile_result = PercentileClipConfig::default().clipped_mean(&mut values.clone());
        assert!(percentile_result.remaining_count >= 1);

        let asymmetric_result = AsymmetricSigmaClipConfig::default()
            .clipped_mean(&mut values.clone(), &mut make_indices(values.len()));
        assert_eq!(asymmetric_result.remaining_count, 2);

        let gesd_result = GesdConfig::default()
            .clipped_mean(&mut values.clone(), &mut make_indices(values.len()));
        assert_eq!(gesd_result.remaining_count, 2);
    }

    // ========== AsymmetricSigmaClipConfig Tests ==========

    #[test]
    fn test_asymmetric_sigma_clip_config_default() {
        let config = AsymmetricSigmaClipConfig::default();
        assert!((config.sigma_low - 4.0).abs() < f32::EPSILON);
        assert!((config.sigma_high - 3.0).abs() < f32::EPSILON);
        assert_eq!(config.max_iterations, 3);
    }

    #[test]
    fn test_asymmetric_sigma_clip_config_new() {
        let config = AsymmetricSigmaClipConfig::new(2.0, 3.0, 5);
        assert!((config.sigma_low - 2.0).abs() < f32::EPSILON);
        assert!((config.sigma_high - 3.0).abs() < f32::EPSILON);
        assert_eq!(config.max_iterations, 5);
    }

    #[test]
    #[should_panic(expected = "Sigma low must be positive")]
    fn test_asymmetric_sigma_clip_config_zero_sigma_low() {
        AsymmetricSigmaClipConfig::new(0.0, 3.0, 3);
    }

    #[test]
    #[should_panic(expected = "Sigma high must be positive")]
    fn test_asymmetric_sigma_clip_config_zero_sigma_high() {
        AsymmetricSigmaClipConfig::new(3.0, 0.0, 3);
    }

    // ========== Asymmetric Sigma Clip Algorithm Tests ==========

    #[test]
    fn test_asymmetric_sigma_clip_removes_high_outlier() {
        // Use data with spread (non-zero MAD) plus a high outlier
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let mut indices = make_indices(values.len());
        let config = AsymmetricSigmaClipConfig::new(4.0, 2.0, 3);
        let result = config.clipped_mean(&mut values, &mut indices);
        assert!(
            result.value < 10.0,
            "High outlier should be clipped, got {}",
            result.value
        );
        assert!(result.remaining_count < 8);
    }

    #[test]
    fn test_asymmetric_sigma_clip_keeps_low_with_high_threshold() {
        // Data with spread, plus both a low and high outlier.
        // Use very conservative sigma_low (10.0) and aggressive sigma_high (2.0).
        // The high outlier should be rejected, but the low outlier should be kept.
        let mut values = vec![-5.0, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 50.0];
        let mut indices = make_indices(values.len());
        let config = AsymmetricSigmaClipConfig::new(10.0, 2.0, 5);
        let result = config.clipped_mean(&mut values, &mut indices);

        // The high outlier (50.0) should be removed
        // The low outlier (-5.0) should be kept due to high sigma_low
        assert!(
            result.remaining_count >= 9,
            "Low outlier should be kept, remaining={}",
            result.remaining_count
        );
        // Mean pulled down by -5.0 compared to median ~2.5
        assert!(
            result.value < 2.5,
            "Mean should be < 2.5 due to kept low outlier, got {}",
            result.value
        );
    }

    #[test]
    fn test_asymmetric_sigma_clip_no_outliers() {
        let mut values = vec![1.0, 1.1, 1.2, 0.9, 1.0];
        let mut indices = make_indices(values.len());
        let config = AsymmetricSigmaClipConfig::new(3.0, 3.0, 3);
        let result = config.clipped_mean(&mut values, &mut indices);
        assert_eq!(result.remaining_count, 5);
    }

    #[test]
    fn test_asymmetric_vs_symmetric_sigma_clip() {
        // With equal thresholds, asymmetric should give the same result as symmetric
        let values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let sigma = 2.5;

        let sym_result = SigmaClipConfig::new(sigma, 3)
            .clipped_mean(&mut values.clone(), &mut make_indices(values.len()));
        let asym_result = AsymmetricSigmaClipConfig::new(sigma, sigma, 3)
            .clipped_mean(&mut values.clone(), &mut make_indices(values.len()));

        assert_eq!(sym_result.remaining_count, asym_result.remaining_count);
        assert!(
            (sym_result.value - asym_result.value).abs() < 1e-6,
            "Symmetric and asymmetric with equal thresholds should match: {} vs {}",
            sym_result.value,
            asym_result.value,
        );
    }

    #[test]
    fn test_asymmetric_clip_differs_from_linear_fit() {
        // Linear trend data: linear fit uses a fitted line as center,
        // asymmetric sigma uses the median as center.
        // For data with a clear trend, they give different results.
        let values = vec![1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0, 17.0, 19.0];

        let asym_result = AsymmetricSigmaClipConfig::new(1.0, 1.0, 3)
            .clipped_mean(&mut values.clone(), &mut make_indices(values.len()));
        let linear_result = LinearFitClipConfig::new(1.0, 1.0, 3)
            .clipped_mean(&mut values.clone(), &mut make_indices(values.len()));

        // Linear fit follows the trend perfectly, so no rejections
        assert_eq!(
            linear_result.remaining_count, 10,
            "Linear fit should keep all points on a perfect line"
        );

        // Asymmetric sigma clips from median (10.0), so values far from
        // median get rejected with tight sigma=1.0
        // MAD=2.0, sigma_est=2.965, threshold=2.965 -> keep [7.035, 12.965]
        assert!(
            asym_result.remaining_count < 10,
            "Asymmetric clip from median should reject points far from center"
        );
    }

    // ========== Index Tracking Tests ==========

    #[test]
    fn test_sigma_clipped_indices_track_survivors() {
        // Frame 0=1.0, Frame 1=1.5, ..., Frame 7=100.0 (outlier)
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let mut indices = make_indices(values.len());
        let config = SigmaClipConfig::new(2.0, 3);
        let result = config.clipped_mean(&mut values, &mut indices);

        // Outlier (frame 7) should be rejected
        let surviving = &indices[..result.remaining_count];
        assert!(
            !surviving.contains(&7),
            "Frame 7 (outlier) should not survive, survivors: {:?}",
            surviving
        );
        // All surviving indices should be valid frame indices
        for &idx in surviving {
            assert!(idx < 8, "Invalid surviving index: {}", idx);
        }
    }

    #[test]
    fn test_gesd_indices_track_survivors() {
        // Frame 7 = outlier
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 100.0];
        let mut indices = make_indices(values.len());
        let config = GesdConfig::new(0.05, Some(3));
        let result = config.clipped_mean(&mut values, &mut indices);

        let surviving = &indices[..result.remaining_count];
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
        // Linear trend with outlier at frame 4
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 100.0, 6.0];
        let mut indices = make_indices(values.len());
        let config = LinearFitClipConfig::new(2.0, 2.0, 3);
        let result = config.clipped_mean(&mut values, &mut indices);

        let surviving = &indices[..result.remaining_count];
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
    fn test_no_rejection_preserves_all_indices() {
        let mut values = vec![1.0, 1.1, 1.2, 0.9, 1.0];
        let mut indices = make_indices(values.len());
        let config = SigmaClipConfig::new(3.0, 3);
        let result = config.clipped_mean(&mut values, &mut indices);

        assert_eq!(result.remaining_count, 5);
        // All original indices should be present
        let surviving = &indices[..result.remaining_count];
        for i in 0..5 {
            assert!(
                surviving.contains(&i),
                "Index {} should survive when no rejection occurs",
                i
            );
        }
    }
}
