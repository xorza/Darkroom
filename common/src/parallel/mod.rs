//! Parallel processing utilities.

use rayon::prelude::*;

#[cfg(test)]
mod tests;

/// Multiplier for number of chunks relative to CPU threads.
/// Using 3x threads provides good load balancing when some chunks finish faster.
const CHUNKS_PER_THREAD: usize = 3;

/// Compute the optimal number of parallel jobs.
#[inline]
pub fn auto_num_jobs() -> usize {
    rayon::current_num_threads() * CHUNKS_PER_THREAD
}

/// Create a parallel iterator over job indices with automatic job count.
///
/// Returns an iterator yielding `(job_index, start, end)` tuples where:
/// - `job_index`: the index of this job (0..num_jobs)
/// - `start`: the starting index for this job's range
/// - `end`: the ending index (exclusive) for this job's range
///
/// Use this when you need to partition work into parallel jobs manually,
/// e.g., when working with raw pointers or complex data structures.
///
/// # Example
/// ```ignore
/// par_iter_auto(height).for_each(|(_, start_row, end_row)| {
///     for row in start_row..end_row {
///         // process row
///     }
/// });
/// ```
pub fn par_iter_auto(total: usize) -> impl IndexedParallelIterator<Item = (usize, usize, usize)> {
    let num_jobs = auto_num_jobs().min(total).max(1);
    let items_per_job = (total / num_jobs).max(1);

    (0..num_jobs).into_par_iter().map(move |job_idx| {
        let start = job_idx * items_per_job;
        let end = if job_idx == num_jobs - 1 {
            total
        } else {
            ((job_idx + 1) * items_per_job).min(total)
        };
        (job_idx, start, end)
    })
}

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
