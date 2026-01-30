//! SSE-accelerated threshold mask creation.

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use crate::common::Buffer2;
use crate::star_detection::background::BackgroundMap;

/// Number of SIMD vectors to process per unrolled iteration.
const UNROLL_FACTOR: usize = 4;
/// Floats per SSE vector.
const SSE_WIDTH: usize = 4;
/// Floats processed per unrolled iteration.
const UNROLL_WIDTH: usize = UNROLL_FACTOR * SSE_WIDTH;

/// SSE-accelerated threshold mask creation.
///
/// Sets `mask[i] = true` where `pixels[i] > background[i] + sigma * noise[i]`.
///
/// # Safety
/// Requires SSE4.1 support.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn create_threshold_mask_sse(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    let len = pixels.len();
    debug_assert_eq!(len, mask.len());

    let mask_ptr = mask.pixels_mut().as_mut_ptr();

    let sigma_vec = _mm_set1_ps(sigma_threshold);
    let min_noise_vec = _mm_set1_ps(1e-6);

    let pixels_ptr = pixels.as_ptr();
    let bg_ptr = background.background.as_ptr();
    let noise_ptr = background.noise.as_ptr();

    // Process 16 floats (4 SSE vectors) per unrolled iteration
    let unrolled_end = len - (len % UNROLL_WIDTH);
    let mut i = 0;

    while i < unrolled_end {
        // Load 4 vectors worth of data
        let px0 = _mm_loadu_ps(pixels_ptr.add(i));
        let px1 = _mm_loadu_ps(pixels_ptr.add(i + 4));
        let px2 = _mm_loadu_ps(pixels_ptr.add(i + 8));
        let px3 = _mm_loadu_ps(pixels_ptr.add(i + 12));

        let bg0 = _mm_loadu_ps(bg_ptr.add(i));
        let bg1 = _mm_loadu_ps(bg_ptr.add(i + 4));
        let bg2 = _mm_loadu_ps(bg_ptr.add(i + 8));
        let bg3 = _mm_loadu_ps(bg_ptr.add(i + 12));

        let noise0 = _mm_loadu_ps(noise_ptr.add(i));
        let noise1 = _mm_loadu_ps(noise_ptr.add(i + 4));
        let noise2 = _mm_loadu_ps(noise_ptr.add(i + 8));
        let noise3 = _mm_loadu_ps(noise_ptr.add(i + 12));

        // threshold = bg + sigma * noise.max(1e-6)
        let eff_noise0 = _mm_max_ps(noise0, min_noise_vec);
        let eff_noise1 = _mm_max_ps(noise1, min_noise_vec);
        let eff_noise2 = _mm_max_ps(noise2, min_noise_vec);
        let eff_noise3 = _mm_max_ps(noise3, min_noise_vec);

        let thresh0 = _mm_add_ps(bg0, _mm_mul_ps(sigma_vec, eff_noise0));
        let thresh1 = _mm_add_ps(bg1, _mm_mul_ps(sigma_vec, eff_noise1));
        let thresh2 = _mm_add_ps(bg2, _mm_mul_ps(sigma_vec, eff_noise2));
        let thresh3 = _mm_add_ps(bg3, _mm_mul_ps(sigma_vec, eff_noise3));

        // px > threshold
        let cmp0 = _mm_movemask_ps(_mm_cmpgt_ps(px0, thresh0));
        let cmp1 = _mm_movemask_ps(_mm_cmpgt_ps(px1, thresh1));
        let cmp2 = _mm_movemask_ps(_mm_cmpgt_ps(px2, thresh2));
        let cmp3 = _mm_movemask_ps(_mm_cmpgt_ps(px3, thresh3));

        // Write directly to mask slice
        *mask_ptr.add(i) = (cmp0 & 1) != 0;
        *mask_ptr.add(i + 1) = (cmp0 & 2) != 0;
        *mask_ptr.add(i + 2) = (cmp0 & 4) != 0;
        *mask_ptr.add(i + 3) = (cmp0 & 8) != 0;

        *mask_ptr.add(i + 4) = (cmp1 & 1) != 0;
        *mask_ptr.add(i + 5) = (cmp1 & 2) != 0;
        *mask_ptr.add(i + 6) = (cmp1 & 4) != 0;
        *mask_ptr.add(i + 7) = (cmp1 & 8) != 0;

        *mask_ptr.add(i + 8) = (cmp2 & 1) != 0;
        *mask_ptr.add(i + 9) = (cmp2 & 2) != 0;
        *mask_ptr.add(i + 10) = (cmp2 & 4) != 0;
        *mask_ptr.add(i + 11) = (cmp2 & 8) != 0;

        *mask_ptr.add(i + 12) = (cmp3 & 1) != 0;
        *mask_ptr.add(i + 13) = (cmp3 & 2) != 0;
        *mask_ptr.add(i + 14) = (cmp3 & 4) != 0;
        *mask_ptr.add(i + 15) = (cmp3 & 8) != 0;

        i += UNROLL_WIDTH;
    }

    // Process remaining full SSE vectors (0-3 vectors)
    while i + SSE_WIDTH <= len {
        let px_vec = _mm_loadu_ps(pixels_ptr.add(i));
        let bg_vec = _mm_loadu_ps(bg_ptr.add(i));
        let noise_vec = _mm_loadu_ps(noise_ptr.add(i));

        let effective_noise = _mm_max_ps(noise_vec, min_noise_vec);
        let threshold_vec = _mm_add_ps(bg_vec, _mm_mul_ps(sigma_vec, effective_noise));
        let bitmask = _mm_movemask_ps(_mm_cmpgt_ps(px_vec, threshold_vec));

        *mask_ptr.add(i) = (bitmask & 1) != 0;
        *mask_ptr.add(i + 1) = (bitmask & 2) != 0;
        *mask_ptr.add(i + 2) = (bitmask & 4) != 0;
        *mask_ptr.add(i + 3) = (bitmask & 8) != 0;

        i += SSE_WIDTH;
    }

    // Handle remaining elements (0-3 elements)
    while i < len {
        let threshold = background.background[i] + sigma_threshold * background.noise[i].max(1e-6);
        *mask_ptr.add(i) = pixels[i] > threshold;
        i += 1;
    }
}

/// SSE-accelerated threshold mask for filtered (background-subtracted) images.
///
/// Sets `mask[i] = true` where `filtered[i] > sigma * noise[i]`.
///
/// # Safety
/// Requires SSE4.1 support.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn create_threshold_mask_filtered_sse(
    filtered: &Buffer2<f32>,
    background: &BackgroundMap,
    sigma_threshold: f32,
    mask: &mut Buffer2<bool>,
) {
    let len = filtered.len();
    debug_assert_eq!(len, mask.len());

    let mask_ptr = mask.pixels_mut().as_mut_ptr();

    let sigma_vec = _mm_set1_ps(sigma_threshold);
    let min_noise_vec = _mm_set1_ps(1e-6);

    let filtered_ptr = filtered.as_ptr();
    let noise_ptr = background.noise.as_ptr();

    // Process 16 floats (4 SSE vectors) per unrolled iteration
    let unrolled_end = len - (len % UNROLL_WIDTH);
    let mut i = 0;

    while i < unrolled_end {
        // Load 4 vectors worth of data
        let px0 = _mm_loadu_ps(filtered_ptr.add(i));
        let px1 = _mm_loadu_ps(filtered_ptr.add(i + 4));
        let px2 = _mm_loadu_ps(filtered_ptr.add(i + 8));
        let px3 = _mm_loadu_ps(filtered_ptr.add(i + 12));

        let noise0 = _mm_loadu_ps(noise_ptr.add(i));
        let noise1 = _mm_loadu_ps(noise_ptr.add(i + 4));
        let noise2 = _mm_loadu_ps(noise_ptr.add(i + 8));
        let noise3 = _mm_loadu_ps(noise_ptr.add(i + 12));

        // threshold = sigma * noise.max(1e-6)
        let thresh0 = _mm_mul_ps(sigma_vec, _mm_max_ps(noise0, min_noise_vec));
        let thresh1 = _mm_mul_ps(sigma_vec, _mm_max_ps(noise1, min_noise_vec));
        let thresh2 = _mm_mul_ps(sigma_vec, _mm_max_ps(noise2, min_noise_vec));
        let thresh3 = _mm_mul_ps(sigma_vec, _mm_max_ps(noise3, min_noise_vec));

        // px > threshold
        let cmp0 = _mm_movemask_ps(_mm_cmpgt_ps(px0, thresh0));
        let cmp1 = _mm_movemask_ps(_mm_cmpgt_ps(px1, thresh1));
        let cmp2 = _mm_movemask_ps(_mm_cmpgt_ps(px2, thresh2));
        let cmp3 = _mm_movemask_ps(_mm_cmpgt_ps(px3, thresh3));

        // Write directly to mask slice
        *mask_ptr.add(i) = (cmp0 & 1) != 0;
        *mask_ptr.add(i + 1) = (cmp0 & 2) != 0;
        *mask_ptr.add(i + 2) = (cmp0 & 4) != 0;
        *mask_ptr.add(i + 3) = (cmp0 & 8) != 0;

        *mask_ptr.add(i + 4) = (cmp1 & 1) != 0;
        *mask_ptr.add(i + 5) = (cmp1 & 2) != 0;
        *mask_ptr.add(i + 6) = (cmp1 & 4) != 0;
        *mask_ptr.add(i + 7) = (cmp1 & 8) != 0;

        *mask_ptr.add(i + 8) = (cmp2 & 1) != 0;
        *mask_ptr.add(i + 9) = (cmp2 & 2) != 0;
        *mask_ptr.add(i + 10) = (cmp2 & 4) != 0;
        *mask_ptr.add(i + 11) = (cmp2 & 8) != 0;

        *mask_ptr.add(i + 12) = (cmp3 & 1) != 0;
        *mask_ptr.add(i + 13) = (cmp3 & 2) != 0;
        *mask_ptr.add(i + 14) = (cmp3 & 4) != 0;
        *mask_ptr.add(i + 15) = (cmp3 & 8) != 0;

        i += UNROLL_WIDTH;
    }

    // Process remaining full SSE vectors (0-3 vectors)
    while i + SSE_WIDTH <= len {
        let px_vec = _mm_loadu_ps(filtered_ptr.add(i));
        let noise_vec = _mm_loadu_ps(noise_ptr.add(i));

        let effective_noise = _mm_max_ps(noise_vec, min_noise_vec);
        let threshold_vec = _mm_mul_ps(sigma_vec, effective_noise);
        let bitmask = _mm_movemask_ps(_mm_cmpgt_ps(px_vec, threshold_vec));

        *mask_ptr.add(i) = (bitmask & 1) != 0;
        *mask_ptr.add(i + 1) = (bitmask & 2) != 0;
        *mask_ptr.add(i + 2) = (bitmask & 4) != 0;
        *mask_ptr.add(i + 3) = (bitmask & 8) != 0;

        i += SSE_WIDTH;
    }

    // Handle remaining elements (0-3 elements)
    while i < len {
        let threshold = sigma_threshold * background.noise[i].max(1e-6);
        *mask_ptr.add(i) = filtered[i] > threshold;
        i += 1;
    }
}
