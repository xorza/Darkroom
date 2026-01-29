//! Shared constants for star detection algorithms.
//!
//! This module centralizes mathematical and algorithmic constants used across
//! the star detection pipeline to avoid magic numbers and ensure consistency.

use crate::common::Buffer2;

/// FWHM to Gaussian sigma conversion factor.
///
/// For a Gaussian distribution, FWHM = 2√(2ln2) × σ ≈ 2.3548 × σ.
/// This is the exact value: 2 * sqrt(2 * ln(2)).
pub const FWHM_TO_SIGMA: f32 = 2.354_82;

/// MAD (Median Absolute Deviation) to standard deviation conversion factor.
///
/// For a normal distribution, σ ≈ 1.4826 × MAD.
/// This is the exact value: 1 / Φ⁻¹(3/4) where Φ⁻¹ is the inverse CDF.
pub const MAD_TO_SIGMA: f32 = 1.4826022;

/// Number of rows to process per parallel chunk.
///
/// This value is chosen to minimize false sharing between CPU cores while
/// maintaining good cache utilization. 8 rows × 64 bytes/cache line provides
/// enough separation for most image widths.
pub const ROWS_PER_CHUNK: usize = 8;

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

// Utility functions for conversions

/// Convert FWHM to Gaussian sigma.
#[inline]
pub fn fwhm_to_sigma(fwhm: f32) -> f32 {
    fwhm / FWHM_TO_SIGMA
}

/// Convert Gaussian sigma to FWHM.
#[inline]
pub fn sigma_to_fwhm(sigma: f32) -> f32 {
    sigma * FWHM_TO_SIGMA
}

/// Convert MAD to standard deviation (assuming normal distribution).
#[inline]
pub fn mad_to_sigma(mad: f32) -> f32 {
    mad * MAD_TO_SIGMA
}

/// Compute stamp radius from expected FWHM.
#[inline]
pub fn compute_stamp_radius(expected_fwhm: f32) -> usize {
    let radius = (expected_fwhm * STAMP_RADIUS_FWHM_FACTOR).ceil() as usize;
    radius.clamp(MIN_STAMP_RADIUS, MAX_STAMP_RADIUS)
}

/// Dilate a binary mask by the given radius (morphological dilation).
///
/// This connects nearby pixels that might be separated due to variable threshold.
/// Used in star detection to merge fragmented detections and in background
/// estimation to mask object wings.
///
/// # Arguments
/// * `mask` - Input binary mask
/// * `radius` - Dilation radius in pixels
/// * `output` - Output buffer for dilated mask (will be cleared and filled)
pub fn dilate_mask(mask: &Buffer2<bool>, radius: usize, output: &mut Buffer2<bool>) {
    assert_eq!(mask.width(), output.width(), "width mismatch");
    assert_eq!(mask.height(), output.height(), "height mismatch");
    output.fill(false);

    let width = mask.width();
    let height = mask.height();

    for y in 0..height {
        for x in 0..width {
            if mask[(x, y)] {
                // Set all pixels within radius
                let y_min = y.saturating_sub(radius);
                let y_max = (y + radius).min(height - 1);
                let x_min = x.saturating_sub(radius);
                let x_max = (x + radius).min(width - 1);

                for dy in y_min..=y_max {
                    for dx in x_min..=x_max {
                        output[(dx, dy)] = true;
                    }
                }
            }
        }
    }
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
    fn test_fwhm_sigma_conversion_roundtrip() {
        let fwhm = 4.5;
        let sigma = fwhm_to_sigma(fwhm);
        let fwhm_back = sigma_to_fwhm(sigma);
        assert!((fwhm - fwhm_back).abs() < 1e-6);
    }

    #[test]
    fn test_fwhm_to_sigma_known_value() {
        // For FWHM = 2.3548, sigma should be ~1.0
        let sigma = fwhm_to_sigma(FWHM_TO_SIGMA);
        assert!((sigma - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_mad_to_sigma_known_value() {
        // For MAD = 1.0, sigma should be ~1.4826
        let sigma = mad_to_sigma(1.0);
        assert!((sigma - MAD_TO_SIGMA).abs() < 1e-6);
    }

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
}
