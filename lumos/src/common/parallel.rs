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
        ParRows2MutWithOffset {
            inner: self
                .0
                .par_chunks_mut(chunk_size)
                .zip(self.1.par_chunks_mut(chunk_size)),
            chunk_rows,
        }
    }
}

/// Three zipped mutable slices ready for parallel row iteration.
pub struct ZippedSlices3<'a, A: Send, B: Send, C: Send>(
    pub &'a mut [A],
    pub &'a mut [B],
    pub &'a mut [C],
);

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
        ParRows3MutWithOffset {
            inner: self
                .0
                .par_chunks_mut(chunk_size)
                .zip(self.1.par_chunks_mut(chunk_size))
                .zip(self.2.par_chunks_mut(chunk_size)),
            chunk_rows,
        }
    }
}

/// Four zipped mutable slices ready for parallel row iteration.
pub struct ZippedSlices4<'a, A: Send, B: Send, C: Send, D: Send>(
    pub &'a mut [A],
    pub &'a mut [B],
    pub &'a mut [C],
    pub &'a mut [D],
);

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
        ParRows4MutWithOffset {
            inner: self
                .0
                .par_chunks_mut(chunk_size)
                .zip(self.1.par_chunks_mut(chunk_size))
                .zip(self.2.par_chunks_mut(chunk_size))
                .zip(self.3.par_chunks_mut(chunk_size)),
            chunk_rows,
        }
    }
}

// Type aliases for nested zips
type Zip3Inner<'a, A, B, C> = rayon::iter::Zip<
    rayon::iter::Zip<rayon::slice::ChunksMut<'a, A>, rayon::slice::ChunksMut<'a, B>>,
    rayon::slice::ChunksMut<'a, C>,
>;
type Zip4Inner<'a, A, B, C, D> =
    rayon::iter::Zip<Zip3Inner<'a, A, B, C>, rayon::slice::ChunksMut<'a, D>>;

/// Parallel iterator over two zipped row-aligned mutable chunks.
/// Yields `(y_start, (&mut [A], &mut [B]))` tuples.
pub struct ParRows2MutWithOffset<'a, A: Send, B: Send> {
    inner: rayon::iter::Zip<rayon::slice::ChunksMut<'a, A>, rayon::slice::ChunksMut<'a, B>>,
    chunk_rows: usize,
}

impl<'a, A: Send + 'a, B: Send + 'a> ParallelIterator for ParRows2MutWithOffset<'a, A, B> {
    type Item = (usize, (&'a mut [A], &'a mut [B]));

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: rayon::iter::plumbing::UnindexedConsumer<Self::Item>,
    {
        let chunk_rows = self.chunk_rows;
        self.inner
            .enumerate()
            .map(|(idx, (a, b))| (idx * chunk_rows, (a, b)))
            .drive_unindexed(consumer)
    }
}

impl<'a, A: Send + 'a, B: Send + 'a> IndexedParallelIterator for ParRows2MutWithOffset<'a, A, B> {
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
            .map(|(idx, (a, b))| (idx * chunk_rows, (a, b)))
            .drive(consumer)
    }

    fn with_producer<CB>(self, callback: CB) -> CB::Output
    where
        CB: rayon::iter::plumbing::ProducerCallback<Self::Item>,
    {
        let chunk_rows = self.chunk_rows;
        self.inner
            .enumerate()
            .map(|(idx, (a, b))| (idx * chunk_rows, (a, b)))
            .with_producer(callback)
    }
}

/// Parallel iterator over three zipped row-aligned mutable chunks.
pub struct ParRows3MutWithOffset<'a, A: Send, B: Send, C: Send> {
    inner: Zip3Inner<'a, A, B, C>,
    chunk_rows: usize,
}

impl<'a, A: Send + 'a, B: Send + 'a, C: Send + 'a> ParallelIterator
    for ParRows3MutWithOffset<'a, A, B, C>
{
    type Item = (usize, (&'a mut [A], &'a mut [B], &'a mut [C]));

    fn drive_unindexed<Co>(self, consumer: Co) -> Co::Result
    where
        Co: rayon::iter::plumbing::UnindexedConsumer<Self::Item>,
    {
        let chunk_rows = self.chunk_rows;
        self.inner
            .enumerate()
            .map(|(idx, ((a, b), c))| (idx * chunk_rows, (a, b, c)))
            .drive_unindexed(consumer)
    }
}

impl<'a, A: Send + 'a, B: Send + 'a, C: Send + 'a> IndexedParallelIterator
    for ParRows3MutWithOffset<'a, A, B, C>
{
    fn len(&self) -> usize {
        self.inner.len()
    }

    fn drive<Co>(self, consumer: Co) -> Co::Result
    where
        Co: rayon::iter::plumbing::Consumer<Self::Item>,
    {
        let chunk_rows = self.chunk_rows;
        self.inner
            .enumerate()
            .map(|(idx, ((a, b), c))| (idx * chunk_rows, (a, b, c)))
            .drive(consumer)
    }

    fn with_producer<CB>(self, callback: CB) -> CB::Output
    where
        CB: rayon::iter::plumbing::ProducerCallback<Self::Item>,
    {
        let chunk_rows = self.chunk_rows;
        self.inner
            .enumerate()
            .map(|(idx, ((a, b), c))| (idx * chunk_rows, (a, b, c)))
            .with_producer(callback)
    }
}

/// Parallel iterator over four zipped row-aligned mutable chunks.
pub struct ParRows4MutWithOffset<'a, A: Send, B: Send, C: Send, D: Send> {
    inner: Zip4Inner<'a, A, B, C, D>,
    chunk_rows: usize,
}

impl<'a, A: Send + 'a, B: Send + 'a, C: Send + 'a, D: Send + 'a> ParallelIterator
    for ParRows4MutWithOffset<'a, A, B, C, D>
{
    type Item = (usize, (&'a mut [A], &'a mut [B], &'a mut [C], &'a mut [D]));

    fn drive_unindexed<Co>(self, consumer: Co) -> Co::Result
    where
        Co: rayon::iter::plumbing::UnindexedConsumer<Self::Item>,
    {
        let chunk_rows = self.chunk_rows;
        self.inner
            .enumerate()
            .map(|(idx, (((a, b), c), d))| (idx * chunk_rows, (a, b, c, d)))
            .drive_unindexed(consumer)
    }
}

impl<'a, A: Send + 'a, B: Send + 'a, C: Send + 'a, D: Send + 'a> IndexedParallelIterator
    for ParRows4MutWithOffset<'a, A, B, C, D>
{
    fn len(&self) -> usize {
        self.inner.len()
    }

    fn drive<Co>(self, consumer: Co) -> Co::Result
    where
        Co: rayon::iter::plumbing::Consumer<Self::Item>,
    {
        let chunk_rows = self.chunk_rows;
        self.inner
            .enumerate()
            .map(|(idx, (((a, b), c), d))| (idx * chunk_rows, (a, b, c, d)))
            .drive(consumer)
    }

    fn with_producer<CB>(self, callback: CB) -> CB::Output
    where
        CB: rayon::iter::plumbing::ProducerCallback<Self::Item>,
    {
        let chunk_rows = self.chunk_rows;
        self.inner
            .enumerate()
            .map(|(idx, (((a, b), c), d))| (idx * chunk_rows, (a, b, c, d)))
            .with_producer(callback)
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

    #[test]
    fn test_par_rows_mut_auto_offsets() {
        let width = 10;
        let height = 20;
        let mut data: Vec<usize> = vec![0; width * height];

        data.par_rows_mut_auto(width).for_each(|(y_start, chunk)| {
            let rows_in_chunk = chunk.len() / width;
            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                for x in 0..width {
                    chunk[local_y * width + x] = y * width + x;
                }
            }
        });

        for (i, &v) in data.iter().enumerate() {
            assert_eq!(v, i);
        }
    }

    #[test]
    fn test_par_rows_mut_auto_row_alignment() {
        let width = 7;
        let height = 13;
        let mut data: Vec<u32> = vec![0; width * height];

        data.par_rows_mut_auto(width).for_each(|(y_start, chunk)| {
            assert_eq!(chunk.len() % width, 0, "Chunk not row-aligned");
            let rows_in_chunk = chunk.len() / width;
            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                for x in 0..width {
                    chunk[local_y * width + x] = y as u32;
                }
            }
        });

        for y in 0..height {
            for x in 0..width {
                assert_eq!(data[y * width + x], y as u32);
            }
        }
    }

    #[test]
    fn test_par_zip_two_slices() {
        let width = 8;
        let height = 10;
        let mut a: Vec<f32> = vec![0.0; width * height];
        let mut b: Vec<f32> = vec![0.0; width * height];

        a.as_mut_slice()
            .par_zip(&mut b)
            .par_rows_mut_auto(width)
            .for_each(|(y_start, (a_chunk, b_chunk))| {
                let rows_in_chunk = a_chunk.len() / width;
                for local_y in 0..rows_in_chunk {
                    let y = y_start + local_y;
                    for x in 0..width {
                        let idx = local_y * width + x;
                        a_chunk[idx] = (y * width + x) as f32;
                        b_chunk[idx] = (y * width + x) as f32 * 2.0;
                    }
                }
            });

        for i in 0..width * height {
            assert_eq!(a[i], i as f32);
            assert_eq!(b[i], i as f32 * 2.0);
        }
    }

    #[test]
    fn test_par_zip_three_slices() {
        let width = 5;
        let height = 8;
        let mut a: Vec<i32> = vec![0; width * height];
        let mut b: Vec<i32> = vec![0; width * height];
        let mut c: Vec<i32> = vec![0; width * height];

        a.as_mut_slice()
            .par_zip(&mut b)
            .par_zip(&mut c)
            .par_rows_mut_auto(width)
            .for_each(|(y_start, (a_chunk, b_chunk, c_chunk))| {
                let rows_in_chunk = a_chunk.len() / width;
                for local_y in 0..rows_in_chunk {
                    let y = y_start + local_y;
                    for x in 0..width {
                        let idx = local_y * width + x;
                        let val = (y * width + x) as i32;
                        a_chunk[idx] = val;
                        b_chunk[idx] = val * 2;
                        c_chunk[idx] = val * 3;
                    }
                }
            });

        for i in 0..width * height {
            assert_eq!(a[i], i as i32);
            assert_eq!(b[i], i as i32 * 2);
            assert_eq!(c[i], i as i32 * 3);
        }
    }

    #[test]
    fn test_par_zip_four_slices() {
        let width = 4;
        let height = 6;
        let mut a: Vec<u8> = vec![0; width * height];
        let mut b: Vec<u8> = vec![0; width * height];
        let mut c: Vec<u8> = vec![0; width * height];
        let mut d: Vec<u8> = vec![0; width * height];

        a.as_mut_slice()
            .par_zip(&mut b)
            .par_zip(&mut c)
            .par_zip(&mut d)
            .par_rows_mut_auto(width)
            .for_each(|(y_start, (a_chunk, b_chunk, c_chunk, d_chunk))| {
                let rows_in_chunk = a_chunk.len() / width;
                for local_y in 0..rows_in_chunk {
                    let y = y_start + local_y;
                    for x in 0..width {
                        let idx = local_y * width + x;
                        let val = (y * width + x) as u8;
                        a_chunk[idx] = val;
                        b_chunk[idx] = val.wrapping_add(1);
                        c_chunk[idx] = val.wrapping_add(2);
                        d_chunk[idx] = val.wrapping_add(3);
                    }
                }
            });

        for i in 0..width * height {
            let val = i as u8;
            assert_eq!(a[i], val);
            assert_eq!(b[i], val.wrapping_add(1));
            assert_eq!(c[i], val.wrapping_add(2));
            assert_eq!(d[i], val.wrapping_add(3));
        }
    }

    #[test]
    #[should_panic(expected = "equal length")]
    fn test_par_zip_unequal_lengths_panics() {
        let mut a: Vec<f32> = vec![0.0; 100];
        let mut b: Vec<f32> = vec![0.0; 50];

        a.as_mut_slice()
            .par_zip(&mut b)
            .par_rows_mut_auto(10)
            .for_each(|_| {});
    }

    #[test]
    fn test_par_rows_mut_auto_single_row() {
        let width = 100;
        let height = 1;
        let mut data: Vec<usize> = vec![0; width * height];

        data.par_rows_mut_auto(width).for_each(|(y_start, chunk)| {
            assert_eq!(y_start, 0);
            for (x, val) in chunk.iter_mut().enumerate() {
                *val = x;
            }
        });

        for (i, &v) in data.iter().enumerate() {
            assert_eq!(v, i);
        }
    }

    #[test]
    fn test_par_rows_mut_auto_large_image() {
        let width = 1920;
        let height = 1080;
        let mut data: Vec<u32> = vec![0; width * height];

        data.par_rows_mut_auto(width).for_each(|(y_start, chunk)| {
            let rows_in_chunk = chunk.len() / width;
            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                for x in 0..width {
                    chunk[local_y * width + x] = (y * width + x) as u32;
                }
            }
        });

        // Spot check some values
        assert_eq!(data[0], 0);
        assert_eq!(data[width], width as u32);
        assert_eq!(data[width * height - 1], (width * height - 1) as u32);
    }

    #[test]
    fn test_indexed_parallel_iterator_len() {
        let width = 10;
        let height = 100;
        let mut data: Vec<f32> = vec![0.0; width * height];

        let iter = data.par_rows_mut_auto(width);
        assert!(iter.len() > 0);
        assert!(iter.len() <= height);
    }

    #[test]
    fn test_par_zip_different_types() {
        let width = 6;
        let height = 4;
        let mut floats: Vec<f32> = vec![0.0; width * height];
        let mut ints: Vec<i32> = vec![0; width * height];

        floats
            .as_mut_slice()
            .par_zip(&mut ints)
            .par_rows_mut_auto(width)
            .for_each(|(y_start, (f_chunk, i_chunk))| {
                let rows_in_chunk = f_chunk.len() / width;
                for local_y in 0..rows_in_chunk {
                    let y = y_start + local_y;
                    for x in 0..width {
                        let idx = local_y * width + x;
                        let val = y * width + x;
                        f_chunk[idx] = val as f32 * 0.5;
                        i_chunk[idx] = val as i32 * 2;
                    }
                }
            });

        for i in 0..width * height {
            assert_eq!(floats[i], i as f32 * 0.5);
            assert_eq!(ints[i], i as i32 * 2);
        }
    }
}
