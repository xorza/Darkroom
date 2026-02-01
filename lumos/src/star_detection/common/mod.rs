//! Shared constants and utilities for star detection algorithms.
//!
//! This module centralizes mathematical and algorithmic constants used across
//! the star detection pipeline to avoid magic numbers and ensure consistency.

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
