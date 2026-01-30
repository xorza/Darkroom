//! Scalar implementation for threshold mask creation.

use crate::common::Buffer2;
use crate::star_detection::background::BackgroundMap;

/// Scalar implementation for threshold mask.
///
/// When `INCLUDE_BACKGROUND` is true:
///   Sets `mask[i] = true` where `pixels[i] > background[i] + sigma * noise[i]`.
///
/// When `INCLUDE_BACKGROUND` is false:
///   Sets `mask[i] = true` where `pixels[i] > sigma * noise[i]`.
pub fn create_threshold_mask_impl<const INCLUDE_BACKGROUND: bool>(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    debug_assert_eq!(pixels.len(), mask.len());
    let mask_slice = mask.pixels_mut();
    for (i, &px) in pixels.iter().enumerate() {
        let base = if INCLUDE_BACKGROUND {
            background.background[i]
        } else {
            0.0
        };
        let threshold = base + sigma_threshold * background.noise[i].max(1e-6);
        mask_slice[i] = px > threshold;
    }
}
