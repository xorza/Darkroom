//! Scalar implementation of detection algorithms.
use crate::star_detection::background::BackgroundMap;

/// Create binary mask of pixels above threshold (scalar version).
pub fn create_threshold_mask(
    pixels: &[f32],
    background: &BackgroundMap,
    sigma_threshold: f32,
) -> Vec<bool> {
    let mut mask = Vec::with_capacity(pixels.len());
    create_threshold_mask_slice(pixels, background, sigma_threshold, 0, &mut mask);
    mask
}

/// Scalar implementation for a slice of pixels, appending to an existing mask vector.
pub fn create_threshold_mask_slice(
    pixels: &[f32],
    background: &BackgroundMap,
    sigma_threshold: f32,
    offset: usize, // The starting index of the slice within the full image
    mask: &mut Vec<bool>,
) {
    for (i, &px) in pixels.iter().enumerate() {
        let idx = offset + i;
        let threshold =
            background.background[idx] + sigma_threshold * background.noise[idx].max(1e-6);
        mask.push(px > threshold);
    }
}

