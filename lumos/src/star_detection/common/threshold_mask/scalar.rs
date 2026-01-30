//! Scalar implementation of threshold mask creation.

use crate::common::Buffer2;
use crate::star_detection::background::BackgroundMap;

/// Scalar implementation for threshold mask.
///
/// Sets `mask[i] = true` where `pixels[i] > background[i] + sigma * noise[i]`.
pub fn create_threshold_mask(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    debug_assert_eq!(pixels.len(), mask.len());
    let mask_slice = mask.pixels_mut();
    for (i, &px) in pixels.iter().enumerate() {
        let threshold = background.background[i] + sigma_threshold * background.noise[i].max(1e-6);
        mask_slice[i] = px > threshold;
    }
}

/// Scalar implementation for filtered images.
///
/// Sets `mask[i] = true` where `filtered[i] > sigma * noise[i]`.
pub fn create_threshold_mask_filtered(
    filtered: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    debug_assert_eq!(filtered.len(), mask.len());
    let mask_slice = mask.pixels_mut();
    for (i, &px) in filtered.iter().enumerate() {
        let threshold = sigma_threshold * background.noise[i].max(1e-6);
        mask_slice[i] = px > threshold;
    }
}
