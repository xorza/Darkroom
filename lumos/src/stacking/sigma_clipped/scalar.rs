//! Scalar (non-SIMD) implementation of sigma-clipped mean calculation.

use crate::math;
use crate::stacking::SigmaClipConfig;

/// Calculate sigma-clipped mean using scalar operations.
///
/// Algorithm:
/// 1. Use median as center (robust to outliers)
/// 2. Compute std dev from median
/// 3. Clip values beyond sigma threshold from median
/// 4. Return mean of remaining values (statistically efficient)
pub(super) fn sigma_clipped_mean(values: &[f32], config: &SigmaClipConfig) -> f32 {
    debug_assert!(!values.is_empty());

    if values.len() <= 2 {
        return math::mean_f32(values);
    }

    let mut included: Vec<f32> = values.to_vec();

    for _ in 0..config.max_iterations {
        if included.len() <= 2 {
            break;
        }

        // Use median as center - robust to outliers
        let center = math::median_f32(&included);
        let variance = math::sum_squared_diff(&included, center) / included.len() as f32;
        let std_dev = variance.sqrt();

        if std_dev < f32::EPSILON {
            break;
        }

        let threshold = config.sigma * std_dev;
        let prev_len = included.len();

        included.retain(|&v| (v - center).abs() <= threshold);

        // Stop if no values were clipped
        if included.len() == prev_len {
            break;
        }
    }

    // Return mean of remaining values (lower noise than median for Gaussian data)
    math::mean_f32(&included)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sigma_clipped_mean_removes_outlier() {
        let values = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0];
        let config = SigmaClipConfig::new(2.0, 3);
        let result = sigma_clipped_mean(&values, &config);
        assert!(
            result < 10.0,
            "Expected outlier to be clipped, got {}",
            result
        );
    }

    #[test]
    fn test_sigma_clipped_mean_large() {
        let mut values: Vec<f32> = vec![10.0; 50];
        values.push(1000.0);
        let config = SigmaClipConfig::new(2.0, 3);
        let result = sigma_clipped_mean(&values, &config);
        assert!(
            (result - 10.0).abs() < 1.0,
            "Expected ~10.0, got {}",
            result
        );
    }
}
