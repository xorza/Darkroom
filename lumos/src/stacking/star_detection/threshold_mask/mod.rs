//! SIMD-optimized threshold mask creation.
//!
//! Creates binary masks marking pixels above a sigma threshold relative to
//! background and noise estimates. Used by both background estimation
//! (to mask bright objects) and detection (to find star candidates).
//!
//! Uses bit-packed storage (`BitBuffer2`) for memory efficiency - each pixel
//! uses 1 bit instead of 1 byte, reducing memory usage by 8x.

use rayon::prelude::*;

#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "x86_64")]
mod sse;

#[cfg(test)]
mod bench;

#[cfg(test)]
mod tests;

use common::BitBuffer2;
use imaginarium::Buffer2;

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

/// Scalar packed threshold kernel. With `WITH_BG` the threshold is `bg + σ·noise`; otherwise it is
/// `σ·noise` (matched-filter case — background already subtracted), and `bg` is unused and may be
/// empty.
#[cfg_attr(not(test), inline)]
#[cfg(any(test, target_arch = "x86_64", not(target_arch = "aarch64")))]
pub(crate) fn process_words_scalar<const WITH_BG: bool>(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    words: &mut [u64],
    pixel_offset: usize,
    pixel_end: usize,
) {
    for (word_idx, word) in words.iter_mut().enumerate() {
        let base_pixel = pixel_offset + word_idx * 64;
        let mut bits = 0u64;

        for bit in 0..64 {
            let px_idx = base_pixel + bit;
            if px_idx >= pixel_end {
                break;
            }

            let px = pixels[px_idx];
            let mut threshold = sigma_threshold * noise[px_idx].max(1e-6);
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
pub(crate) fn process_words<const WITH_BG: bool>(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    words: &mut [u64],
    pixel_offset: usize,
    pixel_end: usize,
) {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        unsafe {
            neon::process_words_neon::<WITH_BG>(
                pixels,
                bg,
                noise,
                sigma_threshold,
                words,
                pixel_offset,
                pixel_end,
            );
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_sse4_1() {
            // SAFETY: SSE4.1 availability checked above.
            unsafe {
                sse::process_words_sse::<WITH_BG>(
                    pixels,
                    bg,
                    noise,
                    sigma_threshold,
                    words,
                    pixel_offset,
                    pixel_end,
                );
            }
            return;
        }

        process_words_scalar::<WITH_BG>(
            pixels,
            bg,
            noise,
            sigma_threshold,
            words,
            pixel_offset,
            pixel_end,
        );
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        process_words_scalar::<WITH_BG>(
            pixels,
            bg,
            noise,
            sigma_threshold,
            words,
            pixel_offset,
            pixel_end,
        );
    }
}

/// Create binary mask of pixels above threshold into a BitBuffer2.
///
/// Sets bit `i` to 1 where `pixels[i] > background[i] + sigma * noise[i]`.
///
/// Uses SIMD acceleration when available (SSE4.1 on x86_64, NEON on aarch64).
/// Writes directly to packed u64 words for better memory efficiency.
///
/// Note: All input buffers must have the same dimensions as the mask.
/// The output mask has row-aligned storage (stride may differ from width).
pub(crate) fn create_threshold_mask(
    pixels: &Buffer2<f32>,
    bg: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    sigma_threshold: f32,
    mask: &mut BitBuffer2,
) {
    let width = mask.width();
    let height = mask.height();
    debug_assert_eq!(width, pixels.width());
    debug_assert_eq!(height, pixels.height());
    debug_assert_eq!(width, bg.width());
    debug_assert_eq!(height, bg.height());
    debug_assert_eq!(width, noise.width());
    debug_assert_eq!(height, noise.height());

    let words_per_row = mask.words_per_row();
    let pixels = pixels.pixels();
    let bg = bg.pixels();
    let noise = noise.pixels();

    mask.words_mut()
        .par_chunks_mut(words_per_row)
        .enumerate()
        .for_each(|(y, row_words)| {
            let row_pixel_start = y * width;
            process_words::<true>(
                pixels,
                bg,
                noise,
                sigma_threshold,
                row_words,
                row_pixel_start,
                row_pixel_start + width,
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
    sigma_threshold: f32,
    mask: &mut BitBuffer2,
) {
    let width = mask.width();
    let height = mask.height();
    debug_assert_eq!(width, filtered.width());
    debug_assert_eq!(height, filtered.height());
    debug_assert_eq!(width, noise.width());
    debug_assert_eq!(height, noise.height());

    let words_per_row = mask.words_per_row();
    let filtered = filtered.pixels();
    let noise = noise.pixels();

    mask.words_mut()
        .par_chunks_mut(words_per_row)
        .enumerate()
        .for_each(|(y, row_words)| {
            let row_pixel_start = y * width;
            process_words::<false>(
                filtered,
                &[],
                noise,
                sigma_threshold,
                row_words,
                row_pixel_start,
                row_pixel_start + width,
            );
        });
}
