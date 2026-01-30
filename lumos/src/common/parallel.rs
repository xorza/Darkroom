//! Parallel processing utilities.

use rayon::prelude::*;

/// Multiplier for number of chunks relative to CPU threads.
/// Using 2x threads provides good load balancing when some chunks finish faster.
const CHUNKS_PER_THREAD: usize = 2;

/// Compute optimal rows per chunk for parallel image processing.
///
/// Returns the number of rows that divides the image height into roughly `num_cpus * 2` chunks,
/// ensuring good load balancing while avoiding excessive overhead from too many small chunks.
/// Minimum of 1 row per chunk.
#[inline]
pub fn rows_per_chunk(height: usize) -> usize {
    let num_chunks = rayon::current_num_threads() * CHUNKS_PER_THREAD;
    (height / num_chunks).max(1)
}

/// Apply a function to each index in parallel, modifying the slice in place.
///
/// # Arguments
/// * `data` - Mutable slice to fill with values
/// * `f` - Function that takes an index and returns a value
pub fn parallel_chunked<T, F>(data: &mut [T], f: F)
where
    T: Send + Sync,
    F: Fn(usize) -> T + Sync + Send,
{
    if data.is_empty() {
        return;
    }

    let num_chunks = rayon::current_num_threads() * CHUNKS_PER_THREAD;
    let chunk_size = (data.len() / num_chunks).max(1);

    data.par_chunks_mut(chunk_size)
        .enumerate()
        .for_each(|(chunk_idx, chunk)| {
            let start_idx = chunk_idx * chunk_size;
            for (i, val) in chunk.iter_mut().enumerate() {
                *val = f(start_idx + i);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

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
