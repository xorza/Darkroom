//! Parallel processing utilities.

use rayon::prelude::*;

#[cfg(test)]
mod tests;

/// Multiplier for number of chunks relative to CPU threads.
/// Using 3x threads provides good load balancing when some chunks finish faster.
const CHUNKS_PER_THREAD: usize = 3;

/// Compute optimal chunk size for the given length.
#[inline]
fn auto_chunk_size(len: usize) -> usize {
    let num_chunks = rayon::current_num_threads() * CHUNKS_PER_THREAD;
    (len / num_chunks).max(1)
}

/// Split a mutable slice into parallel chunks with automatic sizing.
/// Returns an iterator yielding `(start_offset, chunk)` pairs.
///
/// Use this for flat buffer processing where you need the original index.
pub fn par_chunks_auto<T: Send>(
    data: &mut [T],
) -> impl IndexedParallelIterator<Item = (usize, &mut [T])> {
    let chunk_size = auto_chunk_size(data.len());
    data.par_chunks_mut(chunk_size)
        .enumerate()
        .map(move |(idx, chunk)| (idx * chunk_size, chunk))
}

/// Split a mutable slice into parallel chunks aligned to row boundaries.
/// Returns an iterator yielding `(chunk_start_row, chunk)` pairs where chunk contains complete rows.
///
/// Use this for 2D image processing where chunks must align to row boundaries.
pub fn par_chunks_auto_aligned<T: Send>(
    data: &mut [T],
    width: usize,
) -> impl IndexedParallelIterator<Item = (usize, &mut [T])> {
    let height = data.len() / width;
    let chunk_rows = auto_chunk_size(height);
    let chunk_size = width * chunk_rows;
    data.par_chunks_mut(chunk_size)
        .enumerate()
        .map(move |(idx, chunk)| (idx * chunk_rows, chunk))
}

/// Split two mutable slices into parallel chunks aligned to row boundaries.
/// Returns an iterator yielding `(chunk_start_row, (chunk_a, chunk_b))` pairs.
///
/// Both slices must have equal length.
pub fn par_chunks_auto_aligned_zip2<'a, A: Send, B: Send>(
    a: &'a mut [A],
    b: &'a mut [B],
    width: usize,
) -> impl IndexedParallelIterator<Item = (usize, (&'a mut [A], &'a mut [B]))> {
    assert_eq!(a.len(), b.len(), "Zipped slices must have equal length");
    let height = a.len() / width;
    let chunk_rows = auto_chunk_size(height);
    let chunk_size = width * chunk_rows;
    a.par_chunks_mut(chunk_size)
        .zip(b.par_chunks_mut(chunk_size))
        .enumerate()
        .map(move |(idx, chunks)| (idx * chunk_rows, chunks))
}
