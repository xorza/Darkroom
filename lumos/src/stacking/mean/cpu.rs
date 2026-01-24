//! CPU dispatch for mean stacking.

use rayon::prelude::*;

/// Accumulate src into dst in parallel chunks.
#[inline]
pub(super) fn accumulate_parallel(dst: &mut [f32], src: &[f32], chunk_size: usize) {
    dst.par_chunks_mut(chunk_size)
        .zip(src.par_chunks(chunk_size))
        .for_each(|(dst_chunk, src_chunk)| {
            super::scalar::accumulate_chunk(dst_chunk, src_chunk);
        });
}

/// Divide all values by a scalar in parallel.
#[inline]
pub(super) fn divide_parallel(data: &mut [f32], inv_count: f32, chunk_size: usize) {
    data.par_chunks_mut(chunk_size).for_each(|chunk| {
        super::scalar::divide_chunk(chunk, inv_count);
    });
}
