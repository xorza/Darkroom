//! Vector backends for the packed threshold kernel, and the dispatch between them.
//!
//! The kernel writes 64 pixels' worth of comparisons into one `u64` word, so every backend shares
//! the same scalar partial-word tail and the same noise floor — that is what keeps their output
//! bit-identical rather than merely close.

use std::ops::Range;

use crate::simd::dispatch;
use crate::stacking::star_detection::threshold_mask::ThresholdParams;

#[cfg(all(test, feature = "internals"))]
mod bench;

#[cfg(target_arch = "x86_64")]
mod avx2;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod sse41;

#[cfg(test)]
mod tests;

/// Scalar packed threshold kernel. With `WITH_BG` the threshold is `bg + σ·noise`; otherwise it is
/// `σ·noise` (matched-filter case — background already subtracted), and `bg` is unused and may be
/// empty. Also serves as the shared partial-word tail for every SIMD backend.
///
/// Every backend applies the same [`ThresholdParams`] the same way, which is what keeps their
/// output bit-identical rather than merely close.
#[cfg_attr(not(test), inline)]
pub(super) fn process_words_scalar<const WITH_BG: bool>(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    threshold_params: ThresholdParams,
    words: &mut [u64],
    pixel_span: Range<usize>,
) {
    for (word_idx, word) in words.iter_mut().enumerate() {
        let base_pixel = pixel_span.start + word_idx * 64;
        let mut bits = 0u64;

        for bit in 0..64 {
            let px_idx = base_pixel + bit;
            if px_idx >= pixel_span.end {
                break;
            }

            let px = pixels[px_idx];
            let mut threshold =
                threshold_params.sigma * noise[px_idx].max(threshold_params.min_noise);
            if WITH_BG {
                threshold += bg[px_idx];
            }

            if px > threshold {
                bits |= 1u64 << bit;
            }
        }

        *word = bits;
    }
}

/// Dispatch the packed threshold kernel to the best available backend. See `process_words_scalar`
/// for the `WITH_BG` meaning; pass an empty `bg` when `WITH_BG` is false.
#[cfg_attr(not(test), inline)]
pub(super) fn process_words<const WITH_BG: bool>(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    threshold: ThresholdParams,
    words: &mut [u64],
    pixel_span: Range<usize>,
) {
    dispatch! {
        x86: avx2 => avx2::process_words_avx2::<WITH_BG>(
            pixels, bg, noise, threshold, words, pixel_span,
        ),
        x86: sse4_1 => sse41::process_words_sse::<WITH_BG>(
            pixels, bg, noise, threshold, words, pixel_span,
        ),
        aarch64 => neon::process_words_neon::<WITH_BG>(
            pixels, bg, noise, threshold, words, pixel_span,
        ),
        scalar => process_words_scalar::<WITH_BG>(
            pixels, bg, noise, threshold, words, pixel_span,
        ),
    }
}
