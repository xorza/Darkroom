//! Shared constants and utilities for star detection algorithms.
//!
//! This module centralizes mathematical and algorithmic constants used across
//! the star detection pipeline to avoid magic numbers and ensure consistency.

pub mod threshold_mask;

use crate::common::BitBuffer2;
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

/// Dilate a binary mask by the given radius (morphological dilation).
///
/// This connects nearby pixels that might be separated due to variable threshold.
/// Used in star detection to merge fragmented detections and in background
/// estimation to mask object wings.
///
/// # Arguments
/// * `mask` - Input binary mask (BitBuffer2)
/// * `radius` - Dilation radius in pixels
/// * `output` - Output buffer for dilated mask (will be cleared and filled)
pub fn dilate_mask(mask: &BitBuffer2, radius: usize, output: &mut BitBuffer2) {
    assert_eq!(mask.width(), output.width(), "width mismatch");
    assert_eq!(mask.height(), output.height(), "height mismatch");
    output.fill(false);

    let width = mask.width();
    let height = mask.height();

    for y in 0..height {
        for x in 0..width {
            if mask.get_xy(x, y) {
                // Set all pixels within radius
                let y_min = y.saturating_sub(radius);
                let y_max = (y + radius).min(height - 1);
                let x_min = x.saturating_sub(radius);
                let x_max = (x + radius).min(width - 1);

                for dy in y_min..=y_max {
                    for dx in x_min..=x_max {
                        output.set_xy(dx, dy, true);
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
