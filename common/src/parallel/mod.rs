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

// ============================================================================
// Generic parallel iterator wrapper with offset
// ============================================================================

/// Generic parallel iterator that prepends an offset to each item.
/// Used to wrap chunked iterators and provide `(offset, item)` pairs.
pub struct WithOffset<I, T, F> {
    inner: I,
    multiplier: usize,
    transform: F,
    _phantom: std::marker::PhantomData<T>,
}

impl<I, T, F> WithOffset<I, T, F> {
    fn new(inner: I, multiplier: usize, transform: F) -> Self {
        Self {
            inner,
            multiplier,
            transform,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<I, T, F> ParallelIterator for WithOffset<I, T, F>
where
    I: IndexedParallelIterator,
    T: Send,
    F: Fn(I::Item) -> T + Send + Sync,
{
    type Item = (usize, T);

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: rayon::iter::plumbing::UnindexedConsumer<Self::Item>,
    {
        let multiplier = self.multiplier;
        let transform = self.transform;
        self.inner
            .enumerate()
            .map(move |(idx, item)| (idx * multiplier, transform(item)))
            .drive_unindexed(consumer)
    }
}

impl<I, T, F> IndexedParallelIterator for WithOffset<I, T, F>
where
    I: IndexedParallelIterator,
    T: Send,
    F: Fn(I::Item) -> T + Send + Sync,
{
    fn len(&self) -> usize {
        self.inner.len()
    }

    fn drive<C>(self, consumer: C) -> C::Result
    where
        C: rayon::iter::plumbing::Consumer<Self::Item>,
    {
        let multiplier = self.multiplier;
        let transform = self.transform;
        self.inner
            .enumerate()
            .map(move |(idx, item)| (idx * multiplier, transform(item)))
            .drive(consumer)
    }

    fn with_producer<CB>(self, callback: CB) -> CB::Output
    where
        CB: rayon::iter::plumbing::ProducerCallback<Self::Item>,
    {
        let multiplier = self.multiplier;
        let transform = self.transform;
        self.inner
            .enumerate()
            .map(move |(idx, item)| (idx * multiplier, transform(item)))
            .with_producer(callback)
    }
}

// ============================================================================
// Type aliases for common iterator types
// ============================================================================

/// Parallel iterator over row-aligned mutable chunks that yields `(chunk_start_row, chunk)` pairs.
pub type ParRowsMutWithOffset<'a, T> =
    WithOffset<rayon::slice::ChunksMut<'a, T>, &'a mut [T], fn(&'a mut [T]) -> &'a mut [T]>;

/// Parallel iterator over mutable chunks that yields `(offset, chunk)` pairs.
pub type ParChunksMutWithOffset<'a, T> =
    WithOffset<rayon::slice::ChunksMut<'a, T>, &'a mut [T], fn(&'a mut [T]) -> &'a mut [T]>;

/// Parallel iterator over two zipped row-aligned mutable chunks.
pub type ParRows2MutWithOffset<'a, A, B> = WithOffset<
    rayon::iter::Zip<rayon::slice::ChunksMut<'a, A>, rayon::slice::ChunksMut<'a, B>>,
    (&'a mut [A], &'a mut [B]),
    fn((&'a mut [A], &'a mut [B])) -> (&'a mut [A], &'a mut [B]),
>;

/// Parallel iterator over three zipped row-aligned mutable chunks.
#[allow(dead_code)]
pub type ParRows3MutWithOffset<'a, A, B, C> = WithOffset<
    rayon::iter::Zip<
        rayon::iter::Zip<rayon::slice::ChunksMut<'a, A>, rayon::slice::ChunksMut<'a, B>>,
        rayon::slice::ChunksMut<'a, C>,
    >,
    (&'a mut [A], &'a mut [B], &'a mut [C]),
    fn(((&'a mut [A], &'a mut [B]), &'a mut [C])) -> (&'a mut [A], &'a mut [B], &'a mut [C]),
>;

/// Parallel iterator over four zipped row-aligned mutable chunks.
#[allow(dead_code)]
pub type ParRows4MutWithOffset<'a, A, B, C, D> = WithOffset<
    rayon::iter::Zip<
        rayon::iter::Zip<
            rayon::iter::Zip<rayon::slice::ChunksMut<'a, A>, rayon::slice::ChunksMut<'a, B>>,
            rayon::slice::ChunksMut<'a, C>,
        >,
        rayon::slice::ChunksMut<'a, D>,
    >,
    (&'a mut [A], &'a mut [B], &'a mut [C], &'a mut [D]),
    fn(
        (((&'a mut [A], &'a mut [B]), &'a mut [C]), &'a mut [D]),
    ) -> (&'a mut [A], &'a mut [B], &'a mut [C], &'a mut [D]),
>;

// ============================================================================
// Extension traits
// ============================================================================

/// Extension trait for row-aligned mutable parallel chunks with automatic sizing.
pub trait ParRowsMutAuto<'a, T: Send + 'a> {
    type Iter: IndexedParallelIterator;

    /// Split into mutable parallel chunks aligned to row boundaries.
    /// Returns an iterator yielding `(chunk_start_row, chunk)` pairs where chunk contains complete rows.
    fn par_rows_mut_auto(&'a mut self, width: usize) -> Self::Iter;
}

impl<'a, T: Send + 'a> ParRowsMutAuto<'a, T> for [T] {
    type Iter = ParRowsMutWithOffset<'a, T>;

    fn par_rows_mut_auto(&'a mut self, width: usize) -> ParRowsMutWithOffset<'a, T> {
        let height = self.len() / width;
        let chunk_rows = auto_chunk_size(height);
        WithOffset::new(
            self.par_chunks_mut(width * chunk_rows),
            chunk_rows,
            identity as fn(&'a mut [T]) -> &'a mut [T],
        )
    }
}

fn identity<T>(x: T) -> T {
    x
}

/// Extension trait for chaining `par_zip` calls on mutable slices.
pub trait ParZipMut<'a, T: Send + 'a> {
    /// Zip this slice with another for parallel row-based iteration.
    fn par_zip<U: Send + 'a>(self, other: &'a mut [U]) -> ZippedSlices2<'a, T, U>;
}

impl<'a, T: Send + 'a> ParZipMut<'a, T> for &'a mut [T] {
    fn par_zip<U: Send + 'a>(self, other: &'a mut [U]) -> ZippedSlices2<'a, T, U> {
        ZippedSlices2(self, other)
    }
}

/// Two zipped mutable slices ready for parallel row iteration.
pub struct ZippedSlices2<'a, A: Send, B: Send>(pub &'a mut [A], pub &'a mut [B]);

impl<'a, A: Send + 'a, B: Send + 'a> ZippedSlices2<'a, A, B> {
    /// Zip with a third slice.
    #[allow(dead_code)]
    pub fn par_zip<C: Send + 'a>(self, other: &'a mut [C]) -> ZippedSlices3<'a, A, B, C> {
        ZippedSlices3(self.0, self.1, other)
    }

    /// Split into parallel row-aligned chunks.
    pub fn par_rows_mut_auto(self, width: usize) -> ParRows2MutWithOffset<'a, A, B> {
        assert_eq!(
            self.0.len(),
            self.1.len(),
            "Zipped slices must have equal length"
        );
        let height = self.0.len() / width;
        let chunk_rows = auto_chunk_size(height);
        let chunk_size = width * chunk_rows;
        WithOffset::new(
            self.0
                .par_chunks_mut(chunk_size)
                .zip(self.1.par_chunks_mut(chunk_size)),
            chunk_rows,
            identity as fn((&'a mut [A], &'a mut [B])) -> (&'a mut [A], &'a mut [B]),
        )
    }
}

/// Three zipped mutable slices ready for parallel row iteration.
#[allow(dead_code)]
pub struct ZippedSlices3<'a, A: Send, B: Send, C: Send>(
    pub &'a mut [A],
    pub &'a mut [B],
    pub &'a mut [C],
);

#[allow(dead_code)]
impl<'a, A: Send + 'a, B: Send + 'a, C: Send + 'a> ZippedSlices3<'a, A, B, C> {
    /// Zip with a fourth slice.
    pub fn par_zip<D: Send + 'a>(self, other: &'a mut [D]) -> ZippedSlices4<'a, A, B, C, D> {
        ZippedSlices4(self.0, self.1, self.2, other)
    }

    /// Split into parallel row-aligned chunks.
    pub fn par_rows_mut_auto(self, width: usize) -> ParRows3MutWithOffset<'a, A, B, C> {
        assert_eq!(
            self.0.len(),
            self.1.len(),
            "Zipped slices must have equal length"
        );
        assert_eq!(
            self.0.len(),
            self.2.len(),
            "Zipped slices must have equal length"
        );
        let height = self.0.len() / width;
        let chunk_rows = auto_chunk_size(height);
        let chunk_size = width * chunk_rows;
        WithOffset::new(
            self.0
                .par_chunks_mut(chunk_size)
                .zip(self.1.par_chunks_mut(chunk_size))
                .zip(self.2.par_chunks_mut(chunk_size)),
            chunk_rows,
            flatten_zip3
                as fn(
                    ((&'a mut [A], &'a mut [B]), &'a mut [C]),
                ) -> (&'a mut [A], &'a mut [B], &'a mut [C]),
        )
    }
}

fn flatten_zip3<A, B, C>(((a, b), c): ((A, B), C)) -> (A, B, C) {
    (a, b, c)
}

/// Four zipped mutable slices ready for parallel row iteration.
#[allow(dead_code)]
pub struct ZippedSlices4<'a, A: Send, B: Send, C: Send, D: Send>(
    pub &'a mut [A],
    pub &'a mut [B],
    pub &'a mut [C],
    pub &'a mut [D],
);

#[allow(dead_code)]
impl<'a, A: Send + 'a, B: Send + 'a, C: Send + 'a, D: Send + 'a> ZippedSlices4<'a, A, B, C, D> {
    /// Split into parallel row-aligned chunks.
    pub fn par_rows_mut_auto(self, width: usize) -> ParRows4MutWithOffset<'a, A, B, C, D> {
        assert_eq!(
            self.0.len(),
            self.1.len(),
            "Zipped slices must have equal length"
        );
        assert_eq!(
            self.0.len(),
            self.2.len(),
            "Zipped slices must have equal length"
        );
        assert_eq!(
            self.0.len(),
            self.3.len(),
            "Zipped slices must have equal length"
        );
        let height = self.0.len() / width;
        let chunk_rows = auto_chunk_size(height);
        let chunk_size = width * chunk_rows;
        WithOffset::new(
            self.0
                .par_chunks_mut(chunk_size)
                .zip(self.1.par_chunks_mut(chunk_size))
                .zip(self.2.par_chunks_mut(chunk_size))
                .zip(self.3.par_chunks_mut(chunk_size)),
            chunk_rows,
            flatten_zip4
                as fn(
                    (((&'a mut [A], &'a mut [B]), &'a mut [C]), &'a mut [D]),
                ) -> (&'a mut [A], &'a mut [B], &'a mut [C], &'a mut [D]),
        )
    }
}

fn flatten_zip4<A, B, C, D>((((a, b), c), d): (((A, B), C), D)) -> (A, B, C, D) {
    (a, b, c, d)
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
        WithOffset::new(
            self.par_chunks_mut(chunk_size),
            chunk_size,
            identity as fn(&'a mut [T]) -> &'a mut [T],
        )
    }
}
