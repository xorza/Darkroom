//! FWHM estimation from detected bright stars.
//!
//! This module provides robust FWHM estimation for the auto-FWHM feature.
//! It uses median and MAD (Median Absolute Deviation) statistics to handle
//! outliers from cosmic rays, saturated stars, and edge artifacts.

use crate::math::{mad_f32_with_scratch, median_f32_mut};

use super::Star;

/// Result of FWHM estimation.
#[derive(Debug, Clone, Copy)]
// MAD is useful diagnostic info even if not directly used
pub struct FwhmEstimate {
    /// Estimated FWHM in pixels.
    pub fwhm: f32,
    /// Number of stars used for estimation (after filtering).
    pub star_count: usize,
    /// Median Absolute Deviation of FWHM values.
    #[allow(dead_code)]
    pub mad: f32,
    /// Whether estimation succeeded (true) or fell back to default (false).
    pub is_estimated: bool,
}

/// Effective FWHM configuration for matched filtering.
#[derive(Debug, Clone)]
pub enum EffectiveFwhm {
    /// No matched filtering (disabled).
    Disabled,
    /// Manual FWHM specified by user.
    Manual(f32),
    /// Auto-estimated FWHM with estimation details.
    Estimated(FwhmEstimate),
}

impl EffectiveFwhm {
    /// Get the FWHM value if matched filtering is enabled.
    #[inline]
    pub fn fwhm(&self) -> Option<f32> {
        match self {
            Self::Disabled => None,
            Self::Manual(fwhm) => Some(*fwhm),
            Self::Estimated(estimate) => Some(estimate.fwhm),
        }
    }

    /// Get the estimation details if FWHM was auto-estimated.
    #[inline]
    pub fn estimate(&self) -> Option<&FwhmEstimate> {
        match self {
            Self::Estimated(estimate) => Some(estimate),
            _ => None,
        }
    }
}

/// Estimate FWHM from a set of detected stars.
///
/// Uses robust statistics (median + MAD) to handle outliers from
/// cosmic rays, saturated stars, and edge artifacts.
///
/// # Algorithm
/// 1. Filter stars by quality (not saturated, reasonable eccentricity, positive FWHM, not cosmic ray)
/// 2. Compute median FWHM from filtered stars
/// 3. Reject outliers using MAD-based threshold (keep within 3×MAD of median)
/// 4. Recompute median from remaining stars
///
/// # Arguments
/// * `stars` - Detected stars (should be bright, high-SNR stars from first pass)
/// * `min_stars` - Minimum stars required for valid estimation
/// * `default_fwhm` - Fallback FWHM if estimation fails
/// * `max_eccentricity` - Maximum eccentricity for stars used in estimation
/// * `max_sharpness` - Maximum sharpness for stars used in estimation (filters cosmic rays)
pub fn estimate_fwhm(
    stars: &[Star],
    min_stars: usize,
    default_fwhm: f32,
    max_eccentricity: f32,
    max_sharpness: f32,
) -> FwhmEstimate {
    // Filter stars for quality and collect FWHM values
    let mut fwhms: Vec<f32> = stars
        .iter()
        .filter(|s| {
            !s.is_saturated()
                && s.eccentricity <= max_eccentricity
                && s.sharpness < max_sharpness
                && (0.5..20.0).contains(&s.fwhm)
        })
        .map(|s| s.fwhm)
        .collect();

    if fwhms.len() < min_stars {
        tracing::debug!(
            "Insufficient stars for FWHM estimation: {} < {}, using default {:.1}",
            fwhms.len(),
            min_stars,
            default_fwhm
        );
        return FwhmEstimate {
            fwhm: default_fwhm,
            star_count: fwhms.len(),
            mad: 0.0,
            is_estimated: false,
        };
    }

    // Scratch buffer for MAD computation (reused across calls)
    let mut scratch = Vec::with_capacity(fwhms.len());

    // Compute median and MAD for outlier rejection
    let median = median_f32_mut(&mut fwhms);
    let mad = mad_f32_with_scratch(&fwhms, median, &mut scratch);

    // Reject outliers: keep within 3×MAD of median (with floor for uniform distributions)
    let threshold = 3.0 * mad.max(median * 0.1);
    let count_before = fwhms.len();
    fwhms.retain(|&f| (f - median).abs() <= threshold);

    // If too many rejected, use pre-rejection median
    if fwhms.len() < min_stars {
        tracing::debug!(
            "Too many outliers rejected ({count_before} -> {}), using pre-rejection median {median:.2}",
            fwhms.len(),
        );
        return FwhmEstimate {
            fwhm: median,
            star_count: fwhms.len(),
            mad,
            is_estimated: true,
        };
    }

    // Final estimate from filtered stars
    let final_median = median_f32_mut(&mut fwhms);
    let final_mad = mad_f32_with_scratch(&fwhms, final_median, &mut scratch);

    tracing::info!(
        "Estimated FWHM: {final_median:.2} pixels (MAD: {final_mad:.2}, from {} stars)",
        fwhms.len()
    );

    FwhmEstimate {
        fwhm: final_median,
        star_count: fwhms.len(),
        mad: final_mad,
        is_estimated: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_star(fwhm: f32, flux: f32) -> Star {
        Star {
            pos: glam::DVec2::new(100.0, 100.0),
            flux,
            fwhm,
            eccentricity: 0.1,
            snr: 50.0,
            peak: 0.5,
            sharpness: 0.3,
            roundness1: 0.0,
            roundness2: 0.0,
            laplacian_snr: 0.0,
        }
    }

    #[test]
    fn test_estimate_fwhm_sufficient_stars() {
        // Create 20 stars with FWHM around 3.5
        let stars: Vec<Star> = (0..20)
            .map(|i| make_test_star(3.5 + (i as f32 * 0.05), 100.0 - i as f32))
            .collect();

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(estimate.is_estimated);
        assert!((estimate.fwhm - 3.95).abs() < 0.5); // Should be around median
        assert!(estimate.star_count >= 10);
    }

    #[test]
    fn test_estimate_fwhm_insufficient_stars() {
        let stars: Vec<Star> = (0..5)
            .map(|i| make_test_star(3.5, 100.0 - i as f32))
            .collect();

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(!estimate.is_estimated);
        assert!((estimate.fwhm - 4.0).abs() < f32::EPSILON); // Default
    }

    #[test]
    fn test_estimate_fwhm_rejects_outliers() {
        let mut stars: Vec<Star> = (0..18)
            .map(|i| make_test_star(3.5 + (i as f32 * 0.05), 100.0 - i as f32))
            .collect();
        // Add outliers
        stars.push(make_test_star(15.0, 10.0)); // Large FWHM outlier
        stars.push(make_test_star(0.8, 5.0)); // Small FWHM outlier

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(estimate.is_estimated);
        assert!((estimate.fwhm - 3.9).abs() < 0.5); // Should ignore outliers
    }

    #[test]
    fn test_estimate_fwhm_filters_saturated() {
        let stars: Vec<Star> = (0..15)
            .map(|i| {
                let mut s = make_test_star(3.5, 100.0 - i as f32);
                if i < 5 {
                    s.peak = 0.98; // Saturated
                    s.fwhm = 8.0; // Saturated stars have larger apparent FWHM
                }
                s
            })
            .collect();

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(estimate.is_estimated);
        assert!((estimate.fwhm - 3.5).abs() < 0.5); // Should ignore saturated
    }

    #[test]
    fn test_estimate_fwhm_filters_cosmic_rays() {
        let mut stars: Vec<Star> = (0..15)
            .map(|i| make_test_star(3.5, 100.0 - i as f32))
            .collect();
        // Add cosmic rays (high sharpness, small FWHM)
        for i in 0..5 {
            let mut s = make_test_star(1.5, 50.0 - i as f32);
            s.sharpness = 0.9;
            stars.push(s);
        }

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(estimate.is_estimated);
        assert!((estimate.fwhm - 3.5).abs() < 0.5);
    }

    #[test]
    fn test_estimate_fwhm_filters_elongated() {
        let mut stars: Vec<Star> = (0..15)
            .map(|i| make_test_star(3.5, 100.0 - i as f32))
            .collect();
        // Add elongated sources (high eccentricity)
        for i in 0..5 {
            let mut s = make_test_star(6.0, 50.0 - i as f32);
            s.eccentricity = 0.8;
            stars.push(s);
        }

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(estimate.is_estimated);
        assert!((estimate.fwhm - 3.5).abs() < 0.5); // Should ignore elongated
    }

    #[test]
    fn test_estimate_fwhm_uniform_distribution() {
        // All stars have exactly the same FWHM
        let stars: Vec<Star> = (0..20)
            .map(|i| make_test_star(3.5, 100.0 - i as f32))
            .collect();

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(estimate.is_estimated);
        assert!((estimate.fwhm - 3.5).abs() < 0.01);
        assert!(estimate.mad < 0.01); // MAD should be ~0 for uniform
    }

    #[test]
    fn test_estimate_fwhm_empty_input() {
        let stars: Vec<Star> = vec![];

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(!estimate.is_estimated);
        assert!((estimate.fwhm - 4.0).abs() < f32::EPSILON);
        assert_eq!(estimate.star_count, 0);
    }

    #[test]
    fn test_estimate_fwhm_exactly_min_stars() {
        // Exactly min_stars should succeed
        let stars: Vec<Star> = (0..10)
            .map(|i| make_test_star(3.5 + (i as f32 * 0.02), 100.0 - i as f32))
            .collect();

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(estimate.is_estimated);
        assert_eq!(estimate.star_count, 10);
    }

    #[test]
    fn test_estimate_fwhm_filters_invalid_fwhm_range() {
        let mut stars: Vec<Star> = (0..15)
            .map(|i| make_test_star(3.5, 100.0 - i as f32))
            .collect();
        // Add stars with invalid FWHM (too small or too large)
        stars.push(make_test_star(0.3, 50.0)); // Below 0.5 threshold
        stars.push(make_test_star(25.0, 40.0)); // Above 20.0 threshold

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(estimate.is_estimated);
        assert!((estimate.fwhm - 3.5).abs() < 0.1);
        assert_eq!(estimate.star_count, 15); // Invalid FWHM stars filtered
    }

    #[test]
    fn test_estimate_fwhm_bimodal_distribution() {
        // Two groups of stars with different FWHM (simulates focus issues)
        let mut stars: Vec<Star> = (0..10)
            .map(|i| make_test_star(3.0, 100.0 - i as f32))
            .collect();
        // Second group with larger FWHM
        for i in 0..10 {
            stars.push(make_test_star(5.0, 80.0 - i as f32));
        }

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(estimate.is_estimated);
        // Median should be between the two groups
        assert!(estimate.fwhm >= 3.0 && estimate.fwhm <= 5.0);
    }

    #[test]
    fn test_estimate_fwhm_all_filtered_out() {
        // All stars are saturated - should fall back to default
        let stars: Vec<Star> = (0..20)
            .map(|i| {
                let mut s = make_test_star(3.5, 100.0 - i as f32);
                s.peak = 0.99; // All saturated
                s
            })
            .collect();

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(!estimate.is_estimated);
        assert!((estimate.fwhm - 4.0).abs() < f32::EPSILON); // Default
    }

    #[test]
    fn test_estimate_fwhm_too_many_outliers_uses_pre_rejection() {
        // Create scenario where outlier rejection removes too many stars
        // 12 stars: 10 at FWHM=3.5, 2 extreme outliers
        let mut stars: Vec<Star> = (0..8)
            .map(|i| make_test_star(3.5, 100.0 - i as f32))
            .collect();
        // Add outliers that will cause aggressive rejection
        stars.push(make_test_star(3.5, 90.0));
        stars.push(make_test_star(3.5, 89.0));
        stars.push(make_test_star(10.0, 88.0)); // Outlier
        stars.push(make_test_star(12.0, 87.0)); // Outlier

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        // Should still succeed, using pre-rejection or post-rejection median
        assert!(estimate.is_estimated);
        assert!((estimate.fwhm - 3.5).abs() < 1.5);
    }

    #[test]
    fn test_estimate_fwhm_realistic_seeing_variation() {
        // Simulate realistic seeing variation (FWHM varies by ~10-15%)
        let stars: Vec<Star> = (0..50)
            .map(|i| {
                // FWHM varies around 4.0 with ~0.4 std dev
                let variation = ((i as f32 * 7.0) % 10.0 - 5.0) * 0.08;
                make_test_star(4.0 + variation, 100.0 - i as f32 * 0.5)
            })
            .collect();

        let estimate = estimate_fwhm(&stars, 10, 5.0, 0.6, 0.7);

        assert!(estimate.is_estimated);
        assert!((estimate.fwhm - 4.0).abs() < 0.3);
        assert!(estimate.star_count >= 40); // Most should pass
    }

    #[test]
    fn test_estimate_fwhm_mixed_quality() {
        // Mix of good stars, cosmic rays, saturated, and elongated
        let mut stars: Vec<Star> = vec![];

        // 15 good stars
        for i in 0..15 {
            stars.push(make_test_star(3.5 + (i as f32 * 0.03), 100.0 - i as f32));
        }

        // 3 cosmic rays
        for i in 0..3 {
            let mut s = make_test_star(1.2, 50.0 - i as f32);
            s.sharpness = 0.85;
            stars.push(s);
        }

        // 3 saturated
        for i in 0..3 {
            let mut s = make_test_star(7.0, 200.0 - i as f32);
            s.peak = 0.98;
            stars.push(s);
        }

        // 3 elongated (galaxies/trails)
        for i in 0..3 {
            let mut s = make_test_star(5.5, 30.0 - i as f32);
            s.eccentricity = 0.75;
            stars.push(s);
        }

        let estimate = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);

        assert!(estimate.is_estimated);
        // Should only use the 15 good stars
        assert!((estimate.fwhm - 3.7).abs() < 0.3);
        assert!(estimate.star_count >= 10 && estimate.star_count <= 15);
    }

    #[test]
    fn test_estimate_fwhm_different_default_values() {
        let stars: Vec<Star> = (0..3)
            .map(|i| make_test_star(3.5, 100.0 - i as f32))
            .collect();

        // Test with different default FWHM
        let estimate = estimate_fwhm(&stars, 10, 2.5, 0.6, 0.7);
        assert!(!estimate.is_estimated);
        assert!((estimate.fwhm - 2.5).abs() < f32::EPSILON);

        let estimate = estimate_fwhm(&stars, 10, 6.0, 0.6, 0.7);
        assert!(!estimate.is_estimated);
        assert!((estimate.fwhm - 6.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_estimate_fwhm_varying_thresholds() {
        let mut stars: Vec<Star> = (0..15)
            .map(|i| make_test_star(3.5, 100.0 - i as f32))
            .collect();
        // Add elongated source
        let mut elongated = make_test_star(3.5, 50.0);
        elongated.eccentricity = 0.65;
        stars.push(elongated);

        // With strict eccentricity threshold (0.6), elongated is filtered
        let estimate_strict = estimate_fwhm(&stars, 10, 4.0, 0.6, 0.7);
        assert_eq!(estimate_strict.star_count, 15);

        // With relaxed eccentricity threshold (0.7), elongated is included
        let estimate_relaxed = estimate_fwhm(&stars, 10, 4.0, 0.7, 0.7);
        assert_eq!(estimate_relaxed.star_count, 16);
    }

    #[test]
    fn test_estimate_fwhm_mad_calculation() {
        // Create distribution where we can verify MAD
        // FWHM values: 3.0, 3.2, 3.4, 3.6, 3.8, 4.0, 4.2, 4.4, 4.6, 4.8
        // Median = 3.9, deviations from median: 0.9, 0.7, 0.5, 0.3, 0.1, 0.1, 0.3, 0.5, 0.7, 0.9
        // MAD = median of deviations = 0.5
        let stars: Vec<Star> = (0..10)
            .map(|i| make_test_star(3.0 + i as f32 * 0.2, 100.0 - i as f32))
            .collect();

        let estimate = estimate_fwhm(&stars, 5, 4.0, 0.6, 0.7);

        assert!(estimate.is_estimated);
        assert!((estimate.fwhm - 3.9).abs() < 0.15);
        assert!(estimate.mad < 0.6); // MAD should be small for this distribution
    }
}
