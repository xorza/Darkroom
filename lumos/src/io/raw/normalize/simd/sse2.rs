//! SSE2 u16 -> normalized f32 conversion.

use std::arch::x86_64::*;

use crate::io::raw::normalize::normalize_one;

/// SSE2 SIMD normalization for x86_64 (fallback without SSE4.1).
#[target_feature(enable = "sse2")]
pub(super) unsafe fn normalize_chunk_sse2<const CLAMP: bool>(
    input: &[u16],
    output: &mut [f32],
    black: f32,
    inv_range: f32,
) {
    // SAFETY: All operations require SSE2, guaranteed by target_feature
    unsafe {
        let black_vec = _mm_set1_ps(black);
        let inv_range_vec = _mm_set1_ps(inv_range);

        let chunks = input.len() / 4;
        let remainder = input.len() % 4;

        for i in 0..chunks {
            let idx = i * 4;
            // Load 4 u16 values (64 bits) and unpack to i32 using SSE2
            // _mm_loadl_epi64 loads 64 bits into lower half, zeros upper half
            let vals_u16 = _mm_loadl_epi64(input.as_ptr().add(idx) as *const __m128i);
            // Unpack low 16-bit integers to 32-bit by interleaving with zeros
            let vals_i32 = _mm_unpacklo_epi16(vals_u16, _mm_setzero_si128());
            let vals_f32 = _mm_cvtepi32_ps(vals_i32);

            // Subtract black; optionally floor at 0, scale, optionally cap at 1.
            let subtracted = _mm_sub_ps(vals_f32, black_vec);
            let floored = if CLAMP {
                _mm_max_ps(subtracted, _mm_setzero_ps())
            } else {
                subtracted
            };
            let normalized = _mm_mul_ps(floored, inv_range_vec);
            let result = if CLAMP {
                _mm_min_ps(normalized, _mm_set1_ps(1.0))
            } else {
                normalized
            };

            // Store result
            _mm_storeu_ps(output.as_mut_ptr().add(idx), result);
        }

        // Handle remainder with scalar
        let start = chunks * 4;
        for i in 0..remainder {
            let idx = start + i;
            output[idx] = normalize_one::<CLAMP>(input[idx], black, inv_range);
        }
    }
}
