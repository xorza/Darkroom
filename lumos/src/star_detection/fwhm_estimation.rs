//! FWHM estimation from detected bright stars.
//!
//! This module provides robust FWHM estimation for the auto-FWHM feature.
//! It uses median and MAD (Median Absolute Deviation) statistics to handle
//! outliers from cosmic rays, saturated stars, and edge artifacts.

use crate::math::median_f32_mut;

use super::Star;

/// Result of FWHM estimation.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // MAD is useful diagnostic info even if not directly used
pub struct FwhmEstimate {
    /// Estimated FWHM in pixels.
    pub fwhm: f32,
    /// Number of stars used for estimation (after filtering).
    pub star_count: usize,
    /// Median Absolute Deviation of FWHM values.
    pub mad: f32,
    /// Whether estimation succeeded (true) or fell back to default (false).
    pub is_estimated: bool,
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
    // Filter stars for quality
    let mut fwhms: Vec<f32> = stars
        .iter()
        .filter(|s| !s.is_saturated())
        .filter(|s| s.eccentricity <= max_eccentricity)
        .filter(|s| s.fwhm > 0.5 && s.fwhm < 20.0) // Reasonable FWHM range
        .filter(|s| s.sharpness < max_sharpness) // Not cosmic ray
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

    // First pass: compute median and MAD
    let median = median_f32_mut(&mut fwhms);
    let mut deviations: Vec<f32> = fwhms.iter().map(|&f| (f - median).abs()).collect();
    let mad = median_f32_mut(&mut deviations);

    // Reject outliers: keep only stars within 3*MAD of median
    // Use effective MAD with floor to handle uniform distributions
    let effective_mad = mad.max(median * 0.1);
    let max_fwhm = median + 3.0 * effective_mad;
    let min_fwhm = (median - 3.0 * effective_mad).max(0.5);

    let mut filtered_fwhms: Vec<f32> = fwhms
        .into_iter()
        .filter(|&f| f >= min_fwhm && f <= max_fwhm)
        .collect();

    if filtered_fwhms.len() < min_stars {
        tracing::debug!(
            "Too many outliers rejected ({} -> {}), using pre-rejection median {:.2}",
            deviations.len(),
            filtered_fwhms.len(),
            median
        );
        return FwhmEstimate {
            fwhm: median,
            star_count: filtered_fwhms.len(),
            mad,
            is_estimated: true,
        };
    }

    // Final estimate from filtered stars
    let final_median = median_f32_mut(&mut filtered_fwhms);
    let mut final_deviations: Vec<f32> = filtered_fwhms
        .iter()
        .map(|&f| (f - final_median).abs())
        .collect();
    let final_mad = median_f32_mut(&mut final_deviations);

    tracing::info!(
        "Estimated FWHM: {:.2} pixels (MAD: {:.2}, from {} stars)",
        final_median,
        final_mad,
        filtered_fwhms.len()
    );

    FwhmEstimate {
        fwhm: final_median,
        star_count: filtered_fwhms.len(),
        mad: final_mad,
        is_estimated: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_star(fwhm: f32, flux: f32) -> Star {
        Star {
            x: 100.0,
            y: 100.0,
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
}
