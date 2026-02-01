//! Shared constants and utilities for star detection algorithms.
//!
//! This module centralizes mathematical and algorithmic constants used across
//! the star detection pipeline to avoid magic numbers and ensure consistency.

mod dilation;
pub mod threshold_mask;

pub use dilation::dilate_mask;

use crate::math::mad_to_sigma;

/// Stamp radius as a multiple of FWHM.
///
/// A stamp radius of 1.75 × FWHM captures approximately 99% of the PSF flux
/// for a Gaussian profile, providing accurate centroid and flux measurements
/// while minimizing background contamination.
pub const STAMP_RADIUS_FWHM_FACTOR: f32 = 1.75;

/// Minimum stamp radius in pixels.
///
/// Ensures sufficient pixels for accurate centroid computation even for
/// very small PSFs or undersampled images.
pub const MIN_STAMP_RADIUS: usize = 4;

/// Maximum stamp radius in pixels.
///
/// Limits computation time and prevents excessive background inclusion
/// for very large PSFs.
pub const MAX_STAMP_RADIUS: usize = 15;

/// Centroid convergence threshold in pixels squared.
///
/// Iteration stops when the squared distance moved is less than this value.
pub const CENTROID_CONVERGENCE_THRESHOLD: f32 = 0.001;

/// Maximum centroid iterations before giving up.
pub const MAX_CENTROID_ITERATIONS: usize = 10;

/// Compute stamp radius from expected FWHM.
#[inline]
pub fn compute_stamp_radius(expected_fwhm: f32) -> usize {
    let radius = (expected_fwhm * STAMP_RADIUS_FWHM_FACTOR).ceil() as usize;
    radius.clamp(MIN_STAMP_RADIUS, MAX_STAMP_RADIUS)
}

/// Compute sigma-clipped median and MAD-based sigma.
///
/// Iteratively rejects outliers beyond `kappa × sigma` from the median.
/// Uses scratch buffer `deviations` for efficiency when called repeatedly.
///
/// # Arguments
/// * `values` - Mutable slice of values (will be reordered)
/// * `deviations` - Scratch buffer for deviations (reused between calls)
/// * `kappa` - Number of sigma for clipping threshold
/// * `iterations` - Number of clipping iterations
///
/// # Returns
/// Tuple of (median, sigma) after clipping
pub fn sigma_clipped_median_mad(
    values: &mut [f32],
    deviations: &mut Vec<f32>,
    kappa: f32,
    iterations: usize,
) -> (f32, f32) {
    if values.is_empty() {
        return (0.0, 0.0);
    }

    let mut len = values.len();

    for _ in 0..iterations {
        if len < 3 {
            break;
        }

        let active = &mut values[..len];

        // Compute median
        let median = crate::math::median_f32_mut(active);

        // Compute MAD using deviations buffer
        deviations.clear();
        deviations.extend(active.iter().map(|v| (v - median).abs()));
        let mad = crate::math::median_f32_mut(deviations);
        let sigma = mad_to_sigma(mad);

        if sigma < f32::EPSILON {
            return (median, 0.0);
        }

        // Clip values outside threshold
        let threshold = kappa * sigma;
        let mut write_idx = 0;
        for i in 0..len {
            if (values[i] - median).abs() <= threshold {
                values[write_idx] = values[i];
                write_idx += 1;
            }
        }

        if write_idx == len {
            break;
        }
        len = write_idx;
    }

    // Final statistics
    let active = &mut values[..len];
    if active.is_empty() {
        return (0.0, 0.0);
    }

    let median = crate::math::median_f32_mut(active);

    deviations.clear();
    deviations.extend(active.iter().map(|v| (v - median).abs()));
    let mad = crate::math::median_f32_mut(deviations);
    let sigma = mad_to_sigma(mad);

    (median, sigma)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_stamp_radius_typical_fwhm() {
        // FWHM = 4.0 -> radius = ceil(4.0 * 1.75) = 7
        assert_eq!(compute_stamp_radius(4.0), 7);
    }

    #[test]
    fn test_compute_stamp_radius_clamped_min() {
        // Very small FWHM should clamp to minimum
        assert_eq!(compute_stamp_radius(1.0), MIN_STAMP_RADIUS);
    }

    #[test]
    fn test_compute_stamp_radius_clamped_max() {
        // Very large FWHM should clamp to maximum
        assert_eq!(compute_stamp_radius(20.0), MAX_STAMP_RADIUS);
    }

    // --- sigma_clipped_median_mad tests ---

    #[test]
    fn test_sigma_clipped_empty_input() {
        let mut values: Vec<f32> = vec![];
        let mut deviations = Vec::new();
        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
        assert_eq!(median, 0.0);
        assert_eq!(sigma, 0.0);
    }

    #[test]
    fn test_sigma_clipped_single_value() {
        let mut values = vec![5.0];
        let mut deviations = Vec::new();
        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
        assert_eq!(median, 5.0);
        assert_eq!(sigma, 0.0);
    }

    #[test]
    fn test_sigma_clipped_two_values() {
        let mut values = vec![2.0, 4.0];
        let mut deviations = Vec::new();
        let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
        // Median of [2, 4] = 3
        assert!((median - 3.0).abs() < 0.01);
    }

    #[test]
    fn test_sigma_clipped_uniform_values() {
        let mut values = vec![5.0; 100];
        let mut deviations = Vec::new();
        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
        assert_eq!(median, 5.0);
        assert_eq!(sigma, 0.0);
    }

    #[test]
    fn test_sigma_clipped_no_outliers() {
        // Normal-ish distribution with no extreme outliers
        let mut values: Vec<f32> = (0..100).map(|i| 50.0 + (i as f32 - 50.0) * 0.1).collect();
        let mut deviations = Vec::new();
        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
        // Median should be close to 50
        assert!((median - 50.0).abs() < 1.0);
        // Sigma should be small and non-zero
        assert!(sigma > 0.0);
        assert!(sigma < 10.0);
    }

    #[test]
    fn test_sigma_clipped_rejects_outliers() {
        // 97 values around 10, plus 3 extreme outliers
        let mut values: Vec<f32> = vec![10.0; 97];
        values.extend([1000.0, 2000.0, 3000.0]);
        let original_len = values.len();
        let mut deviations = Vec::new();

        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

        // After clipping, median should be ~10 (outliers rejected)
        assert!((median - 10.0).abs() < 0.1);
        // Sigma should be very small (uniform after clipping)
        assert!(sigma < 1.0);
        // Original slice reordered but length unchanged
        assert_eq!(values.len(), original_len);
    }

    #[test]
    fn test_sigma_clipped_negative_values() {
        let mut values = vec![-10.0, -5.0, 0.0, 5.0, 10.0];
        let mut deviations = Vec::new();
        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
        assert!((median - 0.0).abs() < 0.1);
        assert!(sigma > 0.0);
    }

    #[test]
    fn test_sigma_clipped_mixed_outliers() {
        // Values centered at 100 with outliers on both sides
        let mut values: Vec<f32> = vec![100.0; 90];
        values.extend([0.0, 1.0, 2.0, 198.0, 199.0, 200.0]); // outliers
        values.extend([99.0, 100.0, 101.0, 102.0]); // normal values
        let mut deviations = Vec::new();

        let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

        // After clipping, median should be close to 100
        assert!((median - 100.0).abs() < 2.0);
    }

    #[test]
    fn test_sigma_clipped_zero_iterations() {
        let mut values = vec![1.0, 2.0, 3.0, 1000.0];
        let mut deviations = Vec::new();
        let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 0);
        // With 0 iterations, should just return median/MAD of all values
        // Median of [1, 2, 3, 1000] = 2.5
        assert!((median - 2.5).abs() < 0.1);
    }

    #[test]
    fn test_sigma_clipped_one_iteration() {
        // 10 normal values plus one extreme outlier
        let mut values: Vec<f32> = vec![10.0; 10];
        values.push(10000.0);
        let mut deviations = Vec::new();

        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 1);

        // One iteration should reject the extreme outlier
        assert!((median - 10.0).abs() < 0.1);
        assert!(sigma < 1.0);
    }

    #[test]
    fn test_sigma_clipped_kappa_affects_clipping() {
        // Same data, different kappa values
        let base_values: Vec<f32> = {
            let mut v = vec![50.0; 90];
            v.extend([20.0, 25.0, 75.0, 80.0]); // moderate outliers
            v.extend([0.0, 100.0]); // extreme outliers
            v
        };

        let mut values_strict = base_values.clone();
        let mut values_loose = base_values.clone();
        let mut deviations = Vec::new();

        // Strict kappa (1.5) should clip more aggressively
        let (median_strict, sigma_strict) =
            sigma_clipped_median_mad(&mut values_strict, &mut deviations, 1.5, 3);

        // Loose kappa (5.0) should clip less
        let (median_loose, sigma_loose) =
            sigma_clipped_median_mad(&mut values_loose, &mut deviations, 5.0, 3);

        // Both should converge to similar median
        assert!((median_strict - 50.0).abs() < 5.0);
        assert!((median_loose - 50.0).abs() < 5.0);
        // Strict clipping should give smaller sigma
        assert!(sigma_strict <= sigma_loose);
    }

    #[test]
    fn test_sigma_clipped_deviations_buffer_reused() {
        let mut values1 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let mut values2 = vec![10.0, 20.0, 30.0];
        let mut deviations = Vec::new();

        sigma_clipped_median_mad(&mut values1, &mut deviations, 3.0, 2);
        let cap_after_first = deviations.capacity();

        sigma_clipped_median_mad(&mut values2, &mut deviations, 3.0, 2);

        // Buffer should be reused (capacity not reduced)
        assert!(deviations.capacity() >= cap_after_first.min(values2.len()));
    }

    #[test]
    fn test_sigma_clipped_large_dataset() {
        // 10000 values with some outliers
        let mut values: Vec<f32> = (0..10000).map(|i| 100.0 + (i % 10) as f32).collect();
        // Add 100 outliers
        for i in 0..100 {
            values[i * 100] = 1000.0;
        }
        let mut deviations = Vec::new();

        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

        // Should converge to values around 100-109
        assert!((100.0..=110.0).contains(&median));
        assert!(sigma > 0.0 && sigma < 20.0);
    }

    #[test]
    fn test_sigma_clipped_all_same_then_one_different() {
        // Edge case: many identical values plus one outlier
        let mut values: Vec<f32> = vec![42.0; 999];
        values.push(9999.0);
        let mut deviations = Vec::new();

        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

        assert!((median - 42.0).abs() < 0.01);
        // After clipping outlier, sigma should be 0 (all identical)
        assert!(sigma < 0.01);
    }
}
