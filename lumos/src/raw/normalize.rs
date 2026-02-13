use rayon::prelude::*;

/// Normalize u16 raw data to f32 in [0.0, 1.0] range using parallel SIMD processing.
/// Formula: clamp(((value - black).max(0) * inv_range), 0.0, 1.0)
pub(crate) fn normalize_u16_to_f32_parallel(data: &[u16], black: f32, inv_range: f32) -> Vec<f32> {
    const CHUNK_SIZE: usize = 16384; // Process 64KB chunks (16K * 4 bytes)

    // SAFETY: Every element is written by the parallel SIMD pass below before being read.
    let mut result = unsafe { super::alloc_uninit_vec::<f32>(data.len()) };

    result
        .par_chunks_mut(CHUNK_SIZE)
        .zip(data.par_chunks(CHUNK_SIZE))
        .for_each(|(out_chunk, in_chunk)| {
            normalize_chunk_simd(in_chunk, out_chunk, black, inv_range);
        });

    result
}

/// Normalize a chunk of u16 data to f32 using SIMD when available.
#[inline]
fn normalize_chunk_simd(input: &[u16], output: &mut [f32], black: f32, inv_range: f32) {
    #[cfg(target_arch = "x86_64")]
    {
        // Prefer SSE4.1 for faster u16->i32 conversion (pmovzxwd)
        if common::cpu_features::has_sse4_1() {
            // SAFETY: We've verified SSE4.1 is available
            unsafe {
                normalize_chunk_sse41(input, output, black, inv_range);
            }
        } else if common::cpu_features::has_sse2() {
            // SAFETY: We've verified SSE2 is available
            unsafe {
                normalize_chunk_sse2(input, output, black, inv_range);
            }
        } else {
            for (out, &val) in output.iter_mut().zip(input.iter()) {
                *out = (((val as f32) - black).max(0.0) * inv_range).min(1.0);
            }
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64
        unsafe {
            normalize_chunk_neon(input, output, black, inv_range);
        }
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        // Scalar fallback for other architectures
        normalize_chunk_scalar(input, output, black, inv_range);
    }
}

/// Scalar normalization fallback.
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
#[inline]
fn normalize_chunk_scalar(input: &[u16], output: &mut [f32], black: f32, inv_range: f32) {
    for (out, &val) in output.iter_mut().zip(input.iter()) {
        *out = (((val as f32) - black).max(0.0) * inv_range).min(1.0);
    }
}

/// SSE4.1 SIMD normalization for x86_64 (fast path with pmovzxwd).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn normalize_chunk_sse41(input: &[u16], output: &mut [f32], black: f32, inv_range: f32) {
    use std::arch::x86_64::*;

    // SAFETY: All operations require SSE4.1, guaranteed by target_feature
    unsafe {
        let black_vec = _mm_set1_ps(black);
        let inv_range_vec = _mm_set1_ps(inv_range);
        let zero_vec = _mm_setzero_ps();
        let one_vec = _mm_set1_ps(1.0);

        let chunks = input.len() / 4;
        let remainder = input.len() % 4;

        for i in 0..chunks {
            let idx = i * 4;
            // Load 4 u16 values (64 bits) and zero-extend to 4 i32 values using SSE4.1
            let vals_u16 = _mm_loadl_epi64(input.as_ptr().add(idx) as *const __m128i);
            let vals_i32 = _mm_cvtepu16_epi32(vals_u16);
            let vals_f32 = _mm_cvtepi32_ps(vals_i32);

            // Subtract black, clamp to [0, 1], multiply by inv_range
            let subtracted = _mm_sub_ps(vals_f32, black_vec);
            let clamped = _mm_max_ps(subtracted, zero_vec);
            let normalized = _mm_mul_ps(clamped, inv_range_vec);
            let clamped_upper = _mm_min_ps(normalized, one_vec);

            // Store result
            _mm_storeu_ps(output.as_mut_ptr().add(idx), clamped_upper);
        }

        // Handle remainder with scalar
        let start = chunks * 4;
        for i in 0..remainder {
            let idx = start + i;
            output[idx] = (((input[idx] as f32) - black).max(0.0) * inv_range).min(1.0);
        }
    }
}

/// SSE2 SIMD normalization for x86_64 (fallback without SSE4.1).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn normalize_chunk_sse2(input: &[u16], output: &mut [f32], black: f32, inv_range: f32) {
    use std::arch::x86_64::*;

    // SAFETY: All operations require SSE2, guaranteed by target_feature
    unsafe {
        let black_vec = _mm_set1_ps(black);
        let inv_range_vec = _mm_set1_ps(inv_range);
        let zero_vec = _mm_setzero_ps();
        let one_vec = _mm_set1_ps(1.0);

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

            // Subtract black, clamp to [0, 1], multiply by inv_range
            let subtracted = _mm_sub_ps(vals_f32, black_vec);
            let clamped = _mm_max_ps(subtracted, zero_vec);
            let normalized = _mm_mul_ps(clamped, inv_range_vec);
            let clamped_upper = _mm_min_ps(normalized, one_vec);

            // Store result
            _mm_storeu_ps(output.as_mut_ptr().add(idx), clamped_upper);
        }

        // Handle remainder with scalar
        let start = chunks * 4;
        for i in 0..remainder {
            let idx = start + i;
            output[idx] = (((input[idx] as f32) - black).max(0.0) * inv_range).min(1.0);
        }
    }
}

/// NEON SIMD normalization for aarch64.
#[cfg(target_arch = "aarch64")]
unsafe fn normalize_chunk_neon(input: &[u16], output: &mut [f32], black: f32, inv_range: f32) {
    use std::arch::aarch64::*;

    // SAFETY: All NEON intrinsics in this function are safe because:
    // - NEON is guaranteed available on aarch64
    // - We validate array bounds before accessing memory
    // - All pointer arithmetic stays within bounds of the slices
    unsafe {
        let black_vec = vdupq_n_f32(black);
        let inv_range_vec = vdupq_n_f32(inv_range);
        let zero_vec = vdupq_n_f32(0.0);
        let one_vec = vdupq_n_f32(1.0);

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

            // Subtract black, clamp to [0, 1], multiply by inv_range
            let subtracted = vsubq_f32(vals_f32, black_vec);
            let clamped = vmaxq_f32(subtracted, zero_vec);
            let normalized = vmulq_f32(clamped, inv_range_vec);
            let clamped_upper = vminq_f32(normalized, one_vec);

            // Store result
            vst1q_f32(output.as_mut_ptr().add(idx), clamped_upper);
        }

        // Handle remainder with scalar
        let start = chunks * 4;
        for i in 0..remainder {
            let idx = start + i;
            output[idx] = (((input[idx] as f32) - black).max(0.0) * inv_range).min(1.0);
        }
    }
}
