//! Parallel processing utilities.

use std::mem::size_of;

use rayon::prelude::*;

/// Target chunk size in bytes for parallel operations.
/// 16KB ensures each chunk spans multiple cache lines and reduces false sharing.
const TARGET_CHUNK_BYTES: usize = 16 * 1024;

/// Minimum chunk size to avoid excessive task overhead.
const MIN_CHUNK_SIZE: usize = 1024;

/// Calculate optimal chunk size based on element size.
/// Targets ~16KB per chunk, with a minimum of 1024 elements.
fn optimal_chunk_size<T>() -> usize {
    let element_size = size_of::<T>().max(1);
    (TARGET_CHUNK_BYTES / element_size).max(MIN_CHUNK_SIZE)
}

/// Apply a function to each index in parallel, modifying the slice in place.
/// Uses explicit chunking to avoid false cache sharing between threads.
/// Chunk size is automatically calculated based on element size (~16KB per chunk).
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

    let chunk_size = optimal_chunk_size::<T>();

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
    fn test_parallel_map_chunked_f32() {
        let mut result = vec![0.0f32; 10];
        parallel_chunked(&mut result, |i| i as f32 * 2.0);
        assert_eq!(result.len(), 10);
        for (i, &v) in result.iter().enumerate() {
            assert!((v - i as f32 * 2.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_parallel_map_chunked_bool() {
        let mut result = vec![false; 100];
        parallel_chunked(&mut result, |i| i % 3 == 0);
        assert_eq!(result.len(), 100);
        for (i, &v) in result.iter().enumerate() {
            assert_eq!(v, i % 3 == 0);
        }
    }

    #[test]
    fn test_parallel_map_chunked_empty() {
        let mut result: Vec<i32> = vec![];
        parallel_chunked(&mut result, |i| i as i32);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parallel_map_chunked_large() {
        let len = 100_000;
        let mut result = vec![0u32; len];
        parallel_chunked(&mut result, |i| i as u32);
        assert_eq!(result.len(), len);
        for (i, &v) in result.iter().enumerate() {
            assert_eq!(v, i as u32);
        }
    }

    #[test]
    fn test_optimal_chunk_size() {
        // f32: 4 bytes -> 16KB / 4 = 4096 elements
        assert_eq!(optimal_chunk_size::<f32>(), 4096);

        // bool: 1 byte -> 16KB / 1 = 16384 elements
        assert_eq!(optimal_chunk_size::<bool>(), 16384);

        // u64: 8 bytes -> 16KB / 8 = 2048 elements
        assert_eq!(optimal_chunk_size::<u64>(), 2048);

        // Large struct: 1KB -> 16KB / 1024 = 16, but min is 1024
        #[repr(C)]
        struct Large([u8; 1024]);
        assert_eq!(optimal_chunk_size::<Large>(), MIN_CHUNK_SIZE);
    }
}
