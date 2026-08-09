//! Vector backends for the u16 -> normalized f32 conversion, and the dispatch between them.

use crate::io::raw::normalize::normalize_one;
use crate::simd::dispatch;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod sse2;

#[cfg(target_arch = "x86_64")]
mod sse41;

/// Normalize one chunk, vectorized where the target allows.
///
/// SSE4.1 leads SSE2 for its faster u16->i32 conversion (`pmovzxwd`).
#[inline]
pub(super) fn normalize_chunk<const CLAMP: bool>(
    input: &[u16],
    output: &mut [f32],
    black: f32,
    inv_range: f32,
) {
    dispatch! {
        x86: sse4_1 => sse41::normalize_chunk_sse41::<CLAMP>(input, output, black, inv_range),
        x86: sse2 => sse2::normalize_chunk_sse2::<CLAMP>(input, output, black, inv_range),
        aarch64 => neon::normalize_chunk_neon::<CLAMP>(input, output, black, inv_range),
        scalar => normalize_chunk_scalar::<CLAMP>(input, output, black, inv_range),
    }
}

/// Scalar form of the whole chunk, for architectures with no backend of their own.
#[inline]
fn normalize_chunk_scalar<const CLAMP: bool>(
    input: &[u16],
    output: &mut [f32],
    black: f32,
    inv_range: f32,
) {
    for (out, &val) in output.iter_mut().zip(input.iter()) {
        *out = normalize_one::<CLAMP>(val, black, inv_range);
    }
}
