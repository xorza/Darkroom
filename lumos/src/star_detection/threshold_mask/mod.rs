//! Threshold mask creation, packed 64 pixels to a word.
//!
//! Creates binary masks marking pixels above a sigma threshold: against a background map, which
//! background refinement masks bright objects with before the sky is known well enough to
//! subtract, and on the residual, which detection finds its candidates in.
//!
//! Uses bit-packed storage (`BitBuffer2`) for memory efficiency - each pixel
//! uses 1 bit instead of 1 byte, reducing memory usage by 8x.

use rayon::prelude::*;

mod simd;

use crate::bit_buffer2::BitBuffer2;
use imaginarium::Buffer2;

/// What a threshold comparison needs beyond the buffers: how many σ above the noise a pixel must
/// sit, and the floor its σ is held to first.
///
/// Bundled rather than passed loose because they travel together through the entry points into the
/// kernel, the same way the background interpolator hands its kernel a `SplineSegment`. A residual
/// is detected where it exceeds `sigma · max(noise, min_noise)`, plus the background when one is
/// given.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ThresholdParams {
    /// Detection threshold in units of the local noise σ.
    pub(crate) sigma: f32,
    /// Floor applied to each per-pixel σ, in the samples' own units — the frame's own floor from
    /// `background_estimate::noise_floor_from`, which is what every caller in the pipeline passes.
    pub(crate) min_noise: f32,
}

/// Create binary mask of pixels above threshold into a `BitBuffer2`.
///
/// Sets bit `i` to 1 where `pixels[i] > background[i] + sigma * noise[i]`.
///
/// Writes packed u64 words directly, eight lanes at a time on every CPU.
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
    // Release asserts, O(1) per image: a plane of other dimensions but enough samples would
    // threshold each pixel against another pixel's background without any panic.
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
            let row = y * width..(y + 1) * width;
            simd::process_words::<true>(
                &pixels[row.clone()],
                &bg[row.clone()],
                &noise[row],
                threshold,
                row_words,
            );
        });
}

/// Create binary mask from a residual — an image whose sky is already subtracted, matched-filtered
/// or not.
///
/// Sets bit `i` to 1 where `residual[i] > sigma * noise[i]`.
///
/// Note: All input buffers must have the same dimensions as the mask.
/// The output mask has row-aligned storage (stride may differ from width).
pub(crate) fn create_residual_threshold_mask(
    residual: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    threshold: ThresholdParams,
    mask: &mut BitBuffer2,
) {
    let width = mask.size.width;
    let height = mask.size.height;
    // Release asserts, as in `create_threshold_mask`.
    assert_eq!(width, residual.width());
    assert_eq!(height, residual.height());
    assert_eq!(width, noise.width());
    assert_eq!(height, noise.height());

    let words_per_row = mask.words_per_row();
    let residual = residual.pixels();
    let noise = noise.pixels();

    mask.words
        .par_chunks_mut(words_per_row)
        .enumerate()
        .for_each(|(y, row_words)| {
            let row = y * width..(y + 1) * width;
            simd::process_words::<false>(
                &residual[row.clone()],
                &[],
                &noise[row],
                threshold,
                row_words,
            );
        });
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::star_detection::threshold_mask::ThresholdParams;

    /// The σ floor this module's tests threshold with, shared by the kernel cross-checks in
    /// [`super::simd`]. Frame-derived in production; fixed here so every case is graded on the
    /// noise it declares, and low enough never to bind on it.
    pub(crate) const TEST_MIN_NOISE: f32 = 1e-6;

    impl ThresholdParams {
        /// The level a residual must exceed where the noise is `noise`: `sigma · max(noise,
        /// min_noise)`, the expression the kernel computes lane by lane.
        pub(crate) const fn level(self, noise: f32) -> f32 {
            self.sigma * noise.max(self.min_noise)
        }
    }

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
