//! NEON u16 -> normalized f32 conversion.

use std::arch::aarch64::*;

use crate::io::raw::normalize::normalize_one;

/// NEON SIMD normalization for aarch64.
pub(super) unsafe fn normalize_chunk_neon<const CLAMP: bool>(
    input: &[u16],
    output: &mut [f32],
    black: f32,
    inv_range: f32,
) {
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
