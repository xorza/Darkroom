//! Scalar implementation of detection algorithms.
use crate::star_detection::background::BackgroundMap;

/// Scalar implementation for threshold mask, appending to an existing mask vector.
pub fn create_threshold_mask(
    pixels: &[f32],
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Vec<bool>,
) {
    for (i, &px) in pixels.iter().enumerate() {
        let threshold = background.background[i] + sigma_threshold * background.noise[i].max(1e-6);
        mask.push(px > threshold);
    }
}

/// Scalar implementation for filtered images, appending to an existing mask vector.
pub fn create_threshold_mask_filtered(
    filtered: &[f32],
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Vec<bool>,
) {
    for (i, &px) in filtered.iter().enumerate() {
        let threshold = sigma_threshold * background.noise[i].max(1e-6);
        mask.push(px > threshold);
    }
}
