//! CPU dispatch for mean stacking with SIMD support.

#[cfg(not(target_arch = "aarch64"))]
use super::scalar;

use rayon::prelude::*;

/// Accumulate src into dst in parallel chunks.
#[inline]
pub(super) fn accumulate_parallel(dst: &mut [f32], src: &[f32], chunk_size: usize) {
    dst.par_chunks_mut(chunk_size)
        .zip(src.par_chunks(chunk_size))
        .for_each(|(dst_chunk, src_chunk)| {
            accumulate_chunk(dst_chunk, src_chunk);
        });
}

/// Divide all values by a scalar in parallel.
#[inline]
pub(super) fn divide_parallel(data: &mut [f32], inv_count: f32, chunk_size: usize) {
    data.par_chunks_mut(chunk_size).for_each(|chunk| {
        divide_chunk(chunk, inv_count);
    });
}

/// Accumulate a chunk, dispatching to best available implementation.
#[inline]
fn accumulate_chunk(dst: &mut [f32], src: &[f32]) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        super::neon::accumulate_chunk(dst, src)
    }

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("sse2") {
            unsafe { super::sse::accumulate_chunk(dst, src) };
        } else {
            scalar::accumulate_chunk(dst, src);
        }
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        scalar::accumulate_chunk(dst, src);
    }
}

/// Divide a chunk, dispatching to best available implementation.
#[inline]
fn divide_chunk(data: &mut [f32], inv_count: f32) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        super::neon::divide_chunk(data, inv_count)
    }

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("sse2") {
            unsafe { super::sse::divide_chunk(data, inv_count) };
        } else {
            scalar::divide_chunk(data, inv_count);
        }
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        scalar::divide_chunk(data, inv_count);
    }
}
