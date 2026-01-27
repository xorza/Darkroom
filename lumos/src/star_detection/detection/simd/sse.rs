//! SSE-accelerated thresholding.
use std::arch::x86_64::*;

use super::super::scalar;
use crate::star_detection::background::BackgroundMap;

#[target_feature(enable = "sse4.1")]
pub unsafe fn create_threshold_mask_sse(
    pixels: &[f32],
    background: &BackgroundMap,
    sigma_threshold: f32,
) -> Vec<bool> {
    let mut mask = Vec::with_capacity(pixels.len());

    let chunk_size = 4;
    let num_chunks = pixels.len() / chunk_size;
    let remainder_start = num_chunks * chunk_size;

    let sigma_vec = _mm_set1_ps(sigma_threshold);
    let min_noise_vec = _mm_set1_ps(1e-6);

    let pixel_chunks = pixels[..remainder_start].chunks_exact(chunk_size);
    let bg_chunks = background.background[..remainder_start].chunks_exact(chunk_size);
    let noise_chunks = background.noise[..remainder_start].chunks_exact(chunk_size);

    for ((px_chunk, bg_chunk), noise_chunk) in pixel_chunks.zip(bg_chunks).zip(noise_chunks) {
        let px_vec = unsafe { _mm_loadu_ps(px_chunk.as_ptr()) };
        let bg_vec = unsafe { _mm_loadu_ps(bg_chunk.as_ptr()) };
        let noise_vec = unsafe { _mm_loadu_ps(noise_chunk.as_ptr()) };

        // threshold = bg + sigma * noise.max(1e-6)
        let effective_noise = _mm_max_ps(noise_vec, min_noise_vec);
        let threshold_vec = _mm_add_ps(bg_vec, _mm_mul_ps(sigma_vec, effective_noise));

        // px > threshold
        let cmp_mask = _mm_cmpgt_ps(px_vec, threshold_vec);

        // Convert SIMD mask to integer bitmask
        let bitmask = _mm_movemask_ps(cmp_mask);

        // Append booleans based on bitmask
        mask.push((bitmask & 1) != 0);
        mask.push((bitmask & 2) != 0);
        mask.push((bitmask & 4) != 0);
        mask.push((bitmask & 8) != 0);
    }

    // Handle remainder using the scalar slice function
    if remainder_start < pixels.len() {
        scalar::create_threshold_mask_slice(
            &pixels[remainder_start..],
            background,
            sigma_threshold,
            remainder_start,
            &mut mask,
        );
    }

    mask
}
