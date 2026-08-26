//! SIMD-optimized threshold mask creation.
//!
//! Creates binary masks marking pixels above a sigma threshold relative to
//! background and noise estimates. Used by both background estimation
//! (to mask bright objects) and detection (to find star candidates).
//!
//! Uses bit-packed storage (`BitBuffer2`) for memory efficiency - each pixel
//! uses 1 bit instead of 1 byte, reducing memory usage by 8x.

use rayon::prelude::*;

mod simd;

#[cfg(test)]
pub(crate) mod internals {
    use crate::stacking::star_detection::threshold_mask::ThresholdParams;

    /// The σ floor this module's tests threshold with, shared by the kernel cross-checks in
    /// [`super::simd`]. Frame-derived in production; fixed here so every case is graded on the
    /// noise it declares, and low enough never to bind on it.
    pub(crate) const TEST_MIN_NOISE: f32 = 1e-6;

    /// [`ThresholdParams`] at `sigma` with the shared test floor.
    pub(crate) fn test_params(sigma: f32) -> ThresholdParams {
        ThresholdParams {
            sigma,
            min_noise: TEST_MIN_NOISE,
        }
    }
}

#[cfg(test)]
mod tests;

use crate::bit_buffer2::BitBuffer2;
use imaginarium::Buffer2;

/// What a threshold comparison needs beyond the buffers: how many σ above the noise a pixel must
/// sit, and the floor its σ is held to first.
///
/// Bundled rather than passed loose because they travel together through the entry points and every
/// SIMD backend, the same way the background interpolator hands its kernels a `SplineSegment`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ThresholdParams {
    /// Detection threshold in units of the local noise σ.
    pub(crate) sigma: f32,
    /// Floor applied to each per-pixel σ, in the samples' own units — see
    /// [`BackgroundEstimate::noise_floor`](crate::stacking::star_detection::background::background_estimate::BackgroundEstimate::noise_floor), which is
    /// what every caller in the pipeline passes.
    pub(crate) min_noise: f32,
}

/// Create binary mask of pixels above threshold into a BitBuffer2.
///
/// Sets bit `i` to 1 where `pixels[i] > background[i] + sigma * noise[i]`.
///
/// Uses SIMD acceleration when available (AVX2/SSE4.1 on x86_64, NEON on aarch64).
/// Writes directly to packed u64 words for better memory efficiency.
///
/// Note: All input buffers must have the same dimensions as the mask.
/// The output mask has row-aligned storage (stride may differ from width).
pub(crate) fn create_threshold_mask(
    pixels: &Buffer2<f32>,
    bg: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    threshold: ThresholdParams,
    mask: &mut BitBuffer2,
) {
    let width = mask.size.width;
    let height = mask.size.height;
    // Release asserts, not debug: the SIMD kernels do unchecked loads off these dims, so a mismatch
    // is out-of-bounds UB rather than a wrong pixel. The check is O(1) per whole-image call.
    assert_eq!(width, pixels.width());
    assert_eq!(height, pixels.height());
    assert_eq!(width, bg.width());
    assert_eq!(height, bg.height());
    assert_eq!(width, noise.width());
    assert_eq!(height, noise.height());

    let words_per_row = mask.words_per_row();
    let pixels = pixels.pixels();
    let bg = bg.pixels();
    let noise = noise.pixels();

    mask.words
        .par_chunks_mut(words_per_row)
        .enumerate()
        .for_each(|(y, row_words)| {
            let row_pixel_start = y * width;
            simd::process_words::<true>(
                pixels,
                bg,
                noise,
                threshold,
                row_words,
                row_pixel_start..row_pixel_start + width,
            );
        });
}

/// Create binary mask from a filtered (background-subtracted) image.
///
/// Sets bit `i` to 1 where `filtered[i] > sigma * noise[i]`.
/// Used for matched-filtered images where background is already subtracted.
///
/// Note: All input buffers must have the same dimensions as the mask.
/// The output mask has row-aligned storage (stride may differ from width).
pub(crate) fn create_threshold_mask_filtered(
    filtered: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    threshold: ThresholdParams,
    mask: &mut BitBuffer2,
) {
    let width = mask.size.width;
    let height = mask.size.height;
    // Release asserts (see `create_threshold_mask`): these dims drive unchecked SIMD loads.
    assert_eq!(width, filtered.width());
    assert_eq!(height, filtered.height());
    assert_eq!(width, noise.width());
    assert_eq!(height, noise.height());

    let words_per_row = mask.words_per_row();
    let filtered = filtered.pixels();
    let noise = noise.pixels();

    mask.words
        .par_chunks_mut(words_per_row)
        .enumerate()
        .for_each(|(y, row_words)| {
            let row_pixel_start = y * width;
            simd::process_words::<false>(
                filtered,
                &[],
                noise,
                threshold,
                row_words,
                row_pixel_start..row_pixel_start + width,
            );
        });
}
