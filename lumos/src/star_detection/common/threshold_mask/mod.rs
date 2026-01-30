//! SIMD-optimized threshold mask creation.
//!
//! Creates binary masks marking pixels above a sigma threshold relative to
//! background and noise estimates. Used by both background estimation
//! (to mask bright objects) and detection (to find star candidates).

pub mod scalar;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;

use crate::common::Buffer2;
use crate::common::parallel::ParChunksMutAutoWithOffset;
use crate::star_detection::background::BackgroundMap;
use rayon::prelude::*;

#[cfg(target_arch = "x86_64")]
use crate::common::cpu_features;

/// Process a chunk of the threshold mask.
///
/// When `INCLUDE_BACKGROUND` is true:
///   Sets `mask[i] = true` where `pixels[i] > background[i] + sigma * noise[i]`.
///
/// When `INCLUDE_BACKGROUND` is false:
///   Sets `mask[i] = true` where `pixels[i] > sigma * noise[i]`.
#[inline]
fn process_chunk<const INCLUDE_BACKGROUND: bool>(
    pixels: &[f32],
    bg: &[f32],
    noise: &[f32],
    sigma_threshold: f32,
    mask: &mut [bool],
) {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        unsafe {
            neon::process_chunk_neon::<INCLUDE_BACKGROUND>(
                pixels,
                bg,
                noise,
                sigma_threshold,
                mask,
            );
        }
        return;
    }

    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_sse4_1() {
            // SAFETY: We've checked that SSE4.1 is available.
            unsafe {
                sse::process_chunk_sse::<INCLUDE_BACKGROUND>(
                    pixels,
                    bg,
                    noise,
                    sigma_threshold,
                    mask,
                );
            }
            return;
        }
    }

    scalar::process_chunk_scalar::<INCLUDE_BACKGROUND>(pixels, bg, noise, sigma_threshold, mask);
}

/// Internal dispatch to parallel SIMD or scalar implementation.
fn create_threshold_mask_impl<const INCLUDE_BACKGROUND: bool>(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    debug_assert_eq!(pixels.len(), mask.len());
    debug_assert_eq!(pixels.len(), background.background.len());
    debug_assert_eq!(pixels.len(), background.noise.len());

    let pixels_slice = pixels.pixels();
    let bg_slice = background.background.pixels();
    let noise_slice = background.noise.pixels();

    mask.pixels_mut()
        .par_chunks_mut_auto()
        .for_each(|(offset, mask_chunk)| {
            let end = offset + mask_chunk.len();
            let px_chunk = &pixels_slice[offset..end];
            let bg_chunk = &bg_slice[offset..end];
            let noise_chunk = &noise_slice[offset..end];

            process_chunk::<INCLUDE_BACKGROUND>(
                px_chunk,
                bg_chunk,
                noise_chunk,
                sigma_threshold,
                mask_chunk,
            );
        });
}

/// Create binary mask of pixels above threshold.
///
/// Sets `mask[i] = true` where `pixels[i] > background[i] + sigma * noise[i]`.
///
/// Uses SIMD acceleration when available (SSE4.1 on x86_64, NEON on aarch64).
pub fn create_threshold_mask(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    create_threshold_mask_impl::<true>(pixels, background, sigma_threshold, mask);
}

/// Create binary mask from a filtered (background-subtracted) image.
///
/// Sets `mask[i] = true` where `filtered[i] > sigma * noise[i]`.
/// Used for matched-filtered images where background is already subtracted.
pub fn create_threshold_mask_filtered(
    filtered: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    create_threshold_mask_impl::<false>(filtered, background, sigma_threshold, mask);
}

#[cfg(test)]
mod tests;
