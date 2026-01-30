//! Scalar implementation for threshold mask creation.

/// Scalar implementation for threshold mask on a chunk.
///
/// When `INCLUDE_BACKGROUND` is true:
///   Sets `mask[i] = true` where `pixels[i] > bg[i] + sigma * noise[i]`.
///
/// When `INCLUDE_BACKGROUND` is false:
///   Sets `mask[i] = true` where `pixels[i] > sigma * noise[i]`.
#[inline]
pub fn process_chunk_scalar<const INCLUDE_BACKGROUND: bool>(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    mask: &mut [bool],
) {
    debug_assert_eq!(pixels.len(), mask.len());
    for (i, &px) in pixels.iter().enumerate() {
        let base = if INCLUDE_BACKGROUND { bg[i] } else { 0.0 };
        let threshold = base + sigma_threshold * noise[i].max(1e-6);
        mask[i] = px > threshold;
    }
}
