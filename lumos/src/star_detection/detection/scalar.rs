//! Scalar implementation of detection algorithms.
use crate::star_detection::background::BackgroundMap;

/// Scalar implementation for threshold mask, writing to an existing mask slice.
#[allow(dead_code)] // Used in tests and as fallback for non-SIMD architectures
pub fn create_threshold_mask(
    pixels: &[f32],
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut [bool],
) {
    debug_assert_eq!(pixels.len(), mask.len());
    for (i, &px) in pixels.iter().enumerate() {
        let threshold = background.background[i] + sigma_threshold * background.noise[i].max(1e-6);
        mask[i] = px > threshold;
    }
}

/// Scalar implementation for filtered images, writing to an existing mask slice.
#[allow(dead_code)] // Used in tests and as fallback for non-SIMD architectures
pub fn create_threshold_mask_filtered(
    filtered: &[f32],
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut [bool],
) {
    debug_assert_eq!(filtered.len(), mask.len());
    for (i, &px) in filtered.iter().enumerate() {
        let threshold = sigma_threshold * background.noise[i].max(1e-6);
        mask[i] = px > threshold;
    }
}
