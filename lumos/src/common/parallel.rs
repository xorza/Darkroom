//! Parallel processing utilities.

use rayon::prelude::*;

/// Multiplier for number of chunks relative to CPU threads.
/// Using 3x threads provides good load balancing when some chunks finish faster.
const CHUNKS_PER_THREAD: usize = 3;

/// Compute optimal chunk size for the given length.
#[inline]
fn auto_chunk_size(len: usize) -> usize {
    let num_chunks = rayon::current_num_threads() * CHUNKS_PER_THREAD;
    (len / num_chunks).max(1)
}

/// Compute optimal rows per chunk for parallel image processing.
#[inline]
pub fn rows_per_chunk(height: usize) -> usize {
    auto_chunk_size(height)
}

/// Extension trait for row-aligned mutable parallel chunks with automatic sizing.
pub trait ParRowsMutAuto<'a, T: Send + 'a> {
    type Iter: IndexedParallelIterator;

    /// Split into mutable parallel chunks aligned to row boundaries.
    /// Returns an iterator yielding `(y_start, chunk)` pairs where chunk contains complete rows.
    fn par_rows_mut_auto(&'a mut self, width: usize) -> Self::Iter;
}

impl<'a, T: Send + 'a> ParRowsMutAuto<'a, T> for [T] {
    type Iter = ParRowsMutWithOffset<'a, T>;

    fn par_rows_mut_auto(&'a mut self, width: usize) -> ParRowsMutWithOffset<'a, T> {
        let height = self.len() / width;
        let chunk_rows = auto_chunk_size(height);
        ParRowsMutWithOffset {
            inner: self.par_chunks_mut(width * chunk_rows),
            chunk_rows,
        }
    }
}

/// Wrapper for zipping two mutable slices before applying `par_rows_mut_auto`.
pub struct Zip2Mut<'a, A: Send, B: Send>(pub &'a mut [A], pub &'a mut [B]);

impl<'a, A: Send + 'a, B: Send + 'a> ParRowsMutAuto<'a, (A, B)> for Zip2Mut<'a, A, B> {
    type Iter = rayon::iter::Zip<ParRowsMutWithOffset<'a, A>, ParRowsMutWithOffset<'a, B>>;

    fn par_rows_mut_auto(&'a mut self, width: usize) -> Self::Iter {
        let height = self.0.len() / width;
        let chunk_rows = auto_chunk_size(height);
        let iter_a = ParRowsMutWithOffset {
            inner: self.0.par_chunks_mut(width * chunk_rows),
            chunk_rows,
        };
        let iter_b = ParRowsMutWithOffset {
            inner: self.1.par_chunks_mut(width * chunk_rows),
            chunk_rows,
        };
        iter_a.zip(iter_b)
    }
}

/// Parallel iterator over row-aligned mutable chunks that yields `(y_start, chunk)` pairs.
pub struct ParRowsMutWithOffset<'a, T: Send> {
    inner: rayon::slice::ChunksMut<'a, T>,
    chunk_rows: usize,
}

impl<'a, T: Send + 'a> ParallelIterator for ParRowsMutWithOffset<'a, T> {
    type Item = (usize, &'a mut [T]);

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: rayon::iter::plumbing::UnindexedConsumer<Self::Item>,
    {
        let chunk_rows = self.chunk_rows;
        self.inner
            .enumerate()
            .map(|(idx, chunk)| (idx * chunk_rows, chunk))
            .drive_unindexed(consumer)
    }
}

impl<'a, T: Send + 'a> IndexedParallelIterator for ParRowsMutWithOffset<'a, T> {
    fn len(&self) -> usize {
        self.inner.len()
    }

    fn drive<C>(self, consumer: C) -> C::Result
    where
        C: rayon::iter::plumbing::Consumer<Self::Item>,
    {
        let chunk_rows = self.chunk_rows;
        self.inner
            .enumerate()
            .map(|(idx, chunk)| (idx * chunk_rows, chunk))
            .drive(consumer)
    }

    fn with_producer<CB>(self, callback: CB) -> CB::Output
    where
        CB: rayon::iter::plumbing::ProducerCallback<Self::Item>,
    {
        let chunk_rows = self.chunk_rows;
        self.inner
            .enumerate()
            .map(|(idx, chunk)| (idx * chunk_rows, chunk))
            .with_producer(callback)
    }
}

/// Extension trait for mutable parallel chunks with automatic sizing and offsets.
pub trait ParChunksMutAutoWithOffset<'a, T: Send + 'a> {
    /// Split into mutable parallel chunks with automatic sizing.
    /// Returns an iterator yielding `(start_offset, chunk)` pairs.
    fn par_chunks_mut_auto(&'a mut self) -> ParChunksMutWithOffset<'a, T>;
}

impl<'a, T: Send + 'a> ParChunksMutAutoWithOffset<'a, T> for [T] {
    fn par_chunks_mut_auto(&'a mut self) -> ParChunksMutWithOffset<'a, T> {
        let chunk_size = auto_chunk_size(self.len());
        ParChunksMutWithOffset {
            inner: self.par_chunks_mut(chunk_size),
            chunk_size,
        }
    }
}

/// Parallel iterator over mutable chunks that yields `(offset, chunk)` pairs.
pub struct ParChunksMutWithOffset<'a, T: Send> {
    inner: rayon::slice::ChunksMut<'a, T>,
    chunk_size: usize,
}

impl<'a, T: Send + 'a> ParallelIterator for ParChunksMutWithOffset<'a, T> {
    type Item = (usize, &'a mut [T]);

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: rayon::iter::plumbing::UnindexedConsumer<Self::Item>,
    {
        let chunk_size = self.chunk_size;
        self.inner
            .enumerate()
            .map(|(idx, chunk)| (idx * chunk_size, chunk))
            .drive_unindexed(consumer)
    }
}

impl<'a, T: Send + 'a> IndexedParallelIterator for ParChunksMutWithOffset<'a, T> {
    fn len(&self) -> usize {
        self.inner.len()
    }

    fn drive<C>(self, consumer: C) -> C::Result
    where
        C: rayon::iter::plumbing::Consumer<Self::Item>,
    {
        let chunk_size = self.chunk_size;
        self.inner
            .enumerate()
            .map(|(idx, chunk)| (idx * chunk_size, chunk))
            .drive(consumer)
    }

    fn with_producer<CB>(self, callback: CB) -> CB::Output
    where
        CB: rayon::iter::plumbing::ProducerCallback<Self::Item>,
    {
        let chunk_size = self.chunk_size;
        self.inner
            .enumerate()
            .map(|(idx, chunk)| (idx * chunk_size, chunk))
            .with_producer(callback)
    }
}

/// Apply a function to each index in parallel, modifying the slice in place.
///
/// # Arguments
/// * `data` - Mutable slice to fill with values
/// * `f` - Function that takes an index and returns a value
/// todo remove
pub fn parallel_chunked<T, F>(data: &mut [T], f: F)
where
    T: Send + Sync,
    F: Fn(usize) -> T + Sync + Send,
{
    if data.is_empty() {
        return;
    }

    data.par_chunks_mut_auto().for_each(|(start_idx, chunk)| {
        for (i, val) in chunk.iter_mut().enumerate() {
            *val = f(start_idx + i);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_par_chunks_mut_auto_offsets() {
        let mut data: Vec<usize> = vec![0; 100];
        data.par_chunks_mut_auto().for_each(|(offset, chunk)| {
            for (i, val) in chunk.iter_mut().enumerate() {
                *val = offset + i;
            }
        });
        for (i, &v) in data.iter().enumerate() {
            assert_eq!(v, i);
        }
    }

    #[test]
    fn test_parallel_chunked_f32() {
        let mut result = vec![0.0f32; 10];
        parallel_chunked(&mut result, |i| i as f32 * 2.0);
        assert_eq!(result.len(), 10);
        for (i, &v) in result.iter().enumerate() {
            assert!((v - i as f32 * 2.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_parallel_chunked_empty() {
        let mut result: Vec<i32> = vec![];
        parallel_chunked(&mut result, |i| i as i32);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parallel_chunked_large() {
        let len = 100_000;
        let mut result = vec![0u32; len];
        parallel_chunked(&mut result, |i| i as u32);
        assert_eq!(result.len(), len);
        for (i, &v) in result.iter().enumerate() {
            assert_eq!(v, i as u32);
        }
    }
}
