//! Pixel rejection algorithms for stacking.
//!
//! This module contains various outlier rejection methods used during image stacking:
//! - Sigma clipping (Kappa-Sigma)
//! - Winsorized sigma clipping
//! - Linear fit clipping
//! - Percentile clipping
//! - Generalized Extreme Studentized Deviate (GESD)

use crate::math;

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

/// Sigma-clipped mean: iteratively remove outliers beyond sigma threshold.
///
/// Algorithm:
/// 1. Use median as center (robust to outliers)
/// 2. Compute std dev from median
/// 3. Clip values beyond sigma threshold from median
/// 4. Return mean of remaining values
///
/// Modifies the input slice in place.
pub fn sigma_clipped_mean(values: &mut [f32], config: &SigmaClipConfig) -> RejectionResult {
    debug_assert!(!values.is_empty());

    if values.len() <= 2 {
        return RejectionResult {
            value: math::mean_f32(values),
            remaining_count: values.len(),
        };
    }

    let mut len = values.len();

    for _ in 0..config.max_iterations {
        if len <= 2 {
            break;
        }

        let active = &mut values[..len];

        // Use median as center - robust to outliers
        let center = math::median_f32_mut(active);
        let variance = math::sum_squared_diff(active, center) / len as f32;
        let std_dev = variance.sqrt();

        if std_dev < f32::EPSILON {
            break;
        }

        let threshold = config.sigma * std_dev;

        // Partition: move kept values to front
        let mut write_idx = 0;
        for read_idx in 0..len {
            if (values[read_idx] - center).abs() <= threshold {
                values[write_idx] = values[read_idx];
                write_idx += 1;
            }
        }

        if write_idx == len {
            break;
        }
        len = write_idx;
    }

    RejectionResult {
        value: math::mean_f32(&values[..len]),
        remaining_count: len,
    }
}

/// Winsorized sigma-clipped mean: replace outliers with boundary values.
///
/// Unlike standard sigma clipping which removes outliers, this replaces them
/// with the clipping boundary value. More robust for small sample sizes.
///
/// Does NOT modify the input slice.
pub fn winsorized_sigma_clipped_mean(
    values: &[f32],
    config: &WinsorizedClipConfig,
) -> RejectionResult {
    debug_assert!(!values.is_empty());

    if values.len() <= 2 {
        return RejectionResult {
            value: math::mean_f32(values),
            remaining_count: values.len(),
        };
    }

    // Work with a copy for iterative winsorization
    let mut working = values.to_vec();

    for _ in 0..config.max_iterations {
        // Compute median and std dev
        let center = math::median_f32_mut(&mut working.clone());
        let variance = math::sum_squared_diff(&working, center) / working.len() as f32;
        let std_dev = variance.sqrt();

        if std_dev < f32::EPSILON {
            break;
        }

        let low_bound = center - config.sigma * std_dev;
        let high_bound = center + config.sigma * std_dev;

        // Replace outliers with boundary values
        let mut winsorized_this_iter = 0;
        for v in &mut working {
            if *v < low_bound {
                *v = low_bound;
                winsorized_this_iter += 1;
            } else if *v > high_bound {
                *v = high_bound;
                winsorized_this_iter += 1;
            }
        }

        if winsorized_this_iter == 0 {
            break;
        }
    }

    RejectionResult {
        value: math::mean_f32(&working),
        remaining_count: values.len(),
    }
}

/// Linear fit clipping: reject pixels based on deviation from linear fit.
///
/// This method is particularly effective for data with sky gradients where
/// pixel values may have a linear relationship with some reference.
/// Here we use the pixel index as the reference (assumes temporal ordering).
///
/// Modifies the input slice in place.
pub fn linear_fit_clipped_mean(
    values: &mut [f32],
    config: &LinearFitClipConfig,
) -> RejectionResult {
    debug_assert!(!values.is_empty());

    if values.len() <= 3 {
        return RejectionResult {
            value: math::mean_f32(values),
            remaining_count: values.len(),
        };
    }

    let mut len = values.len();
    // Track which original indices are still active
    let mut indices: Vec<usize> = (0..len).collect();

    for _ in 0..config.max_iterations {
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
                residual.abs() <= config.sigma_low * std_dev
            } else {
                residual <= config.sigma_high * std_dev
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

    RejectionResult {
        value: math::mean_f32(&values[..len]),
        remaining_count: len,
    }
}

/// Percentile clipping: reject lowest and highest percentiles.
///
/// Simple and effective for small stacks. Requires sorting.
///
/// Modifies the input slice in place (sorts it).
pub fn percentile_clipped_mean(
    values: &mut [f32],
    config: &PercentileClipConfig,
) -> RejectionResult {
    debug_assert!(!values.is_empty());

    let original_len = values.len();

    if values.len() <= 2 {
        return RejectionResult {
            value: math::mean_f32(values),
            remaining_count: values.len(),
        };
    }

    // Sort for percentile computation
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Calculate indices to keep
    let low_count = ((config.low_percentile / 100.0) * original_len as f32).floor() as usize;
    let high_count = ((config.high_percentile / 100.0) * original_len as f32).floor() as usize;

    let start = low_count;
    let end = original_len.saturating_sub(high_count);

    // Ensure we have at least one value
    let (start, end) = if start >= end {
        let mid = original_len / 2;
        (mid, mid + 1)
    } else {
        (start, end)
    };

    let remaining = &values[start..end];
    let remaining_count = remaining.len();

    RejectionResult {
        value: math::mean_f32(remaining),
        remaining_count,
    }
}

/// GESD (Generalized Extreme Studentized Deviate) test for outlier detection.
///
/// A rigorous statistical test that can identify multiple outliers.
/// Best for large datasets (> 50 frames).
///
/// Algorithm:
/// 1. Compute test statistic R_i for most extreme value
/// 2. Compare to critical value based on t-distribution
/// 3. If R_i > critical, mark as outlier and repeat
/// 4. Return mean of non-outliers
///
/// Modifies the input slice in place.
pub fn gesd_mean(values: &mut [f32], config: &GesdConfig) -> RejectionResult {
    debug_assert!(!values.is_empty());

    let original_len = values.len();

    if values.len() <= 3 {
        return RejectionResult {
            value: math::mean_f32(values),
            remaining_count: values.len(),
        };
    }

    let max_outliers = config
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
        let p = 1.0 - config.alpha / (2.0 * (n - i as f32));

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
            len -= 1;
        } else {
            // No more outliers found
            break;
        }
    }

    RejectionResult {
        value: math::mean_f32(&values[..len]),
        remaining_count: len,
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

    #[test]
    fn test_sigma_clipped_mean_removes_outlier() {
        let mut values = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0];
        let config = SigmaClipConfig::new(2.0, 3);
        let result = sigma_clipped_mean(&mut values, &config);
        assert!(result.value < 10.0, "Expected outlier to be clipped");
        assert!(result.remaining_count < 6);
    }

    #[test]
    fn test_sigma_clipped_mean_no_outliers() {
        let mut values = vec![1.0, 1.1, 1.2, 0.9, 1.0];
        let config = SigmaClipConfig::new(3.0, 3);
        let result = sigma_clipped_mean(&mut values, &config);
        assert_eq!(result.remaining_count, 5);
    }

    #[test]
    fn test_winsorized_sigma_clipped_mean() {
        let values = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0];
        let config = WinsorizedClipConfig::new(2.0, 3);
        let result = winsorized_sigma_clipped_mean(&values, &config);
        // Winsorized should have lower mean than with full outlier
        assert!(result.value < 20.0);
        // All values retained (just modified)
        assert_eq!(result.remaining_count, 6);
    }

    #[test]
    fn test_linear_fit_clipped_mean_constant_data() {
        let mut values = vec![5.0, 5.0, 5.0, 5.0, 5.0];
        let config = LinearFitClipConfig::default();
        let result = linear_fit_clipped_mean(&mut values, &config);
        assert!((result.value - 5.0).abs() < 0.01);
        assert_eq!(result.remaining_count, 5);
    }

    #[test]
    fn test_linear_fit_clipped_mean_linear_trend() {
        // Linear trend with one outlier
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 100.0, 6.0];
        let config = LinearFitClipConfig::new(2.0, 2.0, 3);
        let result = linear_fit_clipped_mean(&mut values, &config);
        // Should reject the 100.0 outlier
        assert!(result.remaining_count < 6);
        assert!(result.value < 20.0);
    }

    #[test]
    fn test_percentile_clipped_mean() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let config = PercentileClipConfig::new(20.0, 20.0);
        let result = percentile_clipped_mean(&mut values, &config);
        // Should clip 2 from each end (20% of 10)
        assert_eq!(result.remaining_count, 6);
        // Mean of [3, 4, 5, 6, 7, 8] = 5.5
        assert!((result.value - 5.5).abs() < 0.01);
    }

    #[test]
    fn test_gesd_mean_removes_outliers() {
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 100.0];
        let config = GesdConfig::new(0.05, Some(3));
        let result = gesd_mean(&mut values, &config);
        // Should detect and remove the 100.0 outlier
        assert!(result.remaining_count < 8);
        assert!(result.value < 10.0);
    }

    #[test]
    fn test_gesd_mean_no_outliers() {
        let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 1.1];
        let config = GesdConfig::new(0.05, Some(3));
        let result = gesd_mean(&mut values, &config);
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

        let sigma_result = sigma_clipped_mean(&mut values.clone(), &SigmaClipConfig::default());
        assert_eq!(sigma_result.remaining_count, 2);

        let winsorized_result =
            winsorized_sigma_clipped_mean(&values, &WinsorizedClipConfig::default());
        assert_eq!(winsorized_result.remaining_count, 2);

        let linear_result =
            linear_fit_clipped_mean(&mut values.clone(), &LinearFitClipConfig::default());
        assert_eq!(linear_result.remaining_count, 2);

        let percentile_result =
            percentile_clipped_mean(&mut values.clone(), &PercentileClipConfig::default());
        assert!(percentile_result.remaining_count >= 1);

        let gesd_result = gesd_mean(&mut values.clone(), &GesdConfig::default());
        assert_eq!(gesd_result.remaining_count, 2);
    }
}
