use rayon::prelude::*;

use crate::io::raw::alloc_uninit_vec;

/// Light-frame normalization: `clamp((value - black).max(0) * inv_range, 0, 1)`.
/// This bounds direct RAW sensor input; demosaic interpolation itself remains unclipped.
pub(crate) fn normalize_u16_to_f32_parallel(data: &[u16], black: f32, inv_range: f32) -> Vec<f32> {
    normalize_generic::<true>(data, black, inv_range)
}

/// Shared parallel driver. `CLAMP` is a compile-time switch so each variant
/// monomorphizes to branch-free SIMD — the light path keeps its `[0, 1]` clamp,
/// the calibration path drops it, with no duplicated kernel.
fn normalize_generic<const CLAMP: bool>(data: &[u16], black: f32, inv_range: f32) -> Vec<f32> {
    const CHUNK_SIZE: usize = 16384; // Process 64KB chunks (16K * 4 bytes)

    // SAFETY: Every element is written by the parallel SIMD pass below before being read.
    let mut result = unsafe { alloc_uninit_vec::<f32>(data.len()) };

    result
        .par_chunks_mut(CHUNK_SIZE)
        .zip(data.par_chunks(CHUNK_SIZE))
        .for_each(|(out_chunk, in_chunk)| {
            normalize_u16_to_f32_into::<CLAMP>(in_chunk, out_chunk, black, inv_range);
        });

    result
}

/// Scalar form of the per-pixel transform, shared by the fallback and the SIMD
/// remainders. `CLAMP` gates the `[0, 1]` floor/ceil.
#[inline(always)]
fn normalize_one<const CLAMP: bool>(val: u16, black: f32, inv_range: f32) -> f32 {
    let subtracted = (val as f32) - black;
    if CLAMP {
        (subtracted.max(0.0) * inv_range).min(1.0)
    } else {
        subtracted * inv_range
    }
}

/// Normalize `input` directly into equally sized caller-owned storage.
#[inline]
pub(crate) fn normalize_u16_to_f32_into<const CLAMP: bool>(
    input: &[u16],
    output: &mut [f32],
    black: f32,
    inv_range: f32,
) {
    debug_assert_eq!(input.len(), output.len());
    #[cfg(target_arch = "x86_64")]
    {
        // Prefer SSE4.1 for faster u16->i32 conversion (pmovzxwd)
        if imaginarium::cpu_features::has_sse4_1() {
            // SAFETY: We've verified SSE4.1 is available
            unsafe {
                normalize_chunk_sse41::<CLAMP>(input, output, black, inv_range);
            }
        } else if imaginarium::cpu_features::has_sse2() {
            // SAFETY: We've verified SSE2 is available
            unsafe {
                normalize_chunk_sse2::<CLAMP>(input, output, black, inv_range);
            }
        } else {
            for (out, &val) in output.iter_mut().zip(input.iter()) {
                *out = normalize_one::<CLAMP>(val, black, inv_range);
            }
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64
        unsafe {
            normalize_chunk_neon::<CLAMP>(input, output, black, inv_range);
        }
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        // Scalar fallback for other architectures
        for (out, &val) in output.iter_mut().zip(input.iter()) {
            *out = normalize_one::<CLAMP>(val, black, inv_range);
        }
    }
}

/// SSE4.1 SIMD normalization for x86_64 (fast path with pmovzxwd).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn normalize_chunk_sse41<const CLAMP: bool>(
    input: &[u16],
    output: &mut [f32],
    black: f32,
    inv_range: f32,
) {
    use std::arch::x86_64::*;

    // SAFETY: All operations require SSE4.1, guaranteed by target_feature
    unsafe {
        let black_vec = _mm_set1_ps(black);
        let inv_range_vec = _mm_set1_ps(inv_range);

        let chunks = input.len() / 4;
        let remainder = input.len() % 4;

        for i in 0..chunks {
            let idx = i * 4;
            // Load 4 u16 values (64 bits) and zero-extend to 4 i32 values using SSE4.1
            let vals_u16 = _mm_loadl_epi64(input.as_ptr().add(idx) as *const __m128i);
            let vals_i32 = _mm_cvtepu16_epi32(vals_u16);
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

/// SSE2 SIMD normalization for x86_64 (fallback without SSE4.1).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn normalize_chunk_sse2<const CLAMP: bool>(
    input: &[u16],
    output: &mut [f32],
    black: f32,
    inv_range: f32,
) {
    use std::arch::x86_64::*;

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

/// NEON SIMD normalization for aarch64.
#[cfg(target_arch = "aarch64")]
unsafe fn normalize_chunk_neon<const CLAMP: bool>(
    input: &[u16],
    output: &mut [f32],
    black: f32,
    inv_range: f32,
) {
    use std::arch::aarch64::*;

    // SAFETY: All NEON intrinsics in this function are safe because:
    // - NEON is guaranteed available on aarch64
    // - We validate array bounds before accessing memory
    // - All pointer arithmetic stays within bounds of the slices
    unsafe {
        let black_vec = vdupq_n_f32(black);
        let inv_range_vec = vdupq_n_f32(inv_range);

        let chunks = input.len() / 4;
        let remainder = input.len() % 4;

        for i in 0..chunks {
            let idx = i * 4;
            // Load 4 u16 values
            let vals_u16 = vld1_u16(input.as_ptr().add(idx));
            // Widen to u32
            let vals_u32 = vmovl_u16(vals_u16);
            // Convert to f32
            let vals_f32 = vcvtq_f32_u32(vals_u32);

            // Subtract black; optionally floor at 0, scale, optionally cap at 1.
            let subtracted = vsubq_f32(vals_f32, black_vec);
            let floored = if CLAMP {
                vmaxq_f32(subtracted, vdupq_n_f32(0.0))
            } else {
                subtracted
            };
            let normalized = vmulq_f32(floored, inv_range_vec);
            let result = if CLAMP {
                vminq_f32(normalized, vdupq_n_f32(1.0))
            } else {
                normalized
            };

            // Store result
            vst1q_f32(output.as_mut_ptr().add(idx), result);
        }

        // Handle remainder with scalar
        let start = chunks * 4;
        for i in 0..remainder {
            let idx = start + i;
            output[idx] = normalize_one::<CLAMP>(input[idx], black, inv_range);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::io::raw::normalize::*;

    /// Pure-scalar reference for cross-checking the SIMD kernels.
    fn scalar_ref<const CLAMP: bool>(data: &[u16], black: f32, inv_range: f32) -> Vec<f32> {
        data.iter()
            .map(|&v| normalize_one::<CLAMP>(v, black, inv_range))
            .collect()
    }

    #[test]
    fn simd_matches_scalar() {
        // The dispatched kernel (NEON on aarch64, SSE4.1/SSE2 on x86) uses the same IEEE ops as
        // the scalar form, so results must be bit-identical for both the clamped (light) and
        // unclamped (calibration) paths. Values span zero, below-black, at-black, mid, max, and
        // above-max; the length is deliberately not a multiple of 4 so the remainder path runs.
        let black = 512.0;
        let inv_range = 1.0 / (16383.0 - 512.0);
        let mut data: Vec<u16> = vec![
            0, 1, 256, 511, 512, 513, 1000, 8191, 16383, 16384, 60000, 65535,
        ];
        for i in 0..39u16 {
            data.push(i.wrapping_mul(421));
        }
        assert!(
            !data.len().is_multiple_of(4),
            "length must exercise the SIMD remainder path"
        );

        let mut simd_clamped = vec![0.0f32; data.len()];
        normalize_u16_to_f32_into::<true>(&data, &mut simd_clamped, black, inv_range);
        assert_eq!(
            simd_clamped,
            scalar_ref::<true>(&data, black, inv_range),
            "clamped SIMD must match scalar"
        );

        let mut simd_unclamped = vec![0.0f32; data.len()];
        normalize_u16_to_f32_into::<false>(&data, &mut simd_unclamped, black, inv_range);
        assert_eq!(
            simd_unclamped,
            scalar_ref::<false>(&data, black, inv_range),
            "unclamped SIMD must match scalar"
        );
    }
}
